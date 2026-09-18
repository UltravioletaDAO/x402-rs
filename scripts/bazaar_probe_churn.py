#!/usr/bin/env python3
"""bazaar_probe_churn.py -- how much does the health prober's view actually move?

WHAT. Measures two numbers about the Bazaar health prober
(`src/discovery_health.rs`) from the PUBLIC discovery API, with no AWS and no
credentials:

  * probes/day   -- how many resources the prober re-checks in a day
  * churn/day    -- how many of those come back with a DIFFERENT HTTP status
                    than the probe before them

WHY IT EXISTS. Every proposal to spend something per probe -- a classifier, an
extra fetch, an alert -- is priced by one of those two numbers, and both are
easy to guess wrong by two orders of magnitude. `DISCOVERY_HEALTH_MAX_RPS`
(default 2) times 86400 gives a BUDGET of 172,800 probes/day; the realised rate
is a different number entirely, because a healthy resource is re-probed once a
week (`HEALTHY_REPROBE_SECS`) and a quarantined one on a 1h..72h backoff
(`BACKOFF_SECS`). Measured 2026-09-18 on a 1,999-resource catalog: 1,022
distinct resources probed in 24 h against that 172,800 budget, and 1.5% of
probes came back with a different status.

!! READ THIS BEFORE QUOTING A `watch` NUMBER !!
Listing the catalog CAUSES probes. `src/discovery.rs:1259-1275`: a listing
served from a price observation that is `Stale` or `Unknown` enqueues that
resource for revalidation, and revalidating is probing. So this script's own
snapshots inflate the probe rate it measures -- measured 2026-09-18, walking
all 1,999 records every ~11 min drove ~21,000 probes/day where the untouched
catalog showed ~1,000/day. Two consequences:

  * The CLEAN volume figure is `snapshot --report` on ONE snapshot, read before
    you start looping: the `lastChecked` histogram describes the 24 h BEFORE
    you arrived.
  * `compare` / `watch` measure the CHURN RATE (what fraction of probes change)
    honestly -- a probe is a probe whoever asked for it -- but their probes/day
    is an upper bound that includes your own demand. Do not quote it as the
    facilitator's baseline rate.

And the probes are real outbound requests to third-party sellers. Keep `watch`
short and `--every` generous; there is no reason to leave it running.

HOW. `GET /discovery/resources?health=any` exposes, per resource, the overlay's
`lastChecked` / `httpStatus`. Two snapshots N minutes apart give:
  probed  = resources whose `lastChecked` advanced
  changed = of those, the ones whose `httpStatus` differs
Both are exact within the window; the per-day figures are extrapolations and
the script prints the window so nobody quotes them as if they were counted.

The prober runs on ONE task (the job owner, `discovery_owner::owns_jobs`), so
this measures the whole fleet, not one replica.

USAGE
  # take snapshots yourself, then compare (recommended: >= 30 min apart)
  python scripts/bazaar_probe_churn.py snapshot /tmp/a.json
  python scripts/bazaar_probe_churn.py snapshot /tmp/b.json
  python scripts/bazaar_probe_churn.py compare /tmp/a.json /tmp/b.json

  # one-shot: N snapshots every M seconds, then the report
  python scripts/bazaar_probe_churn.py watch --count 6 --every 600 --dir /tmp/churn

  # a single snapshot also reports the age distribution of `lastChecked`,
  # which is a lower bound on probes/day from ONE call
  python scripts/bazaar_probe_churn.py snapshot /tmp/a.json --report

NOTE. Aggregates only: the report never prints a resource URL, because the
catalog lists third-party endpoints and this file lives in a public repo.
"""

from __future__ import annotations

import argparse
import collections
import glob
import json
import os
import sys
import time
import urllib.error
import urllib.request

DEFAULT_BASE = "https://facilitator.ultravioletadao.xyz"
PAGE = 100
UA = "uvd-bazaar-probe-churn/1.0"


def fetch_page(base: str, offset: int, retries: int = 5) -> dict:
    url = f"{base}/discovery/resources?limit={PAGE}&offset={offset}&health=any"
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(req, timeout=40) as r:
                return json.loads(r.read())
        except Exception as e:  # noqa: BLE001 - network, throttle, bad gateway
            if attempt == retries - 1:
                raise SystemExit(f"giving up at offset={offset}: {e}")
            # The listing route is rate limited; back off rather than hammer it.
            time.sleep(5 * (attempt + 1))
    raise SystemExit("unreachable")


def snapshot(base: str, pause: float) -> dict:
    records: dict[str, dict] = {}
    offset, total = 0, None
    while True:
        page = fetch_page(base, offset)
        total = page["pagination"]["total"]
        items = page["items"]
        if not items:
            break
        for item in items:
            health = item.get("health") or {}
            records[item["url"]] = {
                "status": health.get("status"),
                "lastChecked": health.get("lastChecked"),
                "httpStatus": health.get("httpStatus"),
            }
        offset += PAGE
        if offset >= total:
            break
        time.sleep(pause)
    return {"capturedAt": int(time.time()), "base": base, "total": total, "records": records}


