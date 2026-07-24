#!/usr/bin/env python3
"""Bazaar discovery audit — reproducible quality report for the curated bazaar.

Paginates the facilitator's /discovery/resources, computes junk / unpayable /
staleness / per-source-quality metrics, and (optionally) runs a single-pass
402 health probe over a stratified sample. This is the tool behind
docs/plans/bazaar/01-current-state-audit.md and the post-deploy verification
gates in docs/plans/bazaar/06-rollout-and-ops.md.

Usage:
    python scripts/bazaar_audit.py                      # static audit vs prod
    python scripts/bazaar_audit.py --probe 300          # + probe 300 sampled URLs
    python scripts/bazaar_audit.py --base http://localhost:8080
    python scripts/bazaar_audit.py --json               # machine-readable output
    python scripts/bazaar_audit.py --snapshot items.json  # audit a saved snapshot

A 402 (or a parseable x402 body) is the healthy signal for an x402 resource.
Probes are plain GETs with no payment attached.
"""
import argparse
import json
import subprocess
import sys
import time
from collections import Counter, defaultdict

DEFAULT_BASE = "https://facilitator.ultravioletadao.xyz"
PAGE_LIMIT = 100  # server caps limit at 100
UA = "uvd-bazaar-audit/1.0"

NON_HTTP = ("http://", "https://")


def curl(args, timeout=25):
    r = subprocess.run(
        ["curl", "-s", "-m", str(timeout)] + args, capture_output=True, text=True
    )
    return r.stdout


def fetch_all(base):
    """Paginate /discovery/resources into a single list."""
    items, offset = [], 0
    while True:
        body = curl([f"{base}/discovery/resources?limit={PAGE_LIMIT}&offset={offset}"])
        try:
            page = json.loads(body)
        except Exception:
            print(f"[error] bad JSON at offset {offset}", file=sys.stderr)
            break
        batch = page.get("items", [])
        items.extend(batch)
        total = page.get("pagination", {}).get("total", len(items))
        offset += len(batch)
        if not batch or offset >= total:
            break
        print(f"  fetched {offset}/{total}", file=sys.stderr)
    return items


def is_junk_url(u):
    if not u.startswith(NON_HTTP):
        return "non_http_scheme"
    low = u.lower()
    if "localhost" in low or "127.0.0.1" in low or "0.0.0.0" in low:
        return "localhost_or_private"
    if ":var" in u or "%7b" in low or "{" in u:
        return "template_placeholder"
    if " " in u or "@" in u.split("//", 1)[-1].split("/", 1)[0]:
        return "malformed_or_userinfo"
    return None


def unpayable(item):
    accepts = item.get("accepts", [])
    if not accepts:
        return True
    return all(not a.get("network") for a in accepts)


def analyze(items):
    total = len(items)
    by_source = Counter(i.get("sourceFacilitator") or i.get("source") or "?" for i in items)
    empty_accepts = sum(1 for i in items if unpayable(i))
    non_tls = sum(1 for i in items if i.get("url", "").startswith("http://"))
    junk = Counter()
    junk_items = 0
    for i in items:
        flag = is_junk_url(i.get("url", ""))
        if flag:
            junk[flag] += 1
            junk_items += 1
    empty_desc = sum(1 for i in items if not i.get("description"))
    networks = Counter()
    for i in items:
        for a in i.get("accepts", []):
            if a.get("network"):
                networks[a["network"]] += 1
    # per-source quality
    per_source = defaultdict(lambda: {"n": 0, "empty": 0, "junk": 0, "non_tls": 0})
    for i in items:
        s = i.get("sourceFacilitator") or i.get("source") or "?"
        ps = per_source[s]
        ps["n"] += 1
        if unpayable(i):
            ps["empty"] += 1
        if is_junk_url(i.get("url", "")):
            ps["junk"] += 1
        if i.get("url", "").startswith("http://"):
            ps["non_tls"] += 1
    return {
        "total": total,
        "by_source": dict(by_source.most_common()),
        "empty_accepts": empty_accepts,
        "non_tls": non_tls,
        "junk_total": junk_items,
        "junk_by_flag": dict(junk),
        "empty_description": empty_desc,
        "networks_top": dict(networks.most_common(15)),
        "per_source": {k: dict(v) for k, v in per_source.items()},
    }


def stratified_sample(items, n):
    """Proportional per-source sample, min 10 / max 60 per source, clean URLs only."""
    by_source = defaultdict(list)
    for i in items:
        u = i.get("url", "")
        if u.startswith(NON_HTTP) and not is_junk_url(u):
            by_source[i.get("sourceFacilitator") or i.get("source") or "?"].append(u)
    sample = []
    for urls in by_source.values():
        take = max(10, min(60, len(urls)))
        sample.extend(urls[:take])
    return list(dict.fromkeys(sample))[:n]


def probe(urls):
    """Single-pass GET; 402 = alive x402. Returns {url: (code, klass)}."""
    results = {}
    for u in urls:
        code = curl(
            ["-o", "/dev/null", "-w", "%{http_code}", "-L", "-A", UA, u], timeout=8
        ).strip()
        if code == "402":
            klass = "alive_x402"
        elif code in ("200", "201", "400", "401", "403", "405", "415", "429"):
            klass = "alive"
        elif code in ("404", "410"):
            klass = "resource_missing"
        else:
            klass = "dead"
        results[u] = (code, klass)
    return results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default=DEFAULT_BASE)
    ap.add_argument("--snapshot", help="audit a saved items JSON instead of fetching")
    ap.add_argument("--probe", type=int, default=0, help="probe N sampled URLs")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    if args.snapshot:
        data = json.load(open(args.snapshot))
        items = data if isinstance(data, list) else data.get("items", [])
    else:
        print(f"Fetching from {args.base} ...", file=sys.stderr)
        items = fetch_all(args.base)

    report = analyze(items)

    if args.probe:
        sample = stratified_sample(items, args.probe)
        print(f"Probing {len(sample)} URLs ...", file=sys.stderr)
        pr = probe(sample)
        klass = Counter(k for _, k in pr.values())
        report["probe"] = {
            "sampled": len(sample),
            "classes": dict(klass),
            "alive_x402_rate": round(klass.get("alive_x402", 0) / max(len(sample), 1), 3),
        }

    if args.json:
        print(json.dumps(report, indent=2))
        return

    r = report
    print(f"\n=== Bazaar audit ({'snapshot' if args.snapshot else args.base}) ===")
    print(f"total items          : {r['total']}")
    print(f"unpayable (no network): {r['empty_accepts']} ({pct(r['empty_accepts'], r['total'])})")
    print(f"junk URLs            : {r['junk_total']} ({pct(r['junk_total'], r['total'])})  {r['junk_by_flag']}")
    print(f"non-TLS http://      : {r['non_tls']} ({pct(r['non_tls'], r['total'])})")
    print(f"empty description    : {r['empty_description']} ({pct(r['empty_description'], r['total'])})")
    print(f"\nnetworks (top)       : {r['networks_top']}")
    print("\nper-source quality:")
    for s, v in sorted(r["per_source"].items(), key=lambda kv: -kv[1]["n"]):
        print(f"  {s:24} n={v['n']:6}  empty={v['empty']:5}  junk={v['junk']:5}  non_tls={v['non_tls']:5}")
    if "probe" in r:
        print(f"\nprobe: {r['probe']['classes']}  alive_x402_rate={r['probe']['alive_x402_rate']}")


def pct(a, b):
    return f"{100 * a / b:.1f}%" if b else "0%"


if __name__ == "__main__":
    main()
