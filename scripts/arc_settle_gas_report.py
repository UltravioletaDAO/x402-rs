#!/usr/bin/env python3
"""Regenerate docs/reports/arc-settle-gas.json: what each recorded Arc settle cost in gas.

The per-settle cost used to be copied into prose (CHANGELOG, docs/networks) as it
stood on the day of the canary. Arc's base fee is not constant -- it sat at its
20 gwei minimum for days and read 81.64 gwei at block 21,205,139 on 2026-09-16 -- so a
copied figure goes stale without anything saying so. The docs now cite this
report instead, and this script rebuilds it from the chain.

Sources: every settle recorded in docs/reports/*.json and *.jsonl whose object
names an Arc network and carries a `transaction` hash (the canary logs and the
production acceptance record). The report points back at each source instead of
repeating the hash, and identifies the transaction by block and index as well.

Read-only: eth_chainId, eth_getTransactionReceipt, eth_getBlockByNumber against
Arc's public RPCs. No key, no transaction.

    python3 scripts/arc_settle_gas_report.py            # rewrite the report
    python3 scripts/arc_settle_gas_report.py --stdout   # print it instead
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import sys
import time
import urllib.request
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
REPORTS = REPO / "docs" / "reports"
OUTPUT = REPORTS / "arc-settle-gas.json"

# network -> (chain id, public RPC). Aliases map the CAIP-2 spelling onto it.
NETWORKS = {
    "arc": (5042, "https://rpc.mainnet.arc.io"),
    "arc-testnet": (5042002, "https://rpc.testnet.arc.io"),
}
ALIASES = {"eip155:5042": "arc", "eip155:5042002": "arc-testnet"}
TX_HASH = re.compile(r"^0x[0-9a-fA-F]{64}$")


def rpc(url: str, method: str, params: list) -> object:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=body, headers={
        "Content-Type": "application/json", "User-Agent": "uvd-x402-facilitator"})
    with urllib.request.urlopen(req, timeout=30) as response:
        answer = json.load(response)
    if "result" not in answer or answer["result"] is None:
        raise RuntimeError(f"{method} returned no result")
    return answer["result"]


def recorded_settles() -> list[dict]:
    """Every (source, network, transaction) the evidence files record, once each."""
    found: list[dict] = []
    seen: set[str] = set()

    def walk(node: object, where: str, source: str) -> None:
        if isinstance(node, dict):
            network = ALIASES.get(node.get("network"), node.get("network"))
            tx = node.get("transaction")
            if network in NETWORKS and isinstance(tx, str) and TX_HASH.match(tx) and tx.lower() not in seen:
                seen.add(tx.lower())
                found.append({"source": f"{source}{where}", "network": network, "tx": tx,
                              "x402_version": node.get("x402_version")})
            for key, value in node.items():
                walk(value, f"{where}.{key}", source)
        elif isinstance(node, list):
            for index, value in enumerate(node):
                walk(value, f"{where}[{index}]", source)

    for path in sorted(REPORTS.glob("*.json")) + sorted(REPORTS.glob("*.jsonl")):
        if path == OUTPUT:
            continue
        source = path.relative_to(REPO).as_posix()
        text = path.read_text(encoding="utf-8")
        try:
            if path.suffix == ".jsonl":
                for number, line in enumerate(text.splitlines(), start=1):
                    if line.strip():
                        walk(json.loads(line), f"#L{number}", source)
            else:
                walk(json.loads(text), "#$", source)
        except json.JSONDecodeError:
            print(f"[WARN] {source}: not JSON, skipped", file=sys.stderr)
    return found


def measure(settle: dict) -> dict:
    chain_id, url = NETWORKS[settle["network"]]
    receipt = rpc(url, "eth_getTransactionReceipt", [settle["tx"]])
    block = rpc(url, "eth_getBlockByNumber", [receipt["blockNumber"], False])
    gas_used = int(receipt["gasUsed"], 16)
    price = int(receipt["effectiveGasPrice"], 16)
    return {
        "source": settle["source"],
        "network": settle["network"],
        "x402_version": settle["x402_version"],
        "block": int(receipt["blockNumber"], 16),
        "transaction_index": int(receipt["transactionIndex"], 16),
        "block_time_utc": datetime.fromtimestamp(int(block["timestamp"], 16), timezone.utc)
        .strftime("%Y-%m-%dT%H:%M:%SZ"),
        "status": int(receipt["status"], 16),
        "gas_used": gas_used,
        "effective_gas_price_gwei": str(Decimal(price) / Decimal(10**9)),
        "base_fee_gwei": str(Decimal(int(block["baseFeePerGas"], 16)) / Decimal(10**9)),
        # Native USDC has 18 decimals on Arc: gas is paid in the same money.
        "gas_usdc": str(Decimal(gas_used * price) / Decimal(10**18)),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stdout", action="store_true", help="print the report instead of writing it")
    args = parser.parse_args()

    for network, (chain_id, url) in NETWORKS.items():
        if int(rpc(url, "eth_chainId", []), 16) != chain_id:
            sys.exit(f"{url} is not {network}")

    rows = []
    for settle in recorded_settles():
        rows.append(measure(settle))
        time.sleep(0.2)
    rows.sort(key=lambda r: (r["network"], r["block"], r["transaction_index"]))

    summary = {}
    for network in NETWORKS:
        costs = [Decimal(r["gas_usdc"]) for r in rows if r["network"] == network]
        if costs:
            summary[network] = {"settles": len(costs), "min_gas_usdc": str(min(costs)),
                                "median_gas_usdc": str(statistics.median(costs)),
                                "max_gas_usdc": str(max(costs))}

    latest = {}
    for network, (_, url) in NETWORKS.items():
        head = rpc(url, "eth_getBlockByNumber", ["latest", False])
        latest[network] = {"block": int(head["number"], 16),
                           "base_fee_gwei": str(Decimal(int(head["baseFeePerGas"], 16)) / Decimal(10**9))}

    report = {
        "generated_by": "scripts/arc_settle_gas_report.py",
        "generated_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "note": ("Gas each recorded Arc settle paid, read back from the chain. gas_usdc = gasUsed x "
                 "effectiveGasPrice / 10^18 (native USDC, 18 decimals). The price follows the base fee, "
                 "so a settle costs more when the base fee is above its 20 gwei minimum; "
                 "latest_base_fee is the reading at generation time."),
        "latest_base_fee": latest,
        "summary": summary,
        "settles": rows,
    }
    text = json.dumps(report, indent=2) + "\n"
    if args.stdout:
        sys.stdout.write(text)
    else:
        OUTPUT.write_text(text, encoding="utf-8")
        print(f"[OK] {OUTPUT.relative_to(REPO)}: {len(rows)} settles")
    return 0


if __name__ == "__main__":
    sys.exit(main())
