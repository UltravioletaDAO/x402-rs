#!/usr/bin/env python3
"""Unstick the facilitator's EVM mainnet signer on Polygon.

WHY THIS EXISTS
---------------
On 2026-09-03 21:28:00Z Polygon's base fee was in a trough of 1.07 gwei (it had
collapsed from its usual ~250 gwei and was on its way back up). The facilitator
priced an escrow `release` there with alloy's default estimator, whose whole
buffer is `2 * baseFee`, giving maxFeePerGas = 32.25 gwei. Forty minutes later
the base fee was back at 248 gwei and that transaction -- nonce 1157 -- could
never be mined again.

Nonces are strictly ordered, so one unmineable transaction at the head freezes
the account. 399 later transactions piled up behind it, and their combined
`gasLimit * maxFeePerGas` reservation reached 82.80 of the signer's 82.86 POL,
which is why every NEW submission since has been refused by the node with
`insufficient funds for gas * price + value: ... queued cost ...`.

Note that "queued cost" in that message is geth/bor's term for *everything
already in the pool for this account*, not the pool's `queued` (non-executable)
bucket. This account has nothing in the queued bucket: all 400 transactions are
`pending` and their nonces are contiguous. There is no nonce gap.

WHAT THIS SCRIPT DOES
---------------------
It replaces stuck transactions with correctly priced 0-value self-transfers
(21000 gas), in ascending nonce order, honouring bor's replace-by-fee rule.

    --mode head        replace ONLY the unmineable head transaction.  The rest
                       of the pool is already priced above the base fee, so it
                       drains on its own once the head clears.  RECOMMENDED.
    --mode cancel-all  replace every pooled nonce.  Cheaper in gas, but it is
                       N signed transactions with the production key instead of
                       one.

SAFETY
------
* The private key is read from AWS Secrets Manager BY NAME, inside this process.
  It is never printed, never written to disk, never passed on a command line.
* The RPC URL is read the same way and is never printed either (it carries an
  API key). Only its hostname is shown.
* `--dry-run` (the default) performs no writes at all.
* `--apply` refuses to run unless the derived address matches --signer, the
  chain id matches --chain-id, and the operator confirms interactively (or
  passes --yes).

Usage:
    python3 scripts/polygon_destrabar_cola.py --dry-run
    python3 scripts/polygon_destrabar_cola.py --dry-run --mode cancel-all
    python3 scripts/polygon_destrabar_cola.py --apply            # needs the go
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from typing import Any, Dict, List, Optional
from urllib.parse import urlparse

GWEI = 1_000_000_000
SELF_TRANSFER_GAS = 21_000

# The facilitator's EVM mainnet signer. `lambda/balances/handler.py` is the
# authoritative source for this address; it is public.
DEFAULT_SIGNER = "0x103040545AC5031A11E8C03dd11324C7333a13C7"

# Secret names come from terraform/environments/production/secrets.tf.
DEFAULT_KEY_SECRET = "facilitator-evm-mainnet-private-key"
DEFAULT_KEY_SECRET_FIELD = "private_key"
DEFAULT_RPC_SECRET = "facilitator-rpc-mainnet"
DEFAULT_RPC_SECRET_FIELD = "polygon"
DEFAULT_REGION = "us-east-2"

POLYGON_CHAIN_ID = 137

# bor inherits geth's txpool `PriceBump = 10`: a replacement needs BOTH
# maxFeePerGas and maxPriorityFeePerGas at least 10% above the transaction it
# replaces. We ask for more than the minimum so that a base-fee tick between
# building and broadcasting cannot land us exactly on the boundary.
DEFAULT_BUMP_PCT = 12.5

# Polygon's base fee sits at ~250 gwei in steady state and periodically collapses
# to ~0 for hours before snapping back within the hour (measured three times
# between 2026-09-01 and 2026-09-03). maxFeePerGas is a CAP, not a payment -- you
# are charged baseFee + priority regardless -- so a generous floor costs nothing
# except pool reservation headroom, and is the only thing that survives a swing
# of that size.
DEFAULT_MIN_MAX_FEE_GWEI = 1000.0
DEFAULT_MIN_PRIORITY_GWEI = 40.0
DEFAULT_BASE_FEE_MULTIPLIER = 4.0


# --------------------------------------------------------------------------
# Fee arithmetic. Pure functions, no I/O -- this is what the tests exercise.
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class FeePolicy:
    """Everything that decides what a replacement transaction is priced at."""

    bump_pct: float = DEFAULT_BUMP_PCT
    min_max_fee: int = int(DEFAULT_MIN_MAX_FEE_GWEI * GWEI)
    min_priority: int = int(DEFAULT_MIN_PRIORITY_GWEI * GWEI)
    base_fee_multiplier: float = DEFAULT_BASE_FEE_MULTIPLIER


def bump(value: int, pct: float) -> int:
    """Smallest integer at least `pct` percent above `value`.

    Integer arithmetic, and rounding UP. Two reasons, both learned the hard way:

    * bor compares with `>=` against its own integer threshold, so landing one
      wei short is a refused replacement, which on this rail means the queue
      stays stuck;
    * `value * (1 + pct/100)` in floating point overshoots -- 100 at +10% comes
      out as 110.00000000000001 and ceils to 111. Harmless for a fee, but it
      makes the function untestable against an exact expectation, and a
      primitive nobody can pin down is a primitive nobody trusts.

    `pct` is taken to three decimal places, which covers bor's 10 and our 12.5.
    """
    if value < 0:
        raise ValueError("fee cannot be negative")
    scale = 100_000
    numerator = value * int(round((100.0 + pct) * 1000))
    return -(-numerator // scale)  # ceiling division, integers throughout


def price_replacement(
    old_max_fee: int, old_priority: int, base_fee: int, policy: FeePolicy
) -> tuple[int, int]:
    """Price a transaction that must both replace `old_*` and actually mine.

    Three constraints, all of which must hold at once:

    1. replace-by-fee: both fields at least `bump_pct` above the old ones, or
       bor answers `replacement transaction underpriced` and nothing moves;
    2. mineable: maxFeePerGas comfortably above the current base fee -- this is
       the constraint the stuck transaction failed;
    3. floors: above the chain's known steady-state cost, so that pricing during
       a base-fee trough does not reproduce the original incident.
    """
    priority = max(
        bump(old_priority, policy.bump_pct),
        policy.min_priority,
    )
    max_fee = max(
        bump(old_max_fee, policy.bump_pct),
        int(base_fee * policy.base_fee_multiplier) + priority,
        policy.min_max_fee,
    )
    # A type-2 transaction with maxFeePerGas < maxPriorityFeePerGas is invalid
    # and every node rejects it. Reachable whenever a floor lifts the priority
    # above a maxFee that the other two constraints left low.
    if max_fee < priority:
        max_fee = priority
    return max_fee, priority


def effective_gas_price(max_fee: int, priority: int, base_fee: int) -> int:
    """What the sender is actually charged per gas (EIP-1559).

    Not `max_fee`: the unused part of the cap is never spent. The cap only
    decides whether the transaction is admissible and how much the txpool
    reserves against the balance.
    """
    return min(max_fee, base_fee + priority)


@dataclass(frozen=True)
class PlannedTx:
    nonce: int
    max_fee: int
    priority: int
    gas: int
    old_max_fee: int
    old_priority: int
    old_gas: int
    old_reserved: int

    @property
    def reserved(self) -> int:
        """What the txpool holds against the balance for this transaction."""
        return self.gas * self.max_fee

    def cost(self, base_fee: int) -> int:
        return self.gas * effective_gas_price(self.max_fee, self.priority, base_fee)


def plan_replacements(
    pool: Dict[int, Dict[str, int]],
    first_unmined: int,
    mode: str,
    base_fee: int,
    policy: FeePolicy,
) -> List[PlannedTx]:
    """Decide which nonces to replace, in the order they must be sent.

    Ascending nonce order is not cosmetic. Each replacement of a big pooled
    transaction with a 21000-gas one FREES reservation, so going upward keeps
    the account solvent from the first step; going downward would ask the node
    to accept the most expensive replacement while the pool is still full.
    """
    if mode not in ("head", "cancel-all"):
        raise ValueError(f"unknown mode {mode!r}")
    if first_unmined not in pool:
        raise ValueError(
            f"nonce {first_unmined} (the account's next nonce) is not in the pool; "
            "nothing is stuck, or the pool was read from a different node"
        )

    nonces = sorted(n for n in pool if n >= first_unmined)
    if mode == "head":
        nonces = nonces[:1]

    planned = []
    for n in nonces:
        entry = pool[n]
        max_fee, priority = price_replacement(
            entry["max_fee"], entry["priority"], base_fee, policy
        )
        planned.append(
            PlannedTx(
                nonce=n,
                max_fee=max_fee,
                priority=priority,
                gas=SELF_TRANSFER_GAS,
                old_max_fee=entry["max_fee"],
                old_priority=entry["priority"],
                old_gas=entry["gas"],
                old_reserved=entry["gas"] * entry["max_fee"],
            )
        )
    return planned


def reservation_headroom(
    planned: List[PlannedTx], pool_reserved_total: int, balance: int
) -> tuple[int, Optional[PlannedTx]]:
    """Worst-case shortfall while walking the plan, and where it happens.

    The node re-checks `balance >= sum(cost of everything pooled for this
    account)` on EVERY insert. Replacing the head -- a cheap transaction -- with
    a properly priced one momentarily RAISES that sum, and this account has only
    a sliver of free balance left. Walk the plan and report the tightest point
    rather than discovering it halfway through an --apply.
    """
    reserved = pool_reserved_total
    worst = balance - reserved
    worst_at = None
    for tx in planned:
        reserved = reserved - tx.old_reserved + tx.reserved
        free = balance - reserved
        if free < worst:
            worst, worst_at = free, tx
    return worst, worst_at


# --------------------------------------------------------------------------
# I/O
# --------------------------------------------------------------------------


class Rpc:
    """Minimal JSON-RPC client that never reveals its endpoint."""

    def __init__(self, url: str, timeout: int = 45):
        import requests

        self._url = url
        self._session = requests.Session()
        self._timeout = timeout
        self._id = 0

    @property
    def host(self) -> str:
        return urlparse(self._url).hostname or "<unknown>"

    def call(self, method: str, params: Optional[List[Any]] = None) -> Any:
        self._id += 1
        payload = {
            "jsonrpc": "2.0",
            "id": self._id,
            "method": method,
            "params": params or [],
        }
        response = self._session.post(self._url, json=payload, timeout=self._timeout)
        body = response.json()
        if "error" in body:
            # Scrub defensively: some providers echo the request URL back inside
            # error payloads, and that URL carries an API key.
            message = str(body["error"]).replace(self._url, "<rpc>")
            raise RuntimeError(f"{method} failed: {message}")
        return body["result"]


def read_secret_field(secret_id: str, field: str, region: str) -> str:
    """Fetch one field of a JSON secret. The value never leaves this process."""
    import boto3

    client = boto3.client("secretsmanager", region_name=region)
    raw = client.get_secret_value(SecretId=secret_id)["SecretString"]
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        # Some secrets are stored as a bare string rather than a JSON object.
        return raw.strip()
    if field not in parsed:
        raise KeyError(f"secret {secret_id!r} has no field {field!r}")
    return str(parsed[field]).strip()


def read_pool(rpc: Rpc, signer: str) -> Dict[int, Dict[str, int]]:
    """Pending transactions for `signer`, keyed by nonce.

    `txpool_contentFrom` is the only view that answers the question this script
    asks -- what THIS account has in THIS node's pool. `eth_getTransactionCount`
    with the pending tag gives a count, not the fees, and different nodes hold
    different pools (measured on 2026-09-10: the provider reported 400 pending
    for this signer, a public node reported none of them).
    """
    content = rpc.call("txpool_contentFrom", [signer])
    queued = content.get("queued") or {}
    if queued:
        print(
            f"NOTE: {len(queued)} transaction(s) in the node's non-executable "
            "`queued` bucket. This script only replaces `pending` ones; a "
            "genuine nonce gap needs a filler transaction, not a replacement.",
            file=sys.stderr,
        )
    pool = {}
    for nonce_str, tx in (content.get("pending") or {}).items():
        # A pre-1559 transaction has only `gasPrice`; a type-2 one has both, and
        # bor reports `gasPrice` equal to maxFeePerGas there. Read the 1559
        # fields when present and fall back LAZILY -- `tx.get(k, tx["gasPrice"])`
        # evaluates the fallback even when the key exists, so it raises on any
        # entry without `gasPrice` instead of using the value that is right there.
        legacy = tx.get("gasPrice")
        max_fee = tx.get("maxFeePerGas", legacy)
        priority = tx.get("maxPriorityFeePerGas", legacy)
        if max_fee is None or priority is None:
            raise RuntimeError(f"pooled transaction at nonce {nonce_str} has no fee fields")
        pool[int(nonce_str)] = {
            "max_fee": int(max_fee, 16),
            "priority": int(priority, 16),
            "gas": int(tx["gas"], 16),
            "to": tx.get("to"),
            "hash": tx.get("hash"),
        }
    return pool


def latest_base_fee(rpc: Rpc) -> int:
    block = rpc.call("eth_getBlockByNumber", ["latest", False])
    return int(block.get("baseFeePerGas", "0x0"), 16)


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def pol(wei: int) -> str:
    return f"{wei / 1e18:.6f} POL"


def usd(wei: int, price: Optional[float]) -> str:
    return f" (${wei / 1e18 * price:,.2f})" if price else ""


def fetch_pol_price() -> Optional[float]:
    try:
        import requests

        r = requests.get(
            "https://api.coingecko.com/api/v3/simple/price",
            params={"ids": "polygon-ecosystem-token", "vs_currencies": "usd"},
            timeout=15,
        )
        return float(next(iter(r.json().values()))["usd"])
    except Exception:
        return None


def report(
    planned: List[PlannedTx],
    base_fee: int,
    balance: int,
    pool_reserved_total: int,
    price: Optional[float],
    mode: str,
) -> None:
    print(f"\nplan: {mode}, {len(planned)} replacement transaction(s)")
    print(
        f"  base fee now {base_fee / GWEI:.2f} gwei | "
        f"balance {pol(balance)} | pool reserves {pol(pool_reserved_total)} | "
        f"free {pol(balance - pool_reserved_total)}"
    )
    print(
        f"\n  {'nonce':>6}  {'replaces (maxFee/prio gwei)':>30}  "
        f"{'sends (maxFee/prio gwei)':>26}  {'charged':>16}"
    )
    total = 0
    shown = 0
    for tx in planned:
        total += tx.cost(base_fee)
        if len(planned) <= 24 or shown < 10 or tx is planned[-1]:
            print(
                f"  {tx.nonce:>6}  "
                f"{tx.old_max_fee / GWEI:>14.3f} / {tx.old_priority / GWEI:>11.3f}  "
                f"{tx.max_fee / GWEI:>12.1f} / {tx.priority / GWEI:>9.1f}  "
                f"{pol(tx.cost(base_fee)):>16}"
            )
        elif shown == 10:
            print(f"  {'...':>6}  ({len(planned) - 11} more)")
        shown += 1

    worst_free, worst_at = reservation_headroom(planned, pool_reserved_total, balance)
    print(f"\n  total charged to the signer: {pol(total)}{usd(total, price)}")
    print(
        f"  tightest free balance during the walk: {pol(worst_free)}"
        + (f" (at nonce {worst_at.nonce})" if worst_at else " (before the first send)")
    )
    if worst_free < 0:
        print(
            "  REFUSING: the plan would overdraw the pool's reservation. "
            "Fund the signer, or use --mode cancel-all which frees reservation "
            "as it walks upward."
        )


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------


def main(argv: Optional[List[str]] = None) -> int:
    p = argparse.ArgumentParser(
        description="Unstick the facilitator's EVM signer on Polygon.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    action = p.add_mutually_exclusive_group()
    action.add_argument(
        "--dry-run",
        action="store_true",
        help="show the plan and send nothing (default)",
    )
    action.add_argument(
        "--apply", action="store_true", help="SIGN AND SEND the plan, in order"
    )
    p.add_argument(
        "--mode",
        choices=("head", "cancel-all"),
        default="head",
        help="head: replace only the unmineable head (default). "
        "cancel-all: replace every pooled nonce.",
    )
    p.add_argument("--signer", default=DEFAULT_SIGNER)
    p.add_argument("--chain-id", type=int, default=POLYGON_CHAIN_ID)
    p.add_argument("--region", default=DEFAULT_REGION)
    p.add_argument("--key-secret", default=DEFAULT_KEY_SECRET)
    p.add_argument("--key-secret-field", default=DEFAULT_KEY_SECRET_FIELD)
    p.add_argument("--rpc-secret", default=DEFAULT_RPC_SECRET)
    p.add_argument("--rpc-secret-field", default=DEFAULT_RPC_SECRET_FIELD)
    p.add_argument("--bump-pct", type=float, default=DEFAULT_BUMP_PCT)
    p.add_argument("--min-max-fee-gwei", type=float, default=DEFAULT_MIN_MAX_FEE_GWEI)
    p.add_argument("--min-priority-gwei", type=float, default=DEFAULT_MIN_PRIORITY_GWEI)
    p.add_argument(
        "--base-fee-multiplier", type=float, default=DEFAULT_BASE_FEE_MULTIPLIER
    )
    p.add_argument(
        "--yes", action="store_true", help="skip the interactive confirmation on --apply"
    )
    p.add_argument("--limit", type=int, default=0, help="cap the plan (0 = no cap)")
    args = p.parse_args(argv)

    policy = FeePolicy(
        bump_pct=args.bump_pct,
        min_max_fee=int(args.min_max_fee_gwei * GWEI),
        min_priority=int(args.min_priority_gwei * GWEI),
        base_fee_multiplier=args.base_fee_multiplier,
    )

    rpc_url = read_secret_field(args.rpc_secret, args.rpc_secret_field, args.region)
    rpc = Rpc(rpc_url)
    print(f"provider host: {rpc.host}")

    chain_id = int(rpc.call("eth_chainId"), 16)
    if chain_id != args.chain_id:
        print(
            f"REFUSING: provider reports chain id {chain_id}, expected {args.chain_id}.",
            file=sys.stderr,
        )
        return 2

    signer = args.signer
    latest = int(rpc.call("eth_getTransactionCount", [signer, "latest"]), 16)
    pending = int(rpc.call("eth_getTransactionCount", [signer, "pending"]), 16)
    balance = int(rpc.call("eth_getBalance", [signer, "latest"]), 16)
    base_fee = latest_base_fee(rpc)
    pool = read_pool(rpc, signer)
    pool_reserved_total = sum(v["gas"] * v["max_fee"] for v in pool.values())

    print(f"signer {signer}")
    print(
        f"  nonce latest={latest} pending={pending} "
        f"({pending - latest} transaction(s) not mined) | pooled here: {len(pool)}"
    )

    if not pool:
        print("nothing pending for this signer on this node; nothing to do.")
        return 0

    unmineable = sorted(n for n, v in pool.items() if v["max_fee"] < base_fee)
    print(
        f"  pooled transactions priced BELOW the current base fee "
        f"({base_fee / GWEI:.2f} gwei): {unmineable if unmineable else 'none'}"
    )

    try:
        planned = plan_replacements(pool, latest, args.mode, base_fee, policy)
    except ValueError as exc:
        print(f"REFUSING: {exc}", file=sys.stderr)
        return 2
    if args.limit:
        planned = planned[: args.limit]

    price = fetch_pol_price()
    report(planned, base_fee, balance, pool_reserved_total, price, args.mode)

    worst_free, _ = reservation_headroom(planned, pool_reserved_total, balance)
    if worst_free < 0:
        return 2

    if not args.apply:
        print("\n--dry-run: nothing was sent.")
        return 0

    return apply_plan(rpc, signer, planned, args, base_fee)


def apply_plan(
    rpc: Rpc, signer: str, planned: List[PlannedTx], args: Any, base_fee: int
) -> int:
    from eth_account import Account

    if not args.yes:
        print(
            f"\nAbout to sign and send {len(planned)} transaction(s) with the "
            f"PRODUCTION facilitator key on chain {args.chain_id}."
        )
        if input("Type 'destrabar' to proceed: ").strip() != "destrabar":
            print("aborted.")
            return 1

    key = read_secret_field(args.key_secret, args.key_secret_field, args.region)
    if not key.startswith("0x"):
        key = "0x" + key
    account = Account.from_key(key)
    del key  # the Account keeps what it needs; do not leave a second copy around

    if account.address.lower() != signer.lower():
        # Printing the derived address is safe (it is public) and is the only
        # way to tell "wrong secret" from "wrong --signer".
        print(
            f"REFUSING: key in {args.key_secret} derives {account.address}, "
            f"not {signer}.",
            file=sys.stderr,
        )
        return 2

    sent = 0
    for tx in planned:
        raw = account.sign_transaction(
            {
                "type": 2,
                "chainId": args.chain_id,
                "nonce": tx.nonce,
                "to": account.address,  # 0-value self-transfer: moves nothing
                "value": 0,
                "gas": tx.gas,
                "maxFeePerGas": tx.max_fee,
                "maxPriorityFeePerGas": tx.priority,
                "data": b"",
            }
        )
        try:
            tx_hash = rpc.call("eth_sendRawTransaction", [encode_raw(raw)])
        except RuntimeError as exc:
            print(f"  nonce {tx.nonce}: SEND FAILED: {exc}", file=sys.stderr)
            print(
                f"  stopping after {sent} successful replacement(s). "
                "Re-run --dry-run to see the current state.",
                file=sys.stderr,
            )
            return 3
        print(f"  nonce {tx.nonce}: sent {tx_hash}", flush=True)
        receipt = wait_for_receipt(rpc, tx_hash)
        status = int(receipt.get("status", "0x0"), 16)
        used = int(receipt.get("gasUsed", "0x0"), 16)
        eff = int(receipt.get("effectiveGasPrice", "0x0"), 16)
        print(
            f"  nonce {tx.nonce}: mined in block "
            f"{int(receipt['blockNumber'], 16)} status={status} "
            f"gasUsed={used} charged={pol(used * eff)}",
            flush=True,
        )
        sent += 1

    print(f"\ndone: {sent} replacement(s) mined.")
    return 0


def encode_raw(signed: Any) -> str:
    """`0x`-prefixed hex of a signed transaction, across eth_account versions.

    eth-account renamed `rawTransaction` to `raw_transaction` in 0.13, and the
    bytes it hands back are sometimes `HexBytes` (whose `.hex()` already carries
    the prefix) and sometimes plain `bytes` (whose `.hex()` does not). Getting
    this wrong produces a malformed payload that the node rejects, which on this
    rail looks exactly like a pricing failure.
    """
    payload = getattr(signed, "raw_transaction", None)
    if payload is None:
        payload = signed.rawTransaction
    text = payload.hex()
    return text if text.startswith("0x") else "0x" + text


def wait_for_receipt(rpc: Rpc, tx_hash: str, attempts: int = 150) -> Dict[str, Any]:
    """Block until the receipt exists. Polygon blocks are ~2s."""
    import time

    for _ in range(attempts):
        receipt = rpc.call("eth_getTransactionReceipt", [tx_hash])
        if receipt:
            return receipt
        time.sleep(2)
    raise RuntimeError(f"no receipt for {tx_hash} after {attempts * 2}s")


if __name__ == "__main__":
    sys.exit(main())
