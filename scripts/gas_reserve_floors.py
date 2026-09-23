#!/usr/bin/env python3
"""Measure the fee cap each alarmed EVM mainnet reserves per settle, and the
low-balance floor that buys `warnSettles` settles at it.

`GET /health/ready` calls a signer `degraded` below DEFAULT_WARN_SETTLES
settles, each reserving SETTLE_GAS_BUDGET gas at the fee cap the send path
would set (`quote_fee_cap` in src/chain/evm.rs). The CloudWatch alarm
`chain_balance_low` (terraform/environments/production/alerts.tf) is meant to
fire at the same point, so its floor is

    min_native = SETTLE_GAS_BUDGET * fee_cap * warnSettles

This script reads both constants out of src/readiness.rs, prices the fee cap
the way src/chain/evm.rs does, and prints the `evm_fee_cap_gwei` map that
alerts.tf derives the floors from. The fee cap moves with the chain; the map is
a dated reading, and /health/ready stays the live figure.

Read-only: eth_chainId, eth_feeHistory and eth_maxPriorityFeePerGas against
public RPCs. No key, no transaction.

    python3 scripts/gas_reserve_floors.py          # table
    python3 scripts/gas_reserve_floors.py --hcl    # the map to paste into alerts.tf
    python3 scripts/gas_reserve_floors.py --json   # machine-readable
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import time
import urllib.request
from decimal import Decimal
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GWEI = 10**9

# (alarm key in alerts.tf, chain id, public RPC). The keys are the `Chain`
# dimension the balances Lambda publishes; the RPCs carry no credential.
CHAINS = [
    ("arbitrum-mainnet", 42161, "https://arb1.arbitrum.io/rpc"),
    ("arc-mainnet", 5042, "https://rpc.mainnet.arc.io"),
    ("avalanche-mainnet", 43114, "https://avalanche-c-chain-rpc.publicnode.com"),
    ("base-mainnet", 8453, "https://mainnet.base.org"),
    ("celo-mainnet", 42220, "https://forno.celo.org"),
    ("ethereum-mainnet", 1, "https://ethereum-rpc.publicnode.com"),
    ("monad-mainnet", 143, "https://rpc.monad.xyz"),
    ("optimism-mainnet", 10, "https://mainnet.optimism.io"),
    ("polygon-mainnet", 137, "https://polygon-bor-rpc.publicnode.com"),
]

# Mirror of `eip1559_fee_floor` in src/chain/evm.rs, in wei:
# (min_priority, min_max_fee, fallback_base_fee). Every chain above prices
# type-2 transactions (`is_eip1559` in the same file).
FLOORS = {
    "ethereum-mainnet": (GWEI, 5 * GWEI, 2 * GWEI),
    "polygon-mainnet": (30 * GWEI, 1000 * GWEI, 250 * GWEI),
    "arc-mainnet": (1_000_000, 20 * GWEI, 20 * GWEI),
}
DEFAULT_FLOOR = (1_000_000, 0, 2 * GWEI)
BASE_FEE_MULTIPLIER = 2  # `BASE_FEE_MULTIPLIER` in src/chain/evm.rs


def readiness_constants() -> tuple[int, int]:
    """SETTLE_GAS_BUDGET and DEFAULT_WARN_SETTLES, read from the source."""
    src = (REPO / "src" / "readiness.rs").read_text(encoding="utf-8")

    def const(name: str) -> int:
        match = re.search(rf"pub const {name}: u\d+ = ([0-9_]+);", src)
        if not match:
            sys.exit(f"{name} not found in src/readiness.rs")
        return int(match.group(1).replace("_", ""))

    return const("SETTLE_GAS_BUDGET"), const("DEFAULT_WARN_SETTLES")


def fee_cap(base_fee: int, rpc_priority: int | None, floor: tuple[int, int, int]) -> int:
    """`compute_eip1559_fees` in src/chain/evm.rs: the maxFeePerGas it sets."""
    min_priority, min_max_fee, _ = floor
    priority = max(rpc_priority if rpc_priority is not None else min_priority, min_priority)
    max_fee = max(base_fee * BASE_FEE_MULTIPLIER + priority, min_max_fee)
    return max(max_fee, priority)


def round_up(value: float, digits: int = 3) -> float:
    """Round up to `digits` significant figures, so a floor never shrinks.

    A value at or below zero is an error, not 0.0: alerts.tf divides by the fee
    cap, and a zero pasted there fails the plan after the merge.
    """
    if value <= 0:
        raise ValueError(f"fee cap {value} is not above zero")
    scale = 10 ** (digits - 1 - math.floor(math.log10(value)))
    return math.ceil(value * scale) / scale


def render_hcl(rows: list[dict], measured_at: str, missing: list[str]) -> str:
    """The `evm_fee_cap_gwei` map for alerts.tf, or ValueError.

    Refuses a partial map: pasting one that lacks a chain removes that chain's
    alarms. Refuses a cap at or below zero for the division above.
    """
    if missing:
        raise ValueError(f"not measured: {', '.join(missing)}; no map printed")
    for row in rows:
        if not row["fee_cap_gwei"] > 0:
            raise ValueError(f"{row['chain']}: fee cap {row['fee_cap_gwei']} is not above zero")
    width = max(len(r["chain"]) for r in rows) + 2
    lines = [f"  # scripts/gas_reserve_floors.py --hcl, {measured_at}", "  evm_fee_cap_gwei = {"]
    lines += [f"    {json.dumps(r['chain']).ljust(width)} = {r['fee_cap_gwei']:g}" for r in rows]
    lines.append("  }")
    return "\n".join(lines)


def rpc(url: str, method: str, params: list) -> object:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=body, headers={
        "Content-Type": "application/json", "User-Agent": "uvd-x402-facilitator"})
    with urllib.request.urlopen(req, timeout=20) as response:
        answer = json.load(response)
    if "result" not in answer:
        raise RuntimeError(f"{method}: {answer.get('error')}")
    return answer["result"]


def measure(key: str, chain_id: int, url: str) -> dict:
    floor = FLOORS.get(key, DEFAULT_FLOOR)
    actual = int(rpc(url, "eth_chainId", []), 16)
    if actual != chain_id:
        raise RuntimeError(f"{url} answers chain {actual}, not {chain_id}")
    history = rpc(url, "eth_feeHistory", ["0x1", "latest", []])
    fees = [int(x, 16) for x in history.get("baseFeePerGas", [])]
    base = fees[-2] if len(fees) >= 2 else floor[2]
    try:
        priority = int(rpc(url, "eth_maxPriorityFeePerGas", []), 16)
    except Exception:  # noqa: BLE001 - the send path falls back the same way
        priority = None
    cap = fee_cap(base, priority, floor)
    return {"chain": key, "base_fee_wei": base, "priority_wei": priority, "fee_cap_wei": cap}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    out = parser.add_mutually_exclusive_group()
    out.add_argument("--hcl", action="store_true", help="print the evm_fee_cap_gwei map for alerts.tf")
    out.add_argument("--json", action="store_true", help="print JSON")
    args = parser.parse_args()

    budget, warn = readiness_constants()
    measured_at = time.strftime("%Y-%m-%dT%H:%MZ", time.gmtime())
    rows, failed = [], []
    for key, chain_id, url in CHAINS:
        try:
            row = measure(key, chain_id, url)
        except Exception as error:  # noqa: BLE001 - one dead RPC must not hide the rest
            failed.append((key, type(error).__name__))
            continue
        try:
            row["fee_cap_gwei"] = round_up(row["fee_cap_wei"] / GWEI)
        except ValueError as error:
            failed.append((key, str(error)))
            continue
        per_settle = Decimal(budget) * Decimal(row["fee_cap_wei"]) / Decimal(10**18)
        row["per_settle_native"] = float(per_settle)
        row["floor_native"] = float(per_settle * warn)
        rows.append(row)
        time.sleep(0.3)

    if args.json:
        print(json.dumps({"measured_at": measured_at, "settle_gas_budget": budget,
                          "warn_settles": warn, "chains": rows,
                          "unreadable": [k for k, _ in failed]}, indent=2))
    elif args.hcl:
        try:
            print(render_hcl(rows, measured_at, [k for k, _ in failed]))
        except ValueError as error:
            print(f"[FAIL] {error}", file=sys.stderr)
            return 1
    else:
        print(f"measured {measured_at}; SETTLE_GAS_BUDGET={budget}, warnSettles={warn}")
        print(f"{'chain':<20} {'base gwei':>12} {'fee cap gwei':>13} {'per settle':>14} {'floor':>12}")
        for r in rows:
            print(f"{r['chain']:<20} {r['base_fee_wei'] / GWEI:>12.6g} {r['fee_cap_gwei']:>13g} "
                  f"{r['per_settle_native']:>14.6g} {r['floor_native']:>12.6g}")
    for key, reason in failed:
        print(f"[WARN] {key}: not measured ({reason})", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
