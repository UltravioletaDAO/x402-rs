#!/usr/bin/env python3
"""Reconstruct the facilitator's settlement history from the chain.

Why
---
The transaction store only began recording a few days ago. Measured 2026-08-03:
328 operations in the index against 17,897 transactions actually sent by the
mainnet facilitator wallet since launch — the index holds about 2% of what
happened. Any conclusion drawn from `/api/stats` about volume or network mix is
therefore a conclusion about a 2% sample that does not announce itself as one.

How this stays inside a free API tier
-------------------------------------
The naive shape of this job is "list the transactions, then fetch a receipt for
each one" — ~18,000 RPC round trips. That is unnecessary: Etherscan's `txlist`
already returns the raw `input` calldata, and for `transferWithAuthorization`
the payer, the recipient AND the amount are all arguments in that calldata. So
the amount is decoded locally and no receipt is ever fetched.

`txlist` returns up to 10,000 records per call, so the whole history of the
busiest chain costs two calls. The entire backfill is roughly a dozen requests
against a limit of 100,000 per day.

What this can and cannot recover
--------------------------------
The chain records what moved and between whom. It does NOT record which endpoint
was paid: `resource` and `description` exist only in the x402 request, which is
not on-chain. Reconstructed rows therefore answer "how much moved" and are
silent on "what was sold".

Every row written here carries `source = "onchain-backfill"`. Without that mark
nobody could later tell a reconstructed figure from a measured one, and a mixed
index that cannot express its own provenance is how a 2% sample gets quoted as a
total in the first place.
"""

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.parse
import urllib.request

# EIP-3009 has TWO transferWithAuthorization overloads and the facilitator uses
# the SECOND one, which is not the selector most references quote:
#
#   0xe3ee160e  (…, bytes32 nonce, uint8 v, bytes32 r, bytes32 s)
#   0xcf092995  (…, bytes32 nonce, bytes signature)      <- what we actually send
#
# Filtering only on 0xe3ee160e found zero settles in 12,636 Base transactions
# and looked exactly like "the facilitator never settled here". It had settled
# 7,005 times. Both are accepted now.
#
# The first six arguments are identical in the two overloads, so from/to/value
# sit at the same word offsets and one decoder covers both.
SELECTORS_TRANSFER_WITH_AUTH = ("0xcf092995", "0xe3ee160e")

FACILITATOR_WALLET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"

# Every chain now goes through Etherscan V2's unified endpoint (paid tier, from
# 2026-08-03). The free tier covered Ethereum only and the community mirrors
# throttled hard on the busiest chain — base.blockscout.com returned 429 to
# three consecutive attempts even with exponential backoff, and Base alone is
# 70% of all activity.
#
# Chain ids verified against src/network.rs `to_caip2()`.
SOURCES = {
    "base":      {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 8453},
    "avalanche": {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 43114},
    "arbitrum":  {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 42161},
    "polygon":   {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 137},
    "optimism":  {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 10},
    "ethereum":  {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 1},
    "celo":      {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 42220},
    "bsc":       {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 56},
    "unichain":  {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 130},
    "monad":     {"url": "https://api.etherscan.io/v2/api", "key": True, "chainid": 143},
}

ALL_CHAINS = ",".join(SOURCES)


def api_key():
    """Read the key from the environment, else from Secrets Manager.

    Never hardcoded, never logged, never written to disk. It is a read-only
    free-tier key, but the habit matters more than this particular key: an RPC
    URL with its credential has leaked into a log in this project before.
    """
    if os.environ.get("ETHERSCAN_API_KEY"):
        return os.environ["ETHERSCAN_API_KEY"]
    out = subprocess.run(
        ["aws", "secretsmanager", "get-secret-value",
         "--secret-id", "facilitator-etherscan-api-key",
         "--region", "us-east-2", "--query", "SecretString", "--output", "text"],
        capture_output=True, text=True, timeout=60,
    )
    if out.returncode != 0:
        sys.exit("ERROR: no hay ETHERSCAN_API_KEY ni secreto accesible")
    return out.stdout.strip()


