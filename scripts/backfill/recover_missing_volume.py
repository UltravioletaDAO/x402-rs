#!/usr/bin/env python3
"""Recover the volume of settles that were recorded without an asset.

Why this exists
---------------
Until v1.66.0 the extractor did not look inside a bare `payload`, so escrow
settles landed in the index with `asset` and `amount` null. They still counted
as operations, which split each network into a second row whose volume read as
zero — a number that looked measured and was an artifact.

The rows are NOT lost. Every one of them carries the on-chain transaction hash,
and the hash is the stronger source anyway: it records what actually moved, not
what the request asked for. Recovering them needs an ordinary `eth_getTransaction
Receipt` against a public RPC — NOT the explorer API key that blocks the general
historical backfill. Those are different problems and only one of them is stuck.

Safety
------
Dry-run by default. It prints the diff it would apply and writes nothing. Pass
--apply to write, and even then it only ADDS volume to rows that currently read
zero — it never overwrites a figure that was already measured.
"""

import argparse
import json
import os
import sys
import urllib.request

TRANSFER_TOPIC = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"

# RPC endpoints, read-only.
#
# Public endpoints answered 403 for most receipts on the first run: 27 of 37
# rows came back "unreadable" and it looked like the data was gone. It was not
# — the endpoint was refusing us. That is why this now prefers the facilitator's
# own premium RPCs from AWS Secrets Manager (`facilitator-rpc-mainnet`), the
# same ones the service settles with.
#
# The URLs carry API keys. They are read into memory and never printed, never
# written to a file, and never included in an error message — `rpc_call` reports
# the network name, not the endpoint. `src/redact.rs` exists because an RPC URL
# with its key leaked into a log once already.
PUBLIC_FALLBACK = {
    "base": "https://mainnet.base.org",
    "arbitrum": "https://arb1.arbitrum.io/rpc",
    "optimism": "https://mainnet.optimism.io",
    "polygon": "https://polygon-rpc.com",
    "avalanche": "https://api.avax.network/ext/bc/C/rpc",
    "ethereum": "https://eth.llamarpc.com",
}


def load_rpcs():
    """Premium endpoints from Secrets Manager, falling back to public ones.

    A network with no endpoint at all is reported as such, never guessed at.
    """
    rpcs = dict(PUBLIC_FALLBACK)
    try:
        import subprocess
        out = subprocess.run(
            ["aws", "secretsmanager", "get-secret-value",
             "--secret-id", "facilitator-rpc-mainnet",
             "--region", "us-east-2", "--query", "SecretString", "--output", "text"],
            capture_output=True, text=True, timeout=60,
        )
        if out.returncode == 0:
            secret = json.loads(out.stdout)
            for net, url in secret.items():
                if isinstance(url, str) and url.startswith("http"):
                    rpcs[net] = url
            print(f"RPC premium cargados para: {sorted(secret.keys())}")
        else:
            print("AVISO: no se pudo leer el secreto; se usan endpoints publicos",
                  file=sys.stderr)
    except Exception as exc:  # noqa: BLE001
        print(f"AVISO: Secrets Manager no disponible ({type(exc).__name__}); "
              f"se usan endpoints publicos", file=sys.stderr)
    return rpcs


