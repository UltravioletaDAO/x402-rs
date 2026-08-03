#!/usr/bin/env python3
"""Fold the `asset=unknown` aggregate rows into their real asset row.

These rows are debris from a bug fixed in v1.66.0: the field extractor did not
look inside a bare `payload`, so escrow settles were recorded without an asset.
They still counted the operation, so every affected network shows up TWICE on
/stats — once properly, and once as a ghost reading "N settles, 0 (atomic)".

That zero is the worst part. It is not a measurement; it is a field that was
never read. Shown next to real figures it invites the reader to believe those
settles moved nothing.

Deleting the rows outright would lose the settle COUNT, which is real. So each
one is merged into the network's actual asset row — counts added, ghost removed.
The volume was already recovered separately by recover_missing_volume.py, which
is why only counts move here.

Dry-run by default.
"""

import argparse
import json
import subprocess
import sys

# The asset each network's ghost row belongs to, taken from the aggregate itself
# where a real row exists. skale-base never produced one — every settle there
# landed in the ghost — so its USDC address comes from src/network.rs
# (USDC_SKALE_BASE, "Bridged USDC (SKALE Bridge)").
FALLBACK_ASSET = {
    "skale-base": "0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20",
}


def ddb(args, table):
    r = subprocess.run(["aws", "dynamodb", *args, "--table-name", table,
                        "--region", "us-east-2"],
                       capture_output=True, text=True, timeout=90)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or "").strip().splitlines()[-1][:150])
    return json.loads(r.stdout) if r.stdout.strip() else {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--table", default="facilitator_transactions")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    items = ddb(["query", "--key-condition-expression", "pk = :p",
                 "--expression-attribute-values",
                 json.dumps({":p": {"S": "AGG"}})], args.table).get("Items", [])

    real, ghosts = {}, []
    for it in items:
        net = it.get("network", {}).get("S")
        asset = it.get("asset", {}).get("S")
        if not net or not asset:
            continue
        if asset == "unknown":
            ghosts.append(it)
        else:
            # Prefer the row with the most settles when a network has several
            # assets: that is the one these settles almost certainly belong to.
            cur = real.get(net)
            n = int(it.get("settles_ok", {}).get("N", 0))
            if not cur or n > cur[1]:
                real[net] = (asset, n)

    if not ghosts:
        print("  no hay filas fantasma que fusionar")
        return 0

    plan = []
    for g in ghosts:
        net = g["network"]["S"]
        target = real.get(net, (FALLBACK_ASSET.get(net), 0))[0]
        plan.append({
            "network": net,
            "settles": int(g.get("settles_ok", {}).get("N", 0)),
            "verifies": int(g.get("verifies", {}).get("N", 0)),
            "failed": int(g.get("settles_failed", {}).get("N", 0)),
            "target": target,
        })

    print("  fila fantasma -> destino")
    for p in plan:
        dest = p["target"] or "SIN DESTINO (se conservara)"
        print(f"    {p['network']:<12} {p['settles']:>3} settles, {p['verifies']:>2} verifies "
              f"-> {str(dest)[:22]}")

    if not args.apply:
        print("\n  DRY-RUN: no se escribio nada.")
        return 0

    moved = kept = 0
    for p in plan:
        if not p["target"]:
            # Never delete a row whose counts have nowhere to go: that would
            # silently shrink the totals.
            print(f"    CONSERVADA {p['network']} (sin fila destino conocida)")
            kept += 1
            continue
        ddb(["update-item",
             "--key", json.dumps({"pk": {"S": "AGG"},
                                  "sk": {"S": f"{p['network']}#{p['target']}"}}),
             "--update-expression",
             "ADD settles_ok :n, verifies :v, settles_failed :f",
             "--expression-attribute-values",
             json.dumps({":n": {"N": str(p["settles"])},
                         ":v": {"N": str(p["verifies"])},
                         ":f": {"N": str(p["failed"])}})], args.table)
        ddb(["delete-item",
             "--key", json.dumps({"pk": {"S": "AGG"},
                                  "sk": {"S": f"{p['network']}#unknown"}})], args.table)
        print(f"    OK {p['network']}: +{p['settles']} settles a {p['target'][:16]}…, fantasma borrada")
        moved += 1

    print(f"\n  fusionadas: {moved} | conservadas: {kept}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
