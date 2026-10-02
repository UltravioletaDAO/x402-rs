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

GET-ONLY VERSUS THE DECLARED METHOD (offline, no network). Since 2.47.0 the
prober sends the method a listing's `bazaar` extension declares
(`info.input.method`, else the schema's, else POST when a body is declared),
and a listing that declares none gets one POST `{}` when its GET answers 405,
400 or 404. `snapshot` now records each listing's planned method, and
`methods` reads a LOCAL file and says what the change touches before it ships:

  # a snapshot taken BEFORE the deploy (the probes in it are all GET)
  python scripts/bazaar_probe_churn.py methods /tmp/before.json

  # or a copy of the catalog object plus the health overlay, no API at all
  python scripts/bazaar_probe_churn.py methods resources.json --health health.json

and, once it ships, the measured effect per declared method:

  python scripts/bazaar_probe_churn.py compare /tmp/before.json /tmp/after.json --by-method

ONLY VERIFIED ALIVE IS PUBLIC (2.47.0). The public listing serves only what is
verified alive -- whatever `health=` says -- so a `snapshot` of a 2.47.0
facilitator holds nothing else unless `BAZAAR_ADMIN_TOKEN` is set in the
environment, in which case it also walks `GET /discovery/admin/pending` (the
token is sent as a header and never printed). And, offline, what the default
listing served before and what it serves at the deploy:

  python scripts/bazaar_probe_churn.py exposure resources.json \\
      --health health.json --terms terms.json

NOTE. Aggregates only: the report never prints a resource URL, because the
catalog lists third-party endpoints and this file lives in a public repo.
`methods --hosts` prints host names (never paths) for the drift-hold count.
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
import urllib.parse
import urllib.request

DEFAULT_BASE = "https://facilitator.ultravioletadao.xyz"
PAGE = 100
UA = "uvd-bazaar-probe-churn/1.0"

# Mirrors `ProbeMethod::parse` in src/discovery_health.rs: HEAD and DELETE are
# probed as GET, anything unrecognised is "declares nothing".
_METHODS = {"GET": "GET", "HEAD": "GET", "DELETE": "GET",
            "POST": "POST", "PUT": "PUT", "PATCH": "PATCH"}
BODY_METHODS = ("POST", "PUT", "PATCH")
# What the fallback answers when a listing declares nothing (`probe_listing`).
FALLBACK_STATUSES = (405, 400, 404)
# Marks a record from a snapshot taken before methods were captured.
UNKNOWN = "unknown"
# Health statuses a consumer that admits only live listings takes (Emporium,
# `SALUDES_QUE_ENTRAN`).
ADMISSIBLE = ("alive", "auth_gated")
# `crate::interop::PUBLIC_URL`'s host: our own origin only ever gets a GET.
OWN_HOST = "facilitator.ultravioletadao.xyz"
MANIFEST = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..",
                        "config", "bazaar_curation.json")
# `DISCOVERY_TERMS_FRESH_SECS`' default: how old a verification may be.
FRESH_WINDOW_SECS = 7 * 86400
# `LEGACY_OBSERVATION_SLACK_SECS`: a probe and the reading it recorded.
LEGACY_OBSERVATION_SLACK_SECS = 60


def _parse_method(raw):
    return _METHODS.get(raw.strip().upper()) if isinstance(raw, str) else None


def _dig(value, *keys):
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value if isinstance(value, dict) else None


def _schema_method(schema_input: dict):
    if isinstance(schema_input.get("method"), str):
        return _parse_method(schema_input["method"])
    method = _dig(schema_input, "properties", "method")
    if method is None:
        return None
    if isinstance(method.get("const"), str):
        return _parse_method(method["const"])
    enum = method.get("enum")
    if isinstance(enum, list) and enum and isinstance(enum[0], str):
        return _parse_method(enum[0])
    return None


def _declares_body(info_input, schema_input) -> bool:
    def present(d, key):
        return isinstance(d, dict) and d.get(key) is not None
    props = _dig(schema_input, "properties") if schema_input else None
    return (present(info_input, "body") or present(info_input, "bodyType")
            or present(props, "body") or present(props, "bodyType"))


