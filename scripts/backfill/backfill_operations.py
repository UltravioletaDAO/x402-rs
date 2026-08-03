#!/usr/bin/env python3
"""Phase 3: every operation the facilitator has processed, payment or not.

Phases 1 and 2 reconstructed money movement. This one counts WORK: the ERC-8004
registrations, feedback submissions and agent-NFT transfers the facilitator paid
gas for and executed on someone's behalf. They move no USDC, so they belong
nowhere near a volume figure — but they are the majority of what this service
actually does, and leaving them out makes the facilitator look like it processed
11k payments when it processed 18.5k operations.

Categories are derived from the method name in the ABI, not guessed from the
selector, and anything unrecognised is reported under its raw selector rather
than folded into a bucket that would make the total look complete.
"""

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.parse
import urllib.request

FACILITATOR_WALLET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
API = "https://api.etherscan.io/v2/api"
CHAINS = {"base": 8453, "avalanche": 43114, "arbitrum": 42161, "polygon": 137,
          "optimism": 10, "ethereum": 1, "celo": 42220, "monad": 143, "unichain": 130}

# Selectors the ABI does not name for us, identified by inspecting real receipts.
KNOWN = {
    "0xcf092995": "settle_exact",     # transferWithAuthorization(…, bytes sig)
    "0xe3ee160e": "settle_exact",     # transferWithAuthorization(…, v, r, s)
    "0x41d66202": "escrow_authorize",
    "0xecf39b0a": "escrow_capture",
    "0xe2b8996f": "escrow_refund",
    "0x3c036a7e": "erc8004_feedback",
    "0xf2c298be": "erc8004_register",
    "0x42842e0e": "erc8004_nft_transfer",
}
BY_NAME = {
    "transferwithauthorization": "settle_exact",
    "authorize": "escrow_authorize",
    "givefeedback": "erc8004_feedback",
    "register": "erc8004_register",
    "safetransferfrom": "erc8004_nft_transfer",
    "executedeposit": "escrow_deposit",
    "approve": "token_approve",
}


def api_key():
    if os.environ.get("ETHERSCAN_API_KEY"):
        return os.environ["ETHERSCAN_API_KEY"]
    out = subprocess.run(["aws", "secretsmanager", "get-secret-value",
                          "--secret-id", "facilitator-etherscan-api-key",
                          "--region", "us-east-2", "--query", "SecretString",
                          "--output", "text"], capture_output=True, text=True, timeout=60)
    if out.returncode != 0:
        sys.exit("ERROR: sin ETHERSCAN_API_KEY ni secreto accesible")
    return out.stdout.strip()


def call(params, key, delay):
    q = urllib.parse.urlencode({**params, "apikey": key})
    req = urllib.request.Request(f"{API}?{q}",
                                 headers={"User-Agent": "x402-facilitator-backfill/1.0"})
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


def classify(tx):
    name = (tx.get("functionName") or "").split("(")[0].strip().lower()
    if name in BY_NAME:
        return BY_NAME[name]
    sel = (tx.get("input") or "0x")[:10]
    if sel in KNOWN:
        return KNOWN[sel]
    if sel in ("0x", "0x0"):
        return "native_transfer"
    return f"unknown:{sel}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chains", default=",".join(CHAINS))
    ap.add_argument("--delay", type=float, default=0.3)
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
        nh = call({"chainid": cid, "module": "proxy", "action": "eth_getTransactionCount",
                   "address": FACILITATOR_WALLET, "tag": "latest"}, key, args.delay).get("result")
        calls += 1
        expected = int(nh, 16) if nh else None

        seen = {}
        for _ in range(5):
            cursor = 0
            while True:
                b = call({"chainid": cid, "module": "account", "action": "txlist",
                          "address": FACILITATOR_WALLET, "startblock": cursor,
                          "endblock": 99999999, "page": 1, "offset": 5000,
                          "sort": "asc"}, key, args.delay).get("result") or []
                calls += 1
                if not isinstance(b, list) or not b:
                    break
                for t in b:
                    seen[t["hash"]] = t
                if len(b) < 5000:
                    break
                cursor = int(b[-1]["blockNumber"])
            if expected is None or len(seen) >= expected:
                break

        per = {}
        reverted = 0
        for t in seen.values():
            if t.get("isError") == "1" or t.get("txreceipt_status") == "0":
                reverted += 1
                continue
            k = classify(t)
            e = per.setdefault(k, {"n": 0, "first": None, "last": None})
            e["n"] += 1
            ts = int(t.get("timeStamp", 0)) * 1000
            e["first"] = min(e["first"] or ts, ts)
            e["last"] = max(e["last"] or ts, ts)
        grand[name] = {"ops": per, "total": len(seen), "expected": expected,
                       "reverted": reverted}
        cov = f"{len(seen)}/{expected}" if expected else str(len(seen))
        print(f"  {name:<11} {cov:<14} revertidas={reverted:<5} tipos={len(per)}")

    print(f"\n  llamadas usadas: {calls}")
    agg = {}
    for net, d in grand.items():
        for k, e in d["ops"].items():
            agg[k] = agg.get(k, 0) + e["n"]
    print("\n  OPERACIONES PROCESADAS (historico completo, sin revertidas):")
    for k, v in sorted(agg.items(), key=lambda x: -x[1]):
        print(f"    {v:>7}  {k}")
    print(f"    {sum(agg.values()):>7}  TOTAL")
    print(f"    {sum(d['reverted'] for d in grand.values()):>7}  (revertidas, excluidas)")

    if not args.apply:
        print("\n  DRY-RUN: no se escribio nada.")
        return 0

    ok = bad = 0
    for net, d in sorted(grand.items()):
        for kind, e in sorted(d["ops"].items()):
            res = subprocess.run(
                ["aws", "dynamodb", "update-item", "--table-name", args.table,
                 "--region", "us-east-2",
                 "--key", json.dumps({"pk": {"S": "AGG-BACKFILL"},
                                      "sk": {"S": f"ops#{net}#{kind}"}}),
                 "--update-expression",
                 "SET #src = :src, network = :net, op_kind = :k, op_count = :n, "
                 "first_ts = :f, last_ts = :l",
                 "--expression-attribute-names", json.dumps({"#src": "source"}),
                 "--expression-attribute-values", json.dumps({
                     ":src": {"S": "onchain-backfill"}, ":net": {"S": net},
                     ":k": {"S": kind}, ":n": {"N": str(e["n"])},
                     ":f": {"N": str(e["first"])}, ":l": {"N": str(e["last"])}})],
                capture_output=True, text=True, timeout=60)
            ok, bad = (ok + 1, bad) if res.returncode == 0 else (ok, bad + 1)
    print(f"\n  filas escritas: {ok} | fallidas: {bad}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
