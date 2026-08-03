#!/usr/bin/env python3
"""Probe and rank EVM RPC endpoints for use as a facilitator primary.

A facilitator RPC has to do more than answer. It has to be on the right chain,
at the chain tip, able to read state, and able to survive a burst without being
rate limited. Each of those is a distinct failure we have actually hit in
production -- see references/failure-modes.md.

Usage:
    # rank a candidate list (fast matrix)
    probe_rpc.py --chain-id 42220 --urls https://a.example,https://b.example

    # same, reading one URL per line (comments with # allowed)
    probe_rpc.py --chain-id 42220 --url-file candidates.txt

    # sustained stability -- single samples lie, this is the one that decides
    probe_rpc.py --chain-id 42220 --url-file c.txt --soak 30 --interval 4

    # rate-limit probe: concurrent burst of the read a settle actually does
    probe_rpc.py --chain-id 42220 --url-file c.txt --burst 60 --concurrency 20

    # nonce agreement across candidates (write-path safety)
    probe_rpc.py --chain-id 42220 --url-file c.txt --nonce-check

Exit code is 1 if no candidate passed, so this can gate a deploy.
"""
import argparse
import concurrent.futures as cf
import json
import statistics
import sys
import time
import urllib.error
import urllib.request

# Some providers 403 the default Python-urllib User-Agent. Every endpoint we
# once wrote off as "down" during the 2026-08-03 Celo sweep was actually just
# rejecting that UA. Always send a browser-shaped one.
HEADERS = {
    "content-type": "application/json",
    "user-agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36",
    "accept": "application/json",
}

# Defaults are Celo mainnet: USDC and the facilitator's mainnet EOA. Override
# with --token / --wallet for other chains. Never type these from memory --
# copy from src/network.rs and lambda/balances/handler.py.
DEFAULT_TOKEN = "0xcebA9300f2b948710d2653dD7B07f33A8B32118C"
DEFAULT_WALLET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"

# Max blocks behind the observed tip before a node is unusable. Nodes that are
# fast but millions of blocks stale are the nastiest trap: they look great on
# latency and fail every state read.
MAX_LAG = 50


