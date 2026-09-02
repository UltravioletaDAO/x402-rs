#!/usr/bin/env python3
"""Report what the live ERC-8004 identity registries can actually answer.

Why this exists
---------------
On 2026-08-29 the owner lookup was rewritten to read the registry's agent count
from ``totalSupply()``, with the old sequential probe kept as a fallback. The
reasoning was that ``totalSupply()`` "is already in the ABI and
``/identity/{network}/total-supply`` already uses it". Both halves were true
about the code and false about the chain: the deployed registries do not
implement ``ERC721Enumerable``, ``totalSupply()`` reverts on every one of them,
and that endpoint had been answering ``501`` on every network for months.

So the fast path never ran once. The "fallback" was the only path, the change
was a no-op in production, and the facilitator's p99 sat at 11.4s until someone
read the alert emails. 1,264 green tests never touched it, because a test cannot
tell you what a contract on mainnet does.

This script can. Run it before making any registry capability load-bearing.

Usage
-----
    python scripts/erc8004_registry_capabilities.py
    python scripts/erc8004_registry_capabilities.py --json
    python scripts/erc8004_registry_capabilities.py --network celo --network base

Read-only: every call is ``eth_call`` against a public endpoint. No keys, no
writes, no facilitator involved.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request

# The ERC-8004 registries are deployed at the same deterministic address on
# every chain. Golden source: src/erc8004/mod.rs (IDENTITY_REGISTRY_ADDRESS).
IDENTITY_REGISTRY = "0x8004A169FB4a3325136EB29fA0ceB6D2e539a432"

# Public endpoints, on purpose: this script must be runnable by anyone, with no
# access to the production RPC secrets. Timings here are therefore indicative of
# shape, not of production latency.
PUBLIC_RPCS: dict[str, str] = {
    "base": "https://mainnet.base.org",
    "celo": "https://forno.celo.org",
    "ethereum": "https://ethereum-rpc.publicnode.com",
    "arbitrum": "https://arb1.arbitrum.io/rpc",
    "optimism": "https://mainnet.optimism.io",
    "polygon": "https://polygon-rpc.com",
    "avalanche": "https://api.avax.network/ext/bc/C/rpc",
    "monad": "https://rpc.monad.xyz",
    "skale": "https://mainnet.skalenodes.com/v1/elated-tan-skat",
}

# Function selectors.
SEL_TOTAL_SUPPLY = "0x18160ddd"                      # totalSupply()
SEL_OWNER_OF = "0x6352211e"                          # ownerOf(uint256)
SEL_SUPPORTS_INTERFACE = "0x01ffc9a7"                # supportsInterface(bytes4)
ERC721_ENUMERABLE_ID = "780e9d63"                    # ERC721Enumerable
ERC721_ID = "80ac58cd"                               # ERC721

# Highest agent ID the ladder search can bracket, mirroring
# BOUND_LADDER_MAX_EXP in src/handlers.rs.
LADDER_MAX_EXP = 24


class RpcError(Exception):
    pass


def rpc_call(url: str, to: str, data: str, timeout: float = 20.0) -> tuple[str | None, str | None]:
    """Return ``(result, revert_reason)``. Exactly one is not None.

    A revert is an ANSWER (the call executed and the contract refused); a
    transport or node failure is not, and raises. Collapsing the two is how a
    rate limit gets recorded as "this function does not exist".
    """
    payload = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{"to": to, "data": data}, "latest"],
        }
    ).encode()
    req = urllib.request.Request(
        url,
        data=payload,
        headers={
            "content-type": "application/json",
            # Several public nodes answer 403 to a request with no User-Agent.
            "user-agent": "x402-rs/erc8004-registry-capabilities",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError) as exc:
        raise RpcError(str(exc)) from exc

    if "error" in body:
        message = str(body["error"].get("message", ""))
        lowered = message.lower()
        if "revert" in lowered or "invalid opcode" in lowered:
            return None, message
        raise RpcError(message)
    return body.get("result"), None


def token_exists(url: str, agent_id: int) -> bool:
    data = SEL_OWNER_OF + f"{agent_id:064x}"
    result, revert = rpc_call(url, IDENTITY_REGISTRY, data)
    if revert is not None:
        return False
    return bool(result) and result != "0x"


def highest_agent_id(url: str) -> tuple[int | None, int]:
    """Bracket the highest agent ID. Returns ``(max_id, eth_calls_spent)``.

    Deliberately the SEQUENTIAL algorithm the facilitator no longer uses, so the
    call count printed here is the cost the facilitator avoids by batching the
    same probes through Multicall3.
    """
    calls = 0
    hi = 1
    while True:
        calls += 1
        if not token_exists(url, hi):
            break
        if hi >= 1 << LADDER_MAX_EXP:
            return None, calls
        hi *= 2
    lo = hi // 2
    if lo == 0:
        return 0, calls
    while lo < hi - 1:
        calls += 1
        mid = lo + (hi - lo) // 2
        if token_exists(url, mid):
            lo = mid
        else:
            hi = mid
    return lo, calls


def inspect(network: str, url: str) -> dict:
    row: dict = {"network": network, "rpc": url}

    try:
        result, revert = rpc_call(url, IDENTITY_REGISTRY, SEL_TOTAL_SUPPLY)
        if revert is not None:
            row["totalSupply"] = None
            row["totalSupplyStatus"] = "reverts"
        else:
            row["totalSupply"] = int(result, 16) if result and result != "0x" else None
            row["totalSupplyStatus"] = "ok"
    except RpcError as exc:
        row["totalSupplyStatus"] = f"rpc-error: {exc}"

    for label, iface in (("erc721", ERC721_ID), ("erc721Enumerable", ERC721_ENUMERABLE_ID)):
        try:
            result, revert = rpc_call(
                url, IDENTITY_REGISTRY, SEL_SUPPORTS_INTERFACE + iface + "0" * 56
            )
            if revert is not None:
                row[label] = "reverts"
            else:
                row[label] = bool(int(result, 16)) if result and result != "0x" else False
        except RpcError as exc:
            row[label] = f"rpc-error: {exc}"

    started = time.monotonic()
    try:
        max_id, calls = highest_agent_id(url)
        row["highestAgentId"] = max_id
        row["sequentialCalls"] = calls
        row["sequentialSeconds"] = round(time.monotonic() - started, 2)
        if calls:
            row["secondsPerCall"] = round(row["sequentialSeconds"] / calls, 3)
    except RpcError as exc:
        row["highestAgentId"] = None
        row["probeStatus"] = f"rpc-error: {exc}"

    return row


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument(
        "--network",
        action="append",
        choices=sorted(PUBLIC_RPCS),
        help="limit to these networks (repeatable); default is all",
    )
    args = parser.parse_args()

    targets = args.network or sorted(PUBLIC_RPCS)
    rows = [inspect(n, PUBLIC_RPCS[n]) for n in targets]

    if args.json:
        print(json.dumps(rows, indent=2))
        return 0

    print(f"ERC-8004 identity registry {IDENTITY_REGISTRY}\n")
    header = f"{'network':<11} {'totalSupply':<14} {'Enumerable':<11} {'highest id':>11} {'serial calls':>13} {'s/call':>8}"
    print(header)
    print("-" * len(header))
    for r in rows:
        supply = r.get("totalSupplyStatus", "?")
        if supply == "ok":
            supply = str(r.get("totalSupply"))
        print(
            f"{r['network']:<11} {supply[:14]:<14} {str(r.get('erc721Enumerable')):<11} "
            f"{str(r.get('highestAgentId')):>11} {str(r.get('sequentialCalls', '-')):>13} "
            f"{str(r.get('secondsPerCall', '-')):>8}"
        )

    reverting = [r["network"] for r in rows if r.get("totalSupplyStatus") == "reverts"]
    answered = [r["network"] for r in rows if r.get("totalSupplyStatus") == "ok"]
    unreachable = [
        r["network"]
        for r in rows
        if r.get("totalSupplyStatus", "").startswith("rpc-error")
    ]

    print()
    if unreachable:
        # An unreachable node is NO VERDICT. Reporting it as a capability -- in
        # either direction -- is the exact mistake this script exists to catch,
        # so it is called out first and it sets the exit code.
        print(
            f"NO VERDICT on {len(unreachable)}/{len(rows)} networks "
            f"({', '.join(unreachable)}): the endpoint did not answer, which says nothing\n"
            "about what the contract implements. Retry or use a different RPC before\n"
            "concluding anything from this run."
        )
        print()
    if reverting:
        print(
            f"totalSupply() REVERTS on {len(reverting)}/{len(rows)} networks: "
            f"{', '.join(reverting)}."
        )
        print(
            "Nothing in the facilitator may depend on it. The owner lookup derives its\n"
            "bound by probing ownerOf through Multicall3 instead -- see\n"
            "discover_max_agent_id in src/handlers.rs."
        )
    if answered:
        print(
            f"totalSupply() ANSWERED on: {', '.join(answered)}. That is new; re-read\n"
            "src/handlers.rs before relying on it, and confirm it on EVERY network --\n"
            "the last time this was assumed from one place it held the p99 at 11.4s."
        )
    if not reverting and not answered:
        print("No network reached a verdict. Nothing can be concluded from this run.")
    print()
    print(
        "'serial calls' is what the OLD sequential probe spent per cold lookup. The\n"
        "facilitator now batches the same probes through Multicall3 in at most 4 round\n"
        "trips, and caches the bound per (network, registry)."
    )
    # Non-zero when any network failed to answer, so CI or a shell loop cannot
    # mistake an unreachable node for a clean result.
    return 1 if unreachable else 0


if __name__ == "__main__":
    sys.exit(main())
