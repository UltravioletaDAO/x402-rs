#!/usr/bin/env python3
"""
Retrospective on-chain scan of the facilitator's EVM wallets.

Answers the question the transaction store cannot: **what did we settle before
we started recording?** The store begins at zero the day it is switched on;
this reads history back off the chains themselves.

WHY A SELECTOR FILTER, AND NOT A TRANSACTION COUNT
--------------------------------------------------
The facilitator wallet does far more than settle x402 payments. It tops up gas,
mints ERC-8004 identities, writes reputation feedback, releases and refunds
escrow. Counting its transactions and calling the result "payments processed"
overcounts, and does so in a way that looks authoritative because the number is
real — it just answers a different question.

So this filters on the method selector. `transferWithAuthorization` is
`0xe3ee160e`: the EIP-3009 call an x402 exact settlement actually makes. That
selector was not taken from a spec — it was read off a real settlement on Base
on 2026-07-29 and confirmed against the receipt.

WHAT THIS DELIBERATELY DOES NOT COUNT
-------------------------------------
ERC-8004 writes, gas top-ups, escrow operations and refunds. They are real
activity and they are not payments. A separate scan can count them; merging them
into one total is how "how many payments have we processed" gets a wrong answer.

DESIGN: MECHANICAL ON PURPOSE
-----------------------------
Everything is driven by ``config/supported_tokens.json``, the canonical source.
No chain list lives here, so adding a network to the facilitator adds it to the
scan for free. The output is JSON on disk. This exists so a cheap model — or
cron — can run it without reasoning about anything.

USAGE
    python scripts/scan/scan_evm.py                    # every EVM mainnet
    python scripts/scan/scan_evm.py --network base     # one
    python scripts/scan/scan_evm.py --testnets         # testnets instead
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time
import urllib.error
import urllib.request

REPO = pathlib.Path(__file__).resolve().parents[2]
CONFIG = REPO / "config" / "supported_tokens.json"
OUT_DIR = REPO / "docs" / "reports" / "onchain-scan"

#: EIP-3009 `transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,uint8,bytes32,bytes32)`.
#: Read off a real Base settlement (0x5e9d2e9c…) on 2026-07-29, not copied from a doc.
SELECTOR_TRANSFER_WITH_AUTHORIZATION = "0xe3ee160e"

#: Selectors seen from the facilitator wallet that are NOT payments. Listed so
#: the report can say what it skipped instead of silently dropping it — an
#: unexplained gap between "transactions" and "payments" invites someone to
#: assume the scan is broken.
NON_PAYMENT_SELECTORS = {
    "0x40c10f19": "mint (ERC-8004 identity)",
    "0xa9059cbb": "transfer (gas top-up / sweep)",
    "0x095ea7b3": "approve",
}

#: Etherscan's unified V2 endpoint. One host, one key, chain selected by
#: `chainid`. The per-chain V1 hosts (api.basescan.org and friends) are DEAD —
#: verified 2026-07-30, they answer
#: "You are using a deprecated V1 endpoint, switch to Etherscan API V2".
ETHERSCAN_V2 = "https://api.etherscan.io/v2/api"

#: Blockscout instances, used when Etherscan V2 refuses a chain on the free
#: plan. Free and keyless, but coverage is uneven — verified 2026-07-30,
#: eth.blockscout.com answers 200 while base.blockscout.com returns 500. Only
#: hosts confirmed working are listed; a guess here becomes a silent zero.
BLOCKSCOUT = {
    1: "https://eth.blockscout.com",
}


def load_networks(testnets: bool) -> dict:
    """Networks straight from the canonical config.

    Read rather than hardcoded so this cannot drift from the facilitator the way
    the old stats skill did — it still referenced a SKALE explorer domain that
    had stopped resolving.
    """
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    return config["evm_testnets" if testnets else "evm_mainnets"]


def fetch(url: str, attempts: int = 3) -> dict | None:
    """GET JSON, retrying transient failures.

    Returns None when the explorer cannot be reached. The caller MUST treat that
    as unknown, never as zero — a rate-limited scan that reports 0 payments is
    the exact failure this whole file is written to avoid.
    """
    # A bare urllib request has no User-Agent and several explorers answer it
    # with 403 — verified against Blockscout on 2026-07-30. That failure looks
    # exactly like "unreachable", which would have been filed as a scan gap
    # rather than a two-line fix.
    request = urllib.request.Request(url, headers={"User-Agent": "x402-facilitator-scan/1.0"})
    for attempt in range(attempts):
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.loads(response.read().decode())
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as exc:
            if attempt == attempts - 1:
                print(f"    ! unreachable after {attempts} attempts: {exc}", file=sys.stderr)
                return None
            time.sleep(2 ** attempt)
    return None


def scan_network(name: str, meta: dict, api_key: str) -> dict:
    """Count payments from one chain, classifying everything else."""
    chain_id = meta.get("chainId")
    wallet = meta.get("facilitatorWallet")
    result = {
        "network": name,
        "chainId": chain_id,
        "wallet": wallet,
        "scanned": False,
        "reason": None,
        "payments": 0,
        "other": {},
        "unknownSelectors": {},
        "firstTs": None,
        "lastTs": None,
    }

    if not wallet:
        result["reason"] = "no facilitator wallet in config"
        return result
    if not api_key and chain_id not in BLOCKSCOUT:
        # Said out loud rather than returning an empty scan: Etherscan V2
        # requires a key even for chain 1.
        result["reason"] = "ETHERSCAN_API_KEY required for this chain"
        return result

    print(f"  {name} (chain {chain_id})…")
    result["via"] = "blockscout" if not api_key else "etherscan-v2"
    page, txs = 1, []
    while True:
        if api_key:
            url = (
                f"{ETHERSCAN_V2}?chainid={chain_id}&module=account&action=txlist"
                f"&address={wallet}&startblock=0&endblock=99999999"
                f"&page={page}&offset=1000&sort=asc&apikey={api_key}"
            )
        else:
            url = (
                f"{BLOCKSCOUT[chain_id]}/api?module=account&action=txlist"
                f"&address={wallet}&page={page}&offset=1000&sort=asc"
            )
        data = fetch(url)
        if data is None:
            result["reason"] = "explorer unreachable; counts would be a floor, not a total"
            return result
        batch = data.get("result")
        if not isinstance(batch, list):
            # "No transactions found" is a legitimate empty answer; anything else
            # is an error we must not silently read as empty.
            message = str(data.get("result") or data.get("message") or "")
            if "No transactions found" not in message:
                # Distinguish "you cannot afford this chain" from "this chain is
                # broken" — the first is a decision for the operator, the second
                # is a bug to chase.
                if "upgrade your api plan" in message.lower():
                    result["reason"] = "chain not covered by the current Etherscan plan"
                else:
                    result["reason"] = f"explorer said: {message or data}"
                return result
            batch = []
        txs.extend(batch)
        if len(batch) < 1000:
            break
        page += 1
        time.sleep(0.25)  # courtesy to free-tier explorers

    for tx in txs:
        selector = (tx.get("input") or "")[:10].lower()
        ts = int(tx.get("timeStamp", 0))
        if selector == SELECTOR_TRANSFER_WITH_AUTHORIZATION:
            result["payments"] += 1
            result["firstTs"] = min(result["firstTs"] or ts, ts)
            result["lastTs"] = max(result["lastTs"] or ts, ts)
        elif selector in NON_PAYMENT_SELECTORS:
            label = NON_PAYMENT_SELECTORS[selector]
            result["other"][label] = result["other"].get(label, 0) + 1
        elif selector and selector != "0x":
            result["unknownSelectors"][selector] = (
                result["unknownSelectors"].get(selector, 0) + 1
            )

    result["scanned"] = True
    result["totalTxs"] = len(txs)
    print(f"    payments={result['payments']}  otras={sum(result['other'].values())}"
          f"  total={len(txs)}")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--network", help="scan only this network")
    parser.add_argument("--testnets", action="store_true", help="scan testnets")
    args = parser.parse_args()

    # One key covers the whole Etherscan V2 family. Without it the scan still
    # runs, just slower and rate-limited.
    api_key = os.environ.get("ETHERSCAN_API_KEY", "")
    if not api_key:
        print(
            "! ETHERSCAN_API_KEY unset. Etherscan V2 requires a key on every chain,\n"
            "  so only chains with a working Blockscout fallback will be scanned.\n"
            "  Everything else comes back UNSCANNED, never as zero.",
            file=sys.stderr,
        )

    networks = load_networks(args.testnets)
    if args.network:
        if args.network not in networks:
            print(f"unknown network: {args.network}", file=sys.stderr)
            return 2
        networks = {args.network: networks[args.network]}

    print(f"scanning {len(networks)} EVM network(s) for {SELECTOR_TRANSFER_WITH_AUTHORIZATION}")
    results = [scan_network(name, meta, api_key) for name, meta in networks.items()]

    scanned = [r for r in results if r["scanned"]]
    skipped = [r for r in results if not r["scanned"]]
    report = {
        "generatedAt": int(time.time()),
        "source": "on-chain scan of facilitator wallets",
        "selector": SELECTOR_TRANSFER_WITH_AUTHORIZATION,
        "method": "transferWithAuthorization (EIP-3009)",
        "caveat": (
            "Counts x402 exact settlements only. ERC-8004 writes, gas top-ups and "
            "escrow operations are real wallet activity and are NOT payments; they "
            "are reported separately under 'other'. Networks under 'skipped' are "
            "UNSCANNED, which is not the same as zero."
        ),
        "totalPayments": sum(r["payments"] for r in scanned),
        "networksScanned": len(scanned),
        "networksSkipped": len(skipped),
        "results": results,
    }

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = OUT_DIR / ("evm-testnets.json" if args.testnets else "evm-mainnets.json")
    out.write_text(json.dumps(report, indent=2), encoding="utf-8")

    print(f"\ntotal pagos x402: {report['totalPayments']}")
    print(f"redes escaneadas: {len(scanned)}  ·  sin escanear: {len(skipped)}")
    for r in skipped:
        print(f"  - {r['network']}: {r['reason']}")
    print(f"\n-> {out.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
