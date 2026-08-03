#!/usr/bin/env python3
"""Phase 2: reconstruct the ESCROW settlements the calldata decoder cannot see.

Why a second script
-------------------
`backfill_history.py` reads the amount straight out of `transferWithAuthorization`
calldata. Escrow payments do not work that way: the money moves in the receipt
logs, so each one needs its receipt fetched.

What the escrow lifecycle actually looks like (verified against real Base
transactions on 2026-08-03, not inferred from the ABI):

  0x41d66202  authorize  -> payer's funds move INTO escrow. Two Transfers of the
                            SAME amount (payer -> collector -> escrow). Counting
                            this as volume would both double-count and include
                            money that was later refunded.
  0xecf39b0a  capture    -> funds leave escrow: seller gets the principal, the
                            operator its fee (87000 + 13000 of a 100000
                            authorisation). THIS is the settlement.
  0xe2b8996f  refund     -> funds go back to the payer. Not volume.

So only captures are counted, and within a capture every Transfer is summed,
because principal + fee together are what the payer actually paid — the same
basis used for `exact` settles, where the full authorised value is counted.

Had this script been written from the selector list alone it would have counted
authorisations, produced a number roughly 2.5x too high, and looked entirely
plausible.
"""

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.parse
import urllib.request

SELECTOR_CAPTURE = "0xecf39b0a"
SELECTOR_REFUND = "0xe2b8996f"
SELECTOR_AUTHORIZE = "0x41d66202"
TRANSFER_TOPIC = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"

FACILITATOR_WALLET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
API = "https://api.etherscan.io/v2/api"
CHAINS = {
    "base": 8453, "avalanche": 43114, "arbitrum": 42161, "polygon": 137,
    "optimism": 10, "ethereum": 1, "celo": 42220, "monad": 143, "unichain": 130,
}


def api_key():
    if os.environ.get("ETHERSCAN_API_KEY"):
        return os.environ["ETHERSCAN_API_KEY"]
    out = subprocess.run(
        ["aws", "secretsmanager", "get-secret-value",
         "--secret-id", "facilitator-etherscan-api-key",
         "--region", "us-east-2", "--query", "SecretString", "--output", "text"],
        capture_output=True, text=True, timeout=60)
    if out.returncode != 0:
        sys.exit("ERROR: sin ETHERSCAN_API_KEY ni secreto accesible")
    return out.stdout.strip()