def fetch_page(source, address, startblock, offset, key, delay):
    params = {
        "module": "account", "action": "txlist", "address": address,
        "startblock": startblock, "endblock": 99999999,
        "page": 1, "offset": offset, "sort": "asc",
    }
    if source.get("key"):
        params["apikey"] = key
        params["chainid"] = source["chainid"]
    url = f"{source['url']}?{urllib.parse.urlencode(params)}"
    # An explicit User-Agent is required, not cosmetic. urllib's default
    # ("Python-urllib/3.x") is refused outright by several of these providers —
    # base.blockscout.com answers 403 to it. The same bite happened the same day
    # on the balances Lambda, where a blocked User-Agent made Celo report null
    # and look like a dead chain. A rejected request and an empty chain are not
    # the same fact, and the default UA makes them indistinguishable.
    req = urllib.request.Request(url, headers={
        "User-Agent": "x402-facilitator-backfill/1.0 (+https://facilitator.ultravioletadao.xyz)",
        "Accept": "application/json",
    })
    # These are free community endpoints and they throttle. Backing off and
    # waiting is the correct behaviour toward a service nobody is paying for;
    # hammering it would be both rude and self-defeating. A 429 that reached the
    # caller as a hard failure would also look like "no history on this chain",
    # which is the failure mode this whole script exists to correct.
    body = None
    for attempt in range(6):
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                body = json.load(r)
            break
        except urllib.error.HTTPError as e:
            if e.code not in (429, 502, 503, 504) or attempt == 5:
                raise
            wait = delay * (2 ** attempt)
            print(f"    {e.code} de {source['url'].split('/')[2]}, reintento en {wait:.0f}s",
                  file=sys.stderr)
            time.sleep(wait)
    time.sleep(delay)
    if body.get("status") == "1":
        return body.get("result", [])
    msg = str(body.get("result") or body.get("message"))
    if "No transactions found" in msg or body.get("result") == []:
        return []
    # The key must never reach an error message.
    raise RuntimeError(f"{source['url'].split('/')[2]}: {msg[:110]}")