def report_one(snap: dict) -> None:
    now = snap["capturedAt"]
    records = snap["records"]
    print(f"captured {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(now))}  "
          f"records={len(records)}")
    by_status = collections.Counter(r["status"] for r in records.values())
    print(f"  health: {dict(by_status)}")
    ages = [now - r["lastChecked"] for r in records.values() if r.get("lastChecked")]
    for label, window in (("1h", 3600), ("24h", 86400), ("7d", 7 * 86400)):
        print(f"  distinct resources last probed within {label:3s}: "
              f"{sum(1 for a in ages if a < window)}")
    print("  (a resource probed twice in a window counts once, so these are LOWER bounds)")


def compare(a: dict, b: dict) -> dict:
    window = b["capturedAt"] - a["capturedAt"]
    if window <= 0:
        raise SystemExit("the second snapshot is not newer than the first")
    A, B = a["records"], b["records"]
    common = set(A) & set(B)
    probed = [u for u in common if (B[u].get("lastChecked") or 0) > (A[u].get("lastChecked") or 0)]
    changed = [u for u in probed if B[u].get("httpStatus") != A[u].get("httpStatus")]
    moved = [u for u in probed if B[u].get("status") != A[u].get("status")]
    transitions = collections.Counter(
        (A[u].get("httpStatus"), B[u].get("httpStatus")) for u in changed
    )
    return {
        "window_secs": window,
        "common": len(common),
        "probed": len(probed),
        "changed_http": len(changed),
        "changed_health_status": len(moved),
        "transitions": transitions,
    }


def print_comparison(c: dict) -> None:
    w = c["window_secs"]
    days = w / 86400.0
    print(f"window {w}s ({w/60:.1f} min) over {c['common']} resources")
    print(f"  probed in window          : {c['probed']}")
    print(f"  http status changed       : {c['changed_http']}")
    print(f"  health class changed      : {c['changed_health_status']}")
    if c["probed"]:
        pct = 100.0 * c["changed_http"] / c["probed"]
        print(f"  churn rate                : {pct:.2f}% of probes")
    print(f"  extrapolated probes/day   : {c['probed'] / days:,.0f}")
    print(f"  extrapolated churn/day    : {c['changed_http'] / days:,.1f}")
    print("  WARNING: probes/day above INCLUDES the probes this script caused --")
    print("  listing the catalog enqueues revalidation for stale observations")
    print("  (src/discovery.rs:1259-1275). It is an upper bound, not the")
    print("  facilitator's baseline rate; for that use `snapshot --report`.")
    print("  The churn RATE (% of probes that changed) is not affected.")
    if c["transitions"]:
        print("  transitions seen (prev -> now):")
        for (x, y), n in c["transitions"].most_common(20):
            print(f"    {x} -> {y} : {n}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default=os.environ.get("FACILITATOR_URL", DEFAULT_BASE))
    ap.add_argument("--pause", type=float, default=2.0,
                    help="seconds between listing pages (the route is rate limited)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("snapshot", help="write one snapshot to a file")
    s.add_argument("out")
    s.add_argument("--report", action="store_true", help="also print the age distribution")

    c = sub.add_parser("compare", help="compare two snapshots")
    c.add_argument("first")
    c.add_argument("second")

    w = sub.add_parser("watch", help="take N snapshots every M seconds, then report")
    w.add_argument("--count", type=int, default=6)
    w.add_argument("--every", type=int, default=600)
    w.add_argument("--dir", default="/tmp/bazaar-churn")

    args = ap.parse_args()

    if args.cmd == "snapshot":
        snap = snapshot(args.base, args.pause)
        with open(args.out, "w") as f:
            json.dump(snap, f)
        print(f"wrote {args.out}: {len(snap['records'])} records")
        if args.report:
            report_one(snap)
        return 0

    if args.cmd == "compare":
        a = json.load(open(args.first))
        b = json.load(open(args.second))
        print_comparison(compare(a, b))
        return 0

    os.makedirs(args.dir, exist_ok=True)
    paths = []
    for i in range(args.count):
        if i:
            time.sleep(args.every)
        path = os.path.join(args.dir, f"snap-{i:03d}.json")
        snap = snapshot(args.base, args.pause)
        with open(path, "w") as f:
            json.dump(snap, f)
        paths.append(path)
        print(f"  [{i+1}/{args.count}] {path}  records={len(snap['records'])}", file=sys.stderr)

    snaps = [json.load(open(p)) for p in paths]
    total_probed = total_changed = 0
    span = snaps[-1]["capturedAt"] - snaps[0]["capturedAt"]
    for a, b in zip(snaps, snaps[1:]):
        c = compare(a, b)
        total_probed += c["probed"]
        total_changed += c["changed_http"]
    print()
    print(f"=== {len(snaps)} snapshots over {span}s ({span/3600:.2f}h) ===")
    print(f"  probes counted            : {total_probed}")
    print(f"  http status changes       : {total_changed}")
    if total_probed:
        print(f"  churn rate                : {100.0*total_changed/total_probed:.2f}% of probes")
    if span:
        print(f"  probes/day (UPPER BOUND)  : {total_probed / (span/86400.0):,.0f}")
        print(f"  churn/day  (UPPER BOUND)  : {total_changed / (span/86400.0):,.1f}")
        print()
        print("  Both per-day figures include the probes THIS RUN caused by listing")
        print("  the catalog. The churn RATE above is the number to carry forward;")
        print("  multiply it by the clean probes/day from `snapshot --report`.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