def call(params, key, delay):
    q = urllib.parse.urlencode({**params, "apikey": key})
    req = urllib.request.Request(f"{API}?{q}", headers={
        "User-Agent": "x402-facilitator-backfill/1.0", "Accept": "application/json"})
    for attempt in range(6):
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                body = json.load(r)
            time.sleep(delay)
            return body
        except urllib.error.HTTPError as e:
            if e.code not in (429, 502, 503, 504) or attempt == 5:
                raise
            time.sleep(delay * (2 ** attempt))
    return {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chains", default=",".join(CHAINS))
    ap.add_argument("--delay", type=float, default=0.25)
    ap.add_argument("--apply", action="store_true")
    ap.add_argument("--table", default=os.environ.get("TRANSACTIONS_TABLE_NAME",
                                                      "facilitator_transactions"))
    args = ap.parse_args()
    key = api_key()
    grand, calls = {}, 0

    for name in [c.strip() for c in args.chains.split(",") if c.strip()]:
        cid = CHAINS.get(name)
        if not cid:
            continue
        # The wallet nonce is the number of transactions it has ever sent, read
        # straight from the chain. It is the ground truth this enumeration is
        # checked against — and it is needed, because `txlist` was observed
        # returning DIFFERENT counts for the same address on consecutive calls
        # (polygon came back with 71 transactions and then 79). A single pass
        # silently under-reports, and an under-report here looks exactly like
        # "there were fewer payments", which is the failure this whole backfill
        # exists to correct.
        nonce_hex = call({"chainid": cid, "module": "proxy",
                          "action": "eth_getTransactionCount",
                          "address": FACILITATOR_WALLET, "tag": "latest"},
                         key, args.delay).get("result")
        calls += 1
        expected = int(nonce_hex, 16) if nonce_hex else None

        # Sweep repeatedly and take the UNION of hashes until the count matches
        # the nonce. Each pass is cheap and they converge; passes stop early the
        # moment coverage is complete.
        txs, seen = [], set()
        for sweep in range(6):
            cursor = 0
            while True:
                b = call({"chainid": cid, "module": "account", "action": "txlist",
                          "address": FACILITATOR_WALLET, "startblock": cursor,
                          "endblock": 99999999, "page": 1, "offset": 5000,
                          "sort": "asc"}, key, args.delay).get("result") or []
                calls += 1
                if not isinstance(b, list) or not b:
                    break
                fresh = [t for t in b if t.get("hash") not in seen]
                for t in fresh:
                    seen.add(t["hash"])
                txs.extend(fresh)
                if len(b) < 5000:
                    break
                cursor = int(b[-1]["blockNumber"])
            if expected is None or len(seen) >= expected:
                break

        coverage = (len(seen) / expected * 100) if expected else 0.0
        if expected and len(seen) < expected:
            print(f"  {name:<11} AVISO: {len(seen)}/{expected} transacciones "
                  f"({coverage:.1f}%) tras 6 barridos — la cifra de abajo es un PISO",
                  file=sys.stderr)

        caps = [t for t in txs if (t.get("input") or "").startswith(SELECTOR_CAPTURE)
                and t.get("isError") != "1"]
        refs = sum(1 for t in txs if (t.get("input") or "").startswith(SELECTOR_REFUND))
        auths = sum(1 for t in txs if (t.get("input") or "").startswith(SELECTOR_AUTHORIZE))

        recovered = unreadable = 0
        for t in caps:
            rec = call({"chainid": cid, "module": "proxy",
                        "action": "eth_getTransactionReceipt",
                        "txhash": t["hash"]}, key, args.delay).get("result") or {}
            calls += 1
            if rec.get("status") != "0x1":
                unreadable += 1
                continue
            token, total = None, 0
            for lg in rec.get("logs", []):
                tp = lg.get("topics") or []
                if len(tp) == 3 and tp[0].lower() == TRANSFER_TOPIC:
                    token = (lg.get("address") or "").lower()
                    total += int(lg["data"], 16)
            if not token or total == 0:
                unreadable += 1
                continue
            g = grand.setdefault((name, token), {"n": 0, "vol": 0, "first": None, "last": None})
            g["n"] += 1
            g["vol"] += total
            ts = int(t.get("timeStamp", 0)) * 1000
            g["first"] = min(g["first"] or ts, ts)
            g["last"] = max(g["last"] or ts, ts)
            recovered += 1

        print(f"  {name:<11} txs={len(seen)}/{expected or '?'} ({coverage:.0f}%) | "
              f"autoriz={auths:<5} capturas={len(caps):<4} reemb={refs:<4} | "
              f"reconstruidas={recovered} ilegibles={unreadable}")

    print(f"\n  llamadas a la API usadas: {calls}")
    if not grand:
        print("  nada que reconstruir")
        return 0

    import datetime
    print("\n  ESCROW RECONSTRUIDO (solo capturas; reembolsos excluidos):")
    tn = tv = 0
    for (net, asset), g in sorted(grand.items(), key=lambda x: -x[1]["vol"]):
        tn += g["n"]; tv += g["vol"]
        f = datetime.datetime.fromtimestamp(g["first"] / 1000, datetime.UTC)
        l = datetime.datetime.fromtimestamp(g["last"] / 1000, datetime.UTC)
        print(f"    {net:<11} {asset[:14]}…  {g['n']:>5} capturas  "
              f"{g['vol'] / 1e6:>10.2f} USDC   {f:%Y-%m-%d} → {l:%Y-%m-%d}")
    print(f"\n  TOTAL ESCROW: {tn} capturas, {tv / 1e6:.2f} USDC")

    if not args.apply:
        print("\n  DRY-RUN: no se escribio nada.")
        return 0

    ok = bad = 0
    for (net, asset), g in sorted(grand.items()):
        res = subprocess.run(
            ["aws", "dynamodb", "update-item", "--table-name", args.table,
             "--region", "us-east-2",
             "--key", json.dumps({"pk": {"S": "AGG-BACKFILL"},
                                  "sk": {"S": f"escrow#{net}#{asset}"}}),
             "--update-expression",
             "SET #src = :src, network = :net, asset = :asset, scheme = :sch, "
             "settles_ok = :n, volume_atomic = :v, first_ts = :f, last_ts = :l",
             "--expression-attribute-names", json.dumps({"#src": "source"}),
             "--expression-attribute-values", json.dumps({
                 ":src": {"S": "onchain-backfill"}, ":sch": {"S": "escrow"},
                 ":net": {"S": net}, ":asset": {"S": asset},
                 ":n": {"N": str(g["n"])}, ":v": {"N": str(g["vol"])},
                 ":f": {"N": str(g["first"])}, ":l": {"N": str(g["last"])}})],
            capture_output=True, text=True, timeout=60)
        if res.returncode == 0:
            print(f"    OK   escrow#{net}  {g['n']} capturas  {g['vol'] / 1e6:.2f} USDC")
            ok += 1
        else:
            print(f"    FALLO escrow#{net}: {(res.stderr or '').strip()[-70:]}")
            bad += 1
    print(f"\n  filas escritas: {ok} | fallidas: {bad}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