def rpc_call(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    req = urllib.request.Request(
        url, data=body.encode(), headers={"content-type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=25) as r:
        return json.load(r).get("result")


def decode_transfer(receipt):
    """Return (token, value) from the first ERC-20 Transfer in the receipt.

    Returns None rather than a guess when the receipt has no Transfer log: a
    settle whose value cannot be read must stay unknown, not become zero.
    """
    if not receipt or receipt.get("status") != "0x1":
        return None
    for log in receipt.get("logs", []):
        topics = log.get("topics", [])
        if len(topics) == 3 and topics[0].lower() == TRANSFER_TOPIC:
            return log["address"].lower(), int(log["data"], 16)
    return None


def main():
    RPC = load_rpcs()
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default="https://facilitator.ultravioletadao.xyz")
    ap.add_argument("--limit", type=int, default=200)
    ap.add_argument("--apply", action="store_true", help="write; default is dry-run")
    args = ap.parse_args()

    with urllib.request.urlopen(f"{args.base_url}/transactions?limit={args.limit}", timeout=30) as r:
        data = json.load(r)
    rows = data if isinstance(data, list) else data.get("transactions", data.get("items", []))

    missing = [
        x for x in rows
        if x.get("kind") == "settle" and x.get("ok") and not x.get("asset") and x.get("tx")
    ]
    print(f"filas de settle sin asset y con hash: {len(missing)}")
    if not missing:
        return 0

    recovered, unreadable, no_rpc = [], [], []
    for row in missing:
        net, tx = row.get("network"), row["tx"]
        url = RPC.get(net)
        if not url:
            no_rpc.append((net, tx))
            continue
        try:
            decoded = decode_transfer(rpc_call(url, "eth_getTransactionReceipt", [tx]))
        except Exception as exc:  # noqa: BLE001 - reported, never silently dropped
            print(f"  [RPC ERROR] {net} {tx[:14]}: {exc}", file=sys.stderr)
            unreadable.append((net, tx))
            continue
        if decoded is None:
            unreadable.append((net, tx))
            continue
        token, value = decoded
        recovered.append({"network": net, "tx": tx, "asset": token, "amount": value})

    print(f"\nrecuperables : {len(recovered)}")
    print(f"ilegibles    : {len(unreadable)}  (sin log Transfer o receipt no disponible)")
    print(f"sin RPC      : {len(no_rpc)}  (redes que este script no cubre)")

    by_net = {}
    for r in recovered:
        k = (r["network"], r["asset"])
        agg = by_net.setdefault(k, {"n": 0, "vol": 0})
        agg["n"] += 1
        agg["vol"] += r["amount"]

    print("\ndiff que se aplicaria (red, asset, filas, volumen a sumar):")
    for (net, asset), agg in sorted(by_net.items()):
        print(f"  {net:<12} {asset[:14]}…  {agg['n']:>3} filas  +{agg['vol']} atomicos")

    if not args.apply:
        print("\nDRY-RUN: no se escribio nada. Pasar --apply para aplicar.")
        return 0

    table = os.environ.get("TRANSACTIONS_TABLE_NAME", "facilitator_transactions")
    import subprocess

    print(f"\n[apply] escribiendo en {table}")
    ok = failed = 0
    for (net, asset), agg in sorted(by_net.items()):
        key = json.dumps({"pk": {"S": "AGG"}, "sk": {"S": f"{net}#{asset}"}})
        # ADD, not SET: this only increments. And the condition means a row whose
        # volume was already measured is left alone — a re-run cannot double-count,
        # and a figure that came from the live path is never overwritten by one
        # reconstructed here.
        res = subprocess.run(
            ["aws", "dynamodb", "update-item",
             "--table-name", table, "--region", "us-east-2",
             "--key", key,
             "--update-expression", "ADD volume_atomic :v",
             "--condition-expression", "attribute_not_exists(volume_atomic) OR volume_atomic = :zero",
             "--expression-attribute-values",
             json.dumps({":v": {"N": str(agg["vol"])}, ":zero": {"N": "0"}})],
            capture_output=True, text=True, timeout=60,
        )
        if res.returncode == 0:
            print(f"  OK   {net:<12} +{agg['vol']}")
            ok += 1
        else:
            err = res.stderr.strip().splitlines()[-1] if res.stderr else "?"
            # A failed condition is the expected, correct outcome for a row that
            # already carries a measured volume. It is not an error.
            note = "(ya tenia volumen medido, se respeta)" if "ConditionalCheckFailed" in err else err[:80]
            print(f"  SKIP {net:<12} {note}")
            failed += 1
    print(f"\nfilas actualizadas: {ok} | omitidas: {failed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