def declared_probe(extensions) -> tuple:
    """(method the prober sends first or None, whether an example body is declared).

    Same order as `declared_request` in src/discovery_health.rs:
    `info.input.method`, then the schema's method, then POST when a body is
    declared without one. Whether the example fits the 8 KiB cap is not judged
    here: it only decides whether the example retry can happen, never the
    method.
    """
    bazaar = extensions.get("bazaar") if isinstance(extensions, dict) else None
    info = _dig(bazaar, "info", "input")
    schema = _dig(bazaar, "schema", "properties", "input")
    method = _parse_method(info.get("method")) if info else None
    if method is None and schema:
        method = _schema_method(schema)
    if method is None and _declares_body(info, schema):
        method = "POST"
    has_example = method in BODY_METHODS and bool(info) and info.get("body") is not None
    return method, has_example


def load_get_only(path: str = MANIFEST) -> list:
    """`probeGetOnly` from the curation manifest, as (host, path) pairs."""
    try:
        with open(path, encoding="utf-8") as f:
            entries = json.load(f).get("probeGetOnly") or []
    except (OSError, ValueError):
        return []
    return [(e["host"].lower().rstrip("."), e["path"]) for e in entries]


def _prefix_matches(url: str, host: str, path: str) -> bool:
    """Host-exact + path-boundary, https only (`match_manifest_prefix`)."""
    u = urllib.parse.urlsplit(url)
    if u.scheme != "https" or u.username or u.password:
        return False
    if (u.hostname or "").rstrip(".") != host:
        return False
    p = u.path or "/"
    if p == path:
        return True
    if path.endswith("/"):
        return p.startswith(path)
    return p.startswith(path + "/")


def planned_probe(url: str, extensions, get_only=()) -> tuple:
    """`probe_request`: our own origin and opted-out prefixes get a GET only."""
    host = (urllib.parse.urlsplit(url).hostname or "").rstrip(".")
    if host == OWN_HOST or any(_prefix_matches(url, h, p) for h, p in get_only):
        return "GET", False
    return declared_probe(extensions)