def rpc(url, method, params, timeout=12):
    """Single JSON-RPC call. Returns (result, elapsed_ms). Raises on rpc error."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=body, headers=HEADERS)
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        payload = json.loads(r.read())
    elapsed = (time.perf_counter() - t0) * 1000
    if "error" in payload:
        raise RuntimeError(str(payload["error"].get("message", "rpc error"))[:70])
    return payload["result"], elapsed


def describe(exc):
    if isinstance(exc, urllib.error.HTTPError):
        return f"HTTP {exc.code}"
    return f"{type(exc).__name__}: {str(exc)[:50]}"


def matrix(url, chain_id, token, wallet):
    """Full qualification of one endpoint. Order matters: cheapest kill first."""
    row = {"url": url, "ok": False, "why": []}

    try:
        cid, _ = rpc(url, "eth_chainId", [])
        row["chain_id"] = int(cid, 16)
    except Exception as e:
        row["why"].append(describe(e))
        return row
    if row["chain_id"] != chain_id:
        row["why"].append(f"WRONG CHAIN: {row['chain_id']} != {chain_id}")
        return row

    lat, blocks = [], []
    for _ in range(5):
        try:
            b, dt = rpc(url, "eth_blockNumber", [])
            lat.append(dt)
            blocks.append(int(b, 16))
        except Exception as e:
            row["why"].append(describe(e))
            return row
    row["block"] = max(blocks)
    row["p50_ms"] = round(statistics.median(lat))
    row["max_ms"] = round(max(lat))
    if row["block"] == 0:
        row["why"].append("HEAD AT BLOCK 0 (node never finished syncing)")
        return row

    # Everything the facilitator touches on a verify/settle round trip.
    checks = {
        "getCode": ("eth_getCode", [token, "latest"]),
        "call": ("eth_call", [{"to": token, "data": "0x06fdde03"}, "latest"]),
        "balance": ("eth_getBalance", [wallet, "latest"]),
        "nonce": ("eth_getTransactionCount", [wallet, "pending"]),
        "gasPrice": ("eth_gasPrice", []),
        "feeHistory": ("eth_feeHistory", ["0x1", "latest", [50]]),
        "getLogs": ("eth_getLogs", [{"fromBlock": hex(row["block"] - 5),
                                     "toBlock": hex(row["block"]), "address": token}]),
    }
    for name, (method, params) in checks.items():
        try:
            res, _ = rpc(url, method, params)
            if name == "getCode" and (not res or res == "0x"):
                row["why"].append("getCode returned empty (no state / wrong token)")
        except Exception as e:
            row["why"].append(f"{name}: {describe(e)}")

    row["ok"] = not row["why"]
    return row


def run_matrix(urls, args):
    with cf.ThreadPoolExecutor(max_workers=12) as ex:
        rows = list(ex.map(lambda u: matrix(u, args.chain_id, args.token, args.wallet), urls))

    tip = max((r.get("block", 0) for r in rows), default=0)
    for r in rows:
        if r["ok"] and tip - r.get("block", 0) > MAX_LAG:
            r["ok"] = False
            r["why"].append(f"STALE: {tip - r['block']} blocks behind tip")

    good = sorted([r for r in rows if r["ok"]], key=lambda r: r["p50_ms"])
    bad = [r for r in rows if not r["ok"]]

    print(f"chain {args.chain_id} | observed tip {tip} | {len(good)} usable / {len(urls)}\n")
    if good:
        print(f"{'RPC':56}{'p50':>8}{'max':>8}{'lag':>7}")
        print("-" * 79)
        for r in good:
            print(f"{r['url']:56}{r['p50_ms']:>6}ms{r['max_ms']:>6}ms{tip - r['block']:>7}")
    if bad:
        print(f"\nUNUSABLE ({len(bad)}):")
        for r in bad:
            print(f"  {r['url']:56}{'; '.join(r['why'])[:75]}")
    return good


def run_soak(urls, args):
    """Repeated sampling. This is the check that actually decides the winner:
    load balancers that front a broken node pass a single probe and fail here."""
    st = {u: {"lat": [], "err": [], "blocks": [], "ids": set()} for u in urls}

    def sample(url):
        s = st[url]
        body = json.dumps([
            {"jsonrpc": "2.0", "id": 1, "method": "eth_chainId", "params": []},
            {"jsonrpc": "2.0", "id": 2, "method": "eth_blockNumber", "params": []},
        ]).encode()
        try:
            req = urllib.request.Request(url, data=body, headers=HEADERS)
            t0 = time.perf_counter()
            with urllib.request.urlopen(req, timeout=10) as r:
                out = json.loads(r.read())
            s["lat"].append((time.perf_counter() - t0) * 1000)
            by_id = {o["id"]: o for o in out}
            if any("error" in by_id[i] for i in (1, 2)):
                s["err"].append("rpc error in batch")
                return
            s["ids"].add(int(by_id[1]["result"], 16))
            s["blocks"].append(int(by_id[2]["result"], 16))
        except Exception as e:
            s["err"].append(describe(e))

    for i in range(args.soak):
        with cf.ThreadPoolExecutor(max_workers=len(urls)) as ex:
            list(ex.map(sample, urls))
        if i < args.soak - 1:
            time.sleep(args.interval)

    tip = max((max(s["blocks"]) for s in st.values() if s["blocks"]), default=0)
    print(f"\nsoak: {args.soak} rounds x {args.interval}s | tip {tip}\n")
    print(f"{'RPC':46}{'errors':>9}{'p50':>8}{'p95':>8}{'lag':>7}  flags")
    print("-" * 95)
    ranked = []
    for u in urls:
        s = st[u]
        if not s["blocks"]:
            print(f"{u:46}{len(s['err']):>4}/{args.soak:<4} ALL FAILED: {s['err'][0] if s['err'] else '?'}")
            continue
        lat = sorted(s["lat"])
        p50 = round(statistics.median(lat))
        p95 = round(lat[max(0, int(len(lat) * 0.95) - 1)])
        flags = []
        if len(s["ids"]) > 1:
            flags.append("CHAIN ID FLIPPED")
        if 0 in s["blocks"]:
            flags.append("SERVED BLOCK 0")
        if s["err"]:
            flags.append(f"errs: {sorted(set(s['err']))[0]}")
        print(f"{u:46}{len(s['err']):>4}/{args.soak:<4}{p50:>6}ms{p95:>6}ms"
              f"{tip - max(s['blocks']):>7}  {', '.join(flags) or '-'}")
        ranked.append((len(s["err"]), p95, u))

    print("\nranked (fewest errors, then p95):")
    for i, (errs, p95, u) in enumerate(sorted(ranked), 1):
        print(f"  {i}. {u}  ({errs} errors, p95 {p95}ms)")
    return ranked


def run_burst(urls, args):
    """Concurrent burst of the read a settle does. Finds rate limits, which are
    an outage when the facilitator has no fallback configured."""
    def one(url):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_call",
                           "params": [{"to": args.token, "data": "0x06fdde03"}, "latest"]}).encode()
        t0 = time.perf_counter()
        try:
            with urllib.request.urlopen(
                    urllib.request.Request(url, data=body, headers=HEADERS), timeout=15) as r:
                payload = json.loads(r.read())
            dt = (time.perf_counter() - t0) * 1000
            return ("rpc_error" if "error" in payload else "ok", dt)
        except Exception as e:
            return (describe(e), (time.perf_counter() - t0) * 1000)

    print(f"\nburst: {args.burst} eth_call at {args.concurrency} concurrent\n")
    print(f"{'RPC':46}{'ok':>9}{'p50':>8}{'p95':>8}  failures")
    print("-" * 95)
    for url in urls:
        t0 = time.perf_counter()
        with cf.ThreadPoolExecutor(max_workers=args.concurrency) as ex:
            res = list(ex.map(lambda _: one(url), range(args.burst)))
        rate = round(args.burst / (time.perf_counter() - t0))
        lat = sorted(d for _, d in res)
        ok = sum(1 for s, _ in res if s == "ok")
        fails = {}
        for s, _ in res:
            if s != "ok":
                fails[s] = fails.get(s, 0) + 1
        p95 = round(lat[max(0, int(len(lat) * 0.95) - 1)])
        print(f"{url:46}{ok:>4}/{args.burst:<4}{round(statistics.median(lat)):>6}ms{p95:>6}ms"
              f"  {fails or '-'}  ({rate} req/s)")
        time.sleep(3)


def run_nonce_check(urls, args):
    """Nonce/head agreement. A cached or lagging node can hand back a stale
    nonce and collide the facilitator's next settle."""
    def q(url):
        body = json.dumps([
            {"jsonrpc": "2.0", "id": 1, "method": "eth_blockNumber", "params": []},
            {"jsonrpc": "2.0", "id": 2, "method": "eth_getTransactionCount",
             "params": [args.wallet, "pending"]},
            {"jsonrpc": "2.0", "id": 3, "method": "eth_getTransactionCount",
             "params": [args.wallet, "latest"]},
        ]).encode()
        try:
            with urllib.request.urlopen(
                    urllib.request.Request(url, data=body, headers=HEADERS), timeout=12) as r:
                d = {o["id"]: o.get("result") for o in json.loads(r.read())}
            return (int(d[1], 16), int(d[2], 16), int(d[3], 16))
        except Exception as e:
            return (None, describe(e), None)

    print(f"\nnonce/head agreement for {args.wallet}\n")
    for rnd in range(args.nonce_rounds):
        with cf.ThreadPoolExecutor(max_workers=len(urls)) as ex:
            res = dict(zip(urls, ex.map(q, urls)))
        tip = max((v[0] for v in res.values() if v[0] is not None), default=0)
        nonces = {v[1] for v in res.values() if v[0] is not None}
        verdict = "AGREE" if len(nonces) <= 1 else f"DISAGREE {sorted(nonces)}"
        print(f"--- round {rnd + 1}  tip={tip}  nonces {verdict}")
        for u, v in res.items():
            if v[0] is None:
                print(f"  {u:46} FAILED: {v[1]}")
            else:
                print(f"  {u:46} lag={tip - v[0]:<4} nonce pending={v[1]} latest={v[2]}")
        if rnd < args.nonce_rounds - 1:
            time.sleep(args.interval)


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--chain-id", type=int, required=True, help="expected chain id, e.g. 42220")
    p.add_argument("--urls", help="comma-separated candidate URLs")
    p.add_argument("--url-file", help="file with one URL per line, # comments allowed")
    p.add_argument("--token", default=DEFAULT_TOKEN, help="token contract for state reads")
    p.add_argument("--wallet", default=DEFAULT_WALLET, help="facilitator EOA for balance/nonce")
    p.add_argument("--soak", type=int, metavar="N", help="run N stability rounds")
    p.add_argument("--interval", type=int, default=4, help="seconds between soak rounds")
    p.add_argument("--burst", type=int, metavar="N", help="fire N concurrent calls")
    p.add_argument("--concurrency", type=int, default=20)
    p.add_argument("--nonce-check", action="store_true")
    p.add_argument("--nonce-rounds", type=int, default=4)
    args = p.parse_args()

    urls = []
    if args.urls:
        urls += [u.strip() for u in args.urls.split(",") if u.strip()]
    if args.url_file:
        with open(args.url_file) as fh:
            urls += [ln.split("#")[0].strip() for ln in fh if ln.split("#")[0].strip()]
    if not urls:
        p.error("need --urls or --url-file")
    urls = list(dict.fromkeys(urls))

    good = run_matrix(urls, args)
    survivors = [r["url"] for r in good]
    if not survivors:
        print("\nno usable endpoint found")
        return 1

    if args.soak:
        run_soak(survivors, args)
    if args.burst:
        run_burst(survivors, args)
    if args.nonce_check:
        run_nonce_check(survivors, args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