def decode_transfer_with_auth(input_hex):
    """Pull (payer, payTo, value) out of the calldata.

    Layout after the 4-byte selector, one 32-byte word each:
        0 from   1 to   2 value   3 validAfter   4 validBefore   5 nonce ...

    Returns None when the calldata is too short to hold those words rather than
    reading past the end and inventing a number.
    """
    data = input_hex[10:]
    if len(data) < 3 * 64:
        return None
    word = lambda i: data[i * 64:(i + 1) * 64]  # noqa: E731
    return (
        "0x" + word(0)[24:],
        "0x" + word(1)[24:],
        int(word(2), 16),
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chains", default=ALL_CHAINS,
                    help="comma-separated; default is the 93%% of activity")
    ap.add_argument("--wallet", default=FACILITATOR_WALLET)
    ap.add_argument("--delay", type=float, default=5.0,
                    help="seconds between API calls (free tier is 5/s; this is deliberately far under)")
    ap.add_argument("--apply", action="store_true", help="write; default is dry-run")
    ap.add_argument("--table", default=os.environ.get("TRANSACTIONS_TABLE_NAME",
                                                      "facilitator_transactions"))
    args = ap.parse_args()

    key = api_key()
    grand = {}
    calls = 0

    for name in [c.strip() for c in args.chains.split(",") if c.strip()]:
        source = SOURCES.get(name)
        if not source:
            print(f"  {name}: sin fuente gratuita conocida, se omite", file=sys.stderr)
            continue

        # Paginate by BLOCK, not by page number. Blockscout caps
        # `page * offset` at 10,000, and Base alone has 12,613 transactions —
        # page 2 is simply refused. Advancing `startblock` past the last block
        # seen has no such ceiling and works identically on every provider here.
        #
        # Transactions are deduplicated by hash because a block boundary can be
        # crossed twice: the cursor restarts AT the last block seen, not after
        # it, so that a block holding several transactions is never split.
        txs, seen, cursor = [], set(), 0
        PAGE = 5000
        while True:
            batch = fetch_page(source, args.wallet, cursor, PAGE, key, args.delay)
            calls += 1
            fresh = [t for t in batch if t.get("hash") not in seen]
            for t in fresh:
                seen.add(t["hash"])
            txs.extend(fresh)
            if len(batch) < PAGE or not fresh:
                break
            cursor = int(batch[-1]["blockNumber"])

        settles, failed_onchain, other = [], 0, 0
        uncovered = {}
        for t in txs:
            inp = t.get("input") or ""
            if not inp.startswith(SELECTORS_TRANSFER_WITH_AUTH):
                other += 1
                # Track what we are NOT reconstructing. Escrow settlements move
                # real money through the PaymentOperator and are invisible to a
                # calldata decoder, so counting them here keeps the gap visible
                # instead of letting the total look complete.
                if len(inp) >= 10:
                    uncovered[inp[:10]] = uncovered.get(inp[:10], 0) + 1
                continue
            # isError=1 means the chain executed and reverted it. It is not a
            # settlement and must not be counted as volume.
            if t.get("isError") == "1" or t.get("txreceipt_status") == "0":
                failed_onchain += 1
                continue
            dec = decode_transfer_with_auth(t["input"])
            if not dec:
                continue
            payer, pay_to, value = dec
            settles.append({
                "network": name, "asset": (t.get("to") or "").lower(),
                "amount": value, "tx": t.get("hash"), "payer": payer,
                "pay_to": pay_to, "ts": int(t.get("timeStamp", 0)) * 1000,
            })

        print(f"  {name:<11} {len(txs):>6} txs totales | {len(settles):>6} settles exact "
              f"| {failed_onchain:>4} revertidos | {other:>6} otras operaciones")
        if uncovered:
            top = sorted(uncovered.items(), key=lambda x: -x[1])[:3]
            print("              no reconstruido: " +
                  ", ".join(f"{sel} x{n}" for sel, n in top))
        for s in settles:
            k = (s["network"], s["asset"])
            g = grand.setdefault(k, {"n": 0, "vol": 0, "first": None, "last": None})
            g["n"] += 1
            g["vol"] += s["amount"]
            g["first"] = min(g["first"] or s["ts"], s["ts"])
            g["last"] = max(g["last"] or s["ts"], s["ts"])

    print(f"\n  llamadas a la API usadas: {calls} (limite del plan gratis: 100.000/dia)")
    if not grand:
        print("  nada que reconstruir")
        return 0

    import datetime
    print("\n  RECONSTRUIDO (red, asset, settles, volumen, rango):")
    total_n = total_v = 0
    for (net, asset), g in sorted(grand.items(), key=lambda x: -x[1]["vol"]):
        total_n += g["n"]
        total_v += g["vol"]
        f = datetime.datetime.fromtimestamp(g["first"] / 1000, datetime.UTC)
        l = datetime.datetime.fromtimestamp(g["last"] / 1000, datetime.UTC)
        print(f"    {net:<11} {asset[:14]}…  {g['n']:>5} settles  "
              f"{g['vol'] / 1e6:>10.2f} USDC   {f:%Y-%m-%d} → {l:%Y-%m-%d}")
    print(f"\n  TOTAL: {total_n} settles, {total_v / 1e6:.2f} USDC")

    if not args.apply:
        print("\n  DRY-RUN: no se escribio nada. Pasar --apply para aplicar.")
        return 0

    print(f"\n  [apply] escribiendo agregados en {args.table}")
    ok = skipped = 0
    for (net, asset), g in sorted(grand.items()):
        sk = f"{net}#{asset}"
        res = subprocess.run(
            ["aws", "dynamodb", "update-item",
             "--table-name", args.table, "--region", "us-east-2",
             "--key", json.dumps({"pk": {"S": "AGG-BACKFILL"}, "sk": {"S": sk}}),
             "--update-expression",
             "SET #src = :src, network = :net, asset = :asset, "
             "settles_ok = :n, volume_atomic = :v, first_ts = :f, last_ts = :l",
             "--expression-attribute-names", json.dumps({"#src": "source"}),
             "--expression-attribute-values", json.dumps({
                 ":src": {"S": "onchain-backfill"},
                 ":net": {"S": net}, ":asset": {"S": asset},
                 ":n": {"N": str(g["n"])}, ":v": {"N": str(g["vol"])},
                 ":f": {"N": str(g["first"])}, ":l": {"N": str(g["last"])},
             })],
            capture_output=True, text=True, timeout=60,
        )
        if res.returncode == 0:
            print(f"    OK   {sk}  {g['n']} settles  {g['vol'] / 1e6:.2f} USDC")
            ok += 1
        else:
            print(f"    FALLO {sk}: {(res.stderr or '').strip().splitlines()[-1][:80]}")
            skipped += 1

    # Written under pk=AGG-BACKFILL, NOT AGG. The live aggregate stays untouched
    # and measured; reconstructed history sits beside it under its own key. A
    # consumer that wants the full picture must combine them deliberately, which
    # is the point — silently merging is how a reconstruction becomes a
    # measurement nobody can question later.
    print(f"\n  filas escritas: {ok} | fallidas: {skipped}")
    print("  Guardadas bajo pk=AGG-BACKFILL, separadas del agregado medido (pk=AGG).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