def fetch_page(base: str, offset: int, retries: int = 5, token: str = None) -> dict:
    """One page of the public listing, or of the admin pending queue with `token`."""
    if token:
        url = f"{base}/discovery/admin/pending?limit={PAGE}&offset={offset}"
        headers = {"User-Agent": UA, "Authorization": f"Bearer {token}"}
    else:
        url = f"{base}/discovery/resources?limit={PAGE}&offset={offset}&health=any"
        headers = {"User-Agent": UA}
    req = urllib.request.Request(url, headers=headers)
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
    total = 0
    get_only = load_get_only()
    token = os.environ.get("BAZAAR_ADMIN_TOKEN") or None
    if not token:
        print("note: from 2.47.0 the public listing holds only verified-alive resources; "
              "set BAZAAR_ADMIN_TOKEN to also read the pending queue", file=sys.stderr)
    # The exposed listing, then -- with the token -- everything it does not show.
    for exposed, page_token in ((True, None), (False, token)):
        if not exposed and not page_token:
            break
        offset = 0
        while True:
            page = fetch_page(base, offset, token=page_token)
            part = page["pagination"]["total"]
            items = page["items"]
            if not items:
                break
            for item in items:
                health = item.get("health") or {}
                method, has_example = planned_probe(item["url"], item.get("extensions"), get_only)
                records[item["url"]] = {
                    "status": health.get("status"),
                    "lastChecked": health.get("lastChecked"),
                    "httpStatus": health.get("httpStatus"),
                    "quarantineReason": health.get("quarantineReason"),
                    "probeMethod": health.get("probeMethod"),
                    "verifiedAt": health.get("verifiedAt"),
                    "verifiedBy": health.get("verifiedBy"),
                    "exposed": exposed,
                    "type": item.get("type"),
                    "method": method,
                    "hasExample": has_example,
                }
            offset += PAGE
            if offset >= part:
                break
            time.sleep(pause)
        total += part
    return {"capturedAt": int(time.time()), "base": base, "total": total,
            "pendingIncluded": bool(token), "records": records}


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
    # Per declared method (taken from the newer snapshot): the health class
    # each probed listing moved between. With a before-deploy snapshot and an
    # after-deploy one, this IS the effect of probing with the declared method.
    by_method: dict = collections.defaultdict(collections.Counter)
    for u in probed:
        method = B[u].get("method", UNKNOWN) or "none"
        by_method[method][(A[u].get("status"), B[u].get("status"))] += 1
    # The three groups a consumer of the catalog cares about. A host that had
    # no visible listing before and has one now is new to anyone who queues
    # hosts for review (Emporium's admission queue does).
    groups = collections.Counter()
    for u in probed:
        before, after = A[u].get("status"), B[u].get("status")
        if before == after:
            groups["unchanged"] += 1
        elif before == "auth_gated" and after == "alive":
            groups["auth_gated -> alive"] += 1
        elif before == "quarantined":
            groups["quarantined -> visible"] += 1
            # The part a consumer that admits only live listings picks up
            # (Emporium: alive and auth_gated).
            if after in ADMISSIBLE:
                groups["quarantined -> alive/auth_gated"] += 1
        else:
            groups["other"] += 1

    def hosts(records: dict, keep) -> set:
        return {_host(u) for u, r in records.items() if keep(r.get("status"))}

    def visible(status):
        return status != "quarantined"

    def admissible(status):
        return status in ADMISSIBLE

    before_c = {u: A[u] for u in common}
    after_c = {u: B[u] for u in common}
    new_hosts = hosts(after_c, visible) - hosts(before_c, visible)
    new_admissible = hosts(after_c, admissible) - hosts(before_c, admissible)
    return {
        "window_secs": window,
        "common": len(common),
        "probed": len(probed),
        "changed_http": len(changed),
        "changed_health_status": len(moved),
        "transitions": transitions,
        "by_method": dict(by_method),
        "groups": groups,
        "newly_visible_hosts": len(new_hosts),
        "newly_admissible_hosts": len(new_admissible),
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
    print("  probed listings, by what happened to them:")
    for group in ("auth_gated -> alive", "quarantined -> visible",
                  "quarantined -> alive/auth_gated", "unchanged", "other"):
        print(f"    {group:32s}: {c['groups'].get(group, 0)}")
    print(f"  hosts with a visible listing now and none before      : {c['newly_visible_hosts']}")
    print(f"  hosts with an alive/auth_gated listing now, none before: "
          f"{c['newly_admissible_hosts']}  (new to a queue that admits only those)")


def print_by_method(c: dict) -> None:
    print("  health class of the probed listings, by declared method (prev -> now):")
    for method in sorted(c["by_method"]):
        moves = c["by_method"][method]
        print(f"    {method:8s} probed={sum(moves.values())}")
        for (x, y), n in moves.most_common():
            print(f"      {x} -> {y} : {n}")


def load_records(path: str, health_path: str = None, terms_path: str = None) -> dict:
    """Records keyed by URL from a LOCAL file, in the snapshot's own shape.

    Accepts a snapshot this script wrote (`{"records": {...}}`), or the catalog
    object itself (`bazaar/resources.json`, a bare JSON array of resources)
    optionally joined with the health overlay (`bazaar/health.json`, keyed by
    URL, snake_case fields) and the observed-terms overlay (`bazaar/terms.json`,
    `{"version", "records": {url: {"observedAt", ...}}}` or the bare map).
    """
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    if isinstance(doc, dict) and "records" in doc:
        return doc["records"]
    if not isinstance(doc, list):
        raise SystemExit(f"{path}: neither a snapshot nor a catalog array")
    health = {}
    if health_path:
        with open(health_path, encoding="utf-8") as f:
            health = json.load(f)
    terms = {}
    if terms_path:
        with open(terms_path, encoding="utf-8") as f:
            terms = json.load(f)
        if isinstance(terms.get("records"), dict) and "version" in terms:
            terms = terms["records"]
    get_only = load_get_only()
    records = {}
    for item in doc:
        h = health.get(item["url"]) or {}
        method, has_example = planned_probe(item["url"], item.get("extensions"), get_only)
        records[item["url"]] = {
            "status": h.get("status"),
            "lastChecked": h.get("last_checked"),
            "httpStatus": h.get("http_status"),
            "quarantineReason": h.get("quarantine_reason"),
            "probeMethod": h.get("probe_method"),
            "verifiedAt": h.get("verified_at"),
            "verifiedBy": h.get("verified_by"),
            "mcpTools": h.get("mcp_tools"),
            "observedAt": (terms.get(item["url"]) or {}).get("observedAt"),
            "type": item.get("type"),
            "method": method,
            "hasExample": has_example,
        }
    return records


def verified_at_of(r: dict):
    """`verifiedAt`, or for a record written before it existed the reading the
    observed-terms overlay took in the same probe (`legacy_verified_at`)."""
    if r.get("verifiedAt") is not None:
        return r["verifiedAt"]
    checked, seen = r.get("lastChecked"), r.get("observedAt")
    if (r.get("probeMethod") is None and r.get("status") == "alive"
            and r.get("httpStatus") == 402 and checked is not None and seen is not None
            and abs(seen - checked) <= LEGACY_OBSERVATION_SLACK_SECS):
        return seen
    return None


def verified_alive(r: dict, now: int, window: int = FRESH_WINDOW_SECS) -> bool:
    """`is_verified_alive`: alive, a readable challenge no older than `window`,
    to the request the listing declares (any, when it declares none) -- or, for
    an MCP endpoint, a handshake that listed a tool (`verifiedBy`
    `mcp_handshake`) no older than `window`. Neither counts for the other."""
    if r.get("status") != "alive":
        return False
    if r.get("type") == "mcp":
        at = r.get("verifiedAt")
        return (r.get("verifiedBy") == "mcp_handshake" and at is not None
                and now - at <= window)
    if r.get("verifiedBy") == "mcp_handshake":
        return False
    at = verified_at_of(r)
    if at is None or now - at > window:
        return False
    method = r.get("method")
    return method is None or (r.get("probeMethod") or "GET") == method


def _why_pending(r: dict, now: int, window: int) -> str:
    status = r.get("status")
    if status is None:
        return "never_probed"
    if status != "alive":
        return status
    if r.get("type") == "mcp":
        # Alive, but no handshake of this build listed a tool within the window.
        return "alive_mcp_handshake"
    at = verified_at_of(r)
    if at is None:
        return "alive_no_readable_challenge"
    if now - at > window:
        return "alive_verified_too_long_ago"
    return "alive_to_another_method"


def exposure_report(records: dict, now: int, window: int = FRESH_WINDOW_SECS) -> dict:
    """What the default listing served before 2.47.0 and what it serves at the deploy.

    Before, it hid only `quarantined` (`health_visible`): everything else was
    served, never-probed records included. From 2.47.0 it serves only what is
    verified alive. Suppressed URLs are not in a local copy, so both sides count
    them.
    """
    before = after = 0
    pending = collections.Counter()
    for r in records.values():
        if r.get("status") != "quarantined":
            before += 1
        if verified_alive(r, now, window):
            after += 1
        else:
            pending[_why_pending(r, now, window)] += 1
    return {"records": len(records), "served_before": before, "served_after": after,
            "pending_by_reason": dict(pending), "window_secs": window, "now": now}


def _host(url: str) -> str:
    return urllib.parse.urlsplit(url).hostname or "?"


def methods_report(records: dict) -> dict:
    """What probing with the declared method changes, from one local view.

    Every probe in a snapshot taken before the deploy is a GET, so its health
    is the GET-only verdict. Against it this counts which listings the new
    prober asks differently, and which of them it re-probes in its first cycle
    (`probed_with_another_request` in src/discovery_health.rs). It cannot say
    how many come back alive -- that takes the probe; `compare --by-method`
    measures it after the deploy.
    """
    declared = collections.Counter()
    to_body = collections.Counter()
    to_body_http = collections.Counter()
    fallback = collections.Counter()
    drift_hosts = collections.Counter()
    first_cycle = 0
    for url, r in records.items():
        if "method" not in r:
            declared[UNKNOWN] += 1
            continue
        method, status, http = r["method"], r.get("status"), r.get("httpStatus")
        declared[method or "none"] += 1
        reason = r.get("quarantineReason")
        # The reason when the record carries one; before it did, its signature:
        # a fail never carries a 402, so quarantined on a 402 is a drift hold.
        drift_hold = status == "quarantined" and (
            reason == "pay_to_drift" or (reason is None and http == 402))
        if drift_hold:
            drift_hosts[_host(url)] += 1
        if r.get("type") == "mcp":
            # `unverified_legacy_alive`: an alive MCP record no handshake of
            # this build ran on (no tool count) is probed at once, once.
            first_cycle += (status == "alive" and r.get("verifiedAt") is None
                            and r.get("mcpTools") is None)
            continue
        if method in BODY_METHODS:
            to_body[status] += 1
            to_body_http[http] += 1
        elif method is None and http in FALLBACK_STATUSES:
            fallback[status] += 1
        # `probed_with_another_request`, record by record.
        if status is None:
            continue
        legacy = r.get("probeMethod") is None
        last = r.get("probeMethod") or "GET"
        if drift_hold:
            first_cycle += legacy  # looked at once, never lifted by it
        elif method in BODY_METHODS or method == "GET":
            first_cycle += last != method
        elif method is None:
            first_cycle += legacy and http in FALLBACK_STATUSES

    def keyed(counter) -> dict:
        # JSON keys are strings: a listing with no record has status `None`.
        return {str(k): v for k, v in counter.items()}

    return {
        "records": len(records),
        "declared": keyed(declared),
        "get_to_body_method_by_status": keyed(to_body),
        "get_to_body_method_by_http": keyed(to_body_http),
        "fallback_candidates_by_status": keyed(fallback),
        "reprobed_in_first_cycle": first_cycle,
        "drift_hold": sum(drift_hosts.values()),
        "drift_hold_hosts": dict(drift_hosts),
    }


def print_methods(m: dict, hosts: bool) -> None:
    print(f"records={m['records']}")
    print(f"  first request planned  : {m['declared']}  "
          f"(declared, or GET on our origin / probeGetOnly; none = GET + fallback)")
    if m["declared"].get(UNKNOWN):
        print(f"  ({m['declared'][UNKNOWN]} records come from a snapshot that predates method capture)")
    print(f"  probed with POST/PUT/PATCH instead of GET, by current status: "
          f"{m['get_to_body_method_by_status']}")
    print(f"    ... by the HTTP status their GET got : {m['get_to_body_method_by_http']}")
    print(f"  no method declared and GET got 405/400/404 (one POST {{}} retry): "
          f"{m['fallback_candidates_by_status']}")
    print(f"  re-probed in the first cycle after the deploy: {m['reprobed_in_first_cycle']}")
    print(f"  drift holds (quarantined for a payTo drift, not a liveness verdict): "
          f"{m['drift_hold']} on {len(m['drift_hold_hosts'])} hosts")
    if hosts:
        for host, n in sorted(m["drift_hold_hosts"].items(), key=lambda kv: -kv[1])[:20]:
            print(f"    {host}: {n}")


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
    c.add_argument("--by-method", action="store_true",
                   help="also break the health transitions down by declared method")

    m = sub.add_parser("methods", help="offline: what probing with the declared method changes")
    m.add_argument("path", help="a snapshot from this script, or the catalog object (JSON array)")
    m.add_argument("--health", help="the health overlay, when PATH is the catalog object")
    m.add_argument("--hosts", action="store_true", help="name the hosts holding drift holds")
    m.add_argument("--json", action="store_true", help="print the report as JSON")

    e = sub.add_parser("exposure", help="offline: served by the default listing before and after 2.47.0")
    e.add_argument("path", help="a snapshot from this script, or the catalog object (JSON array)")
    e.add_argument("--health", help="the health overlay, when PATH is the catalog object")
    e.add_argument("--terms", help="the observed-terms overlay, when PATH is the catalog object")
    e.add_argument("--window", type=int, default=FRESH_WINDOW_SECS,
                   help="DISCOVERY_TERMS_FRESH_SECS (seconds)")
    e.add_argument("--now", type=int, help="evaluate at this unix time (default: the snapshot's, else now)")

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

    if args.cmd == "exposure":
        now = args.now
        if now is None:
            with open(args.path, encoding="utf-8") as f:
                doc = json.load(f)
            now = doc.get("capturedAt") if isinstance(doc, dict) else None
            now = now or int(time.time())
        report = exposure_report(load_records(args.path, args.health, args.terms), now, args.window)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0

    if args.cmd == "compare":
        a = json.load(open(args.first))
        b = json.load(open(args.second))
        for name, snap in ((args.first, a), (args.second, b)):
            if snap.get("pendingIncluded") is False:
                print(f"note: {name} holds only the exposed listings (taken without "
                      "BAZAAR_ADMIN_TOKEN); the groups compare what both snapshots hold",
                      file=sys.stderr)
        c = compare(a, b)
        print_comparison(c)
        if args.by_method:
            print_by_method(c)
        return 0

    if args.cmd == "methods":
        report = methods_report(load_records(args.path, args.health))
        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True, default=str))
        else:
            print_methods(report, args.hosts)
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
