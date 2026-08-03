#!/usr/bin/env python3
"""Fold lowercase asset rows into their checksummed twin.

An EVM address has one canonical spelling (EIP-55 checksum) and DynamoDB sort
keys are byte-compared, so `base#0x833589fCD6…` and `base#0x833589fcd6…` are two
different rows for the same token.

recover_missing_volume.py decoded the token address out of a receipt log, where
it arrives lowercase, and wrote it straight into the sort key. Every figure it
recovered therefore landed in a NEW row instead of the existing one: the real
row kept its settle count with no recovered volume, and an orphan appeared
carrying volume with zero settles.

The totals still added up, which is exactly why it went unnoticed — summing all
rows gave the right answer while every individual row was wrong. Anyone reading
the table row by row, which is what /stats shows, saw a split that has no
meaning on-chain.

Dry-run by default.
"""

import argparse
import json
import subprocess
import sys


def ddb(args, table):
    r = subprocess.run(["aws", "dynamodb", *args, "--table-name", table,
                        "--region", "us-east-2"],
                       capture_output=True, text=True, timeout=90)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or "").strip().splitlines()[-1][:150])
    return json.loads(r.stdout) if r.stdout.strip() else {}


def num(item, key):
    v = item.get(key, {}).get("N")
    return int(v) if v not in (None, "") else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--table", default="facilitator_transactions")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    items = ddb(["query", "--key-condition-expression", "pk = :p",
                 "--expression-attribute-values",
                 json.dumps({":p": {"S": "AGG"}})], args.table).get("Items", [])

    # Group by the case-insensitive key. Where a group has more than one member,
    # the canonical row is the one NOT written entirely in lowercase — that is
    # the checksummed spelling the live path uses.
    groups = {}
    for it in items:
        sk = it.get("sk", {}).get("S")
        if not sk:
            continue
        groups.setdefault(sk.lower(), []).append(it)

    plan = []
    for key, rows in groups.items():
        if len(rows) < 2:
            continue
        canonical = next((r for r in rows if r["sk"]["S"] != r["sk"]["S"].lower()), None)
        if canonical is None:
            continue  # all lowercase: nothing to fold into, leave alone
        for dup in rows:
            if dup["sk"]["S"] == canonical["sk"]["S"]:
                continue
            plan.append((dup["sk"]["S"], canonical["sk"]["S"], {
                "settles_ok": num(dup, "settles_ok"),
                "settles_failed": num(dup, "settles_failed"),
                "verifies": num(dup, "verifies"),
                "volume_atomic": num(dup, "volume_atomic"),
                "last_ts": num(dup, "last_ts"),
            }))

    if not plan:
        print("  no hay filas duplicadas por mayusculas")
        return 0

    print("  duplicado -> canonica")
    for dup, canon, v in plan:
        print(f"    {dup[:46]:<48} -> {canon[:24]}…")
        print(f"      aporta: settles={v['settles_ok']} volumen={v['volume_atomic']}")

    if not args.apply:
        print("\n  DRY-RUN: no se escribio nada.")
        return 0

    ok = 0
    for dup, canon, v in plan:
        ddb(["update-item",
             "--key", json.dumps({"pk": {"S": "AGG"}, "sk": {"S": canon}}),
             "--update-expression",
             "ADD settles_ok :s, settles_failed :f, verifies :vr, volume_atomic :vol",
             "--expression-attribute-values", json.dumps({
                 ":s": {"N": str(v["settles_ok"])},
                 ":f": {"N": str(v["settles_failed"])},
                 ":vr": {"N": str(v["verifies"])},
                 ":vol": {"N": str(v["volume_atomic"])}})], args.table)
        ddb(["delete-item",
             "--key", json.dumps({"pk": {"S": "AGG"}, "sk": {"S": dup}})], args.table)
        print(f"    OK {dup[:40]}… fusionada y borrada")
        ok += 1
    print(f"\n  fusionadas: {ok}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
