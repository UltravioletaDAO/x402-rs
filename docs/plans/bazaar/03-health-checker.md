# WS-B — Health Checker (Pre-Ping / Liveness Prober)

**Ships as**: v1.52.0 · **Depends on**: WS-A (snapshot-persistence hygiene §2.7 — health lives in a separate overlay so imports can't clobber it; the real dependency is write-path hygiene, not clobbering) · **Unblocks**: WS-C full ordering, WS-D health badges, WS-E attestations
**Effect**: default listing shrinks to ~4,600–5,000 **verified-alive** items. **The day-1 shrink is driven by two mechanisms, not by quarantining** (which takes weeks under M=3 weekly hysteresis): (1) **probation gating** — payai/anyspend/aurracloud items stay hidden as `unknown` until their first successful 402 probe (~20k hidden the moment v1.52.0 deploys); (2) the initial full sweep promoting the ~5k that answer 402. Standard/trusted-source dead items (thirdweb/coinbase, ~500–600) follow the normal hysteresis and are quarantined over ~2–3 weeks unless the bootstrap rule (§2) is enabled.
**Security**: implements the probe half of `08-security-hardening.md` — SSRF connector (F2), manual redirects (F3), per-destination-IP bucketing + global rps (F7), port allowlist (F16), sha256 overlay keys (F9). **F2/F3 are blockers — the prober fetches attacker-supplied URLs; without the hardened connector it will read AWS task-role credentials.** Read 08 §2/§3/§7/§9/§16 before implementing.

## 1. Probe semantics — "402 is the healthy signal"

Per x402scan's proven implementation and the protocol's own logic: an x402 resource is UP iff it answers **HTTP 402 with a parseable x402 body** (coherent `x402Version` + non-empty `accepts`). Everything else is a degraded/dead signal for a *payment* catalog.

| Probe outcome | Classification | Action |
|---|---|---|
| 402 + parseable x402 body | `alive` | promote/keep; refresh stored accepts from body (see §4 payTo drift) |
| 402, body unparseable | `degraded` | keep visible, flag; count toward failure hysteresis |
| 200/201 (no payment enforcement) | `degraded` | listed as x402 but doesn't challenge — visible w/ flag, candidate for quarantine after M cycles |
| 401/403/405/415 | `auth_gated` | **not a failure**: auth precedes payment on some products (verified live: Execution Market `POST /api/v1/tasks` returns 401 before its documented 402). Keep visible with flag |
| 429 | `alive` (throttled) | honor `Retry-After`, do not count as failure |
| 404/410 | `resource_missing` | count toward quarantine (dominant failure mode: 62% of catalog) |
| timeout / DNS / refused / 5xx | `dead` | count toward quarantine |
| URL has routeTemplate placeholders (R6-tagged), or `type=a2a` GET-unfriendly | `unprobeable` | never quarantined by probing; **excluded from the 15-min schedule**; visibility governed by static filters + settlement activity only |
| `type=mcp` (Streamable HTTP) | probe via **MCP handshake** | GET/POST won't 402; instead POST a JSON-RPC `initialize` — a valid MCP response counts as `alive`. Covers Execution Market + 402Milly's listed MCP endpoints (else our own flagships show forever-grey — completeness #8). If handshake unimplemented, fall back to `unprobeable` + manifest `expectedStatus` |

Method: **GET first, then POST with `Content-Type: application/json` and empty-object body ONLY if GET yields 404/405** (never speculative — F7). x402scan probes GET then POST; many payable routes are POST-only (402Milly's `/purchase`). Never HEAD. Never attach payment. Timeouts: 5s connect / 12s total. UA: `uvd-bazaar-health/1.0 (+https://facilitator.ultravioletadao.xyz)`. **Manifest first-party/VIP entries may declare `expectedStatus: auth_gated|alive|mcp`** so the UI renders their known-healthy state (Execution Market REST is auth-before-402 → `auth_gated` is its healthy state, not a failure).

**SSRF defense is a connector, not prose — see `08-security-hardening.md` §2/§3 (F2/F3) for the required implementation**: custom DNS resolver that rejects if ANY resolved A/AAAA is disallowed (mixed answers = attack), pins the socket to the checked IP (no re-resolve at connect), `redirect(Policy::none())` + manual ≤3-hop follow re-running the full check each hop, port allowlist {80,443,8080,8443} (F16). Extend `is_disallowed_target_ip` (`src/discovery.rs:652-722`) for `240.0.0.0/4`, `192.88.99.0/24`, and IPv4-mapped IPv6 (08 §2.3). The same hardened connector is **mandatory** (not "while we're there") for the aggregator + crawler clients (08 §15/F15).

## 2. State machine (hysteresis — never flip on one probe)

```
          K=2 consecutive alive-class probes
   ┌──────────────────────────────────────────────┐
   ▼                                              │
 ALIVE ──M=3 consecutive fail-class probes──▶ QUARANTINED
   ▲                                              │
   │        30 days continuously dead             ▼
 (new item: UNKNOWN — visible if source            REMOVED
  trust ≠ probation; first probe decides)   (unregister + tombstone 90d)
```

- Fail-class = `resource_missing` | `dead`. `degraded`/`auth_gated` are visible-with-flag and only quarantine after M **consecutive** `degraded` cycles combined with zero settlement activity.
- Backoff for quarantined: 1h → 6h → 24h → 72h capped, ±20% jitter. Recovery needs K=2 consecutive alive probes → back to ALIVE (fresh hysteresis counters).
- **Flap detection**: >4 state transitions in 7 days → pin QUARANTINED for 7 days regardless of instantaneous state.
- **Passive health**: a successful settlement through us counts as an alive probe (strongest possible evidence, zero cost) — resets failure counters and defers the next scheduled probe. **Wiring caveat (feasibility #3)**: today `track_settlement` (`src/discovery.rs:550-594`) is invoked ONLY when `payment_requirements.extra.discoverable == true` (`src/handlers.rs:2014-2023`), so it fires for ~0% of the aggregated catalog. WS-B must add a settlement→health hook that fires for **all** successful settlements whose v1 `payment_requirements.resource` URL matches a listed URL (independent of `discoverable`). This hook updates the health overlay, NOT the resources store (no `discoverable` auto-registration for aggregated items). Until this hook exists, do not gate any rule on "zero settlement activity" — treat absence of settlement data as neutral, not as a failure signal.
- Tombstone on removal: keep `(url, removedAt, reason)` for 90 days so a re-import of a dead URL from a feed doesn't resurrect it into UNKNOWN visibility; a live probe can still resurrect it deliberately. Removal threshold: **30 days continuously dead** → `unregister` + 90-day tombstone (this supersedes the "30-90 days" range in 00 §3).
- **Initial-sweep bootstrap rule (opt-in, `DISCOVERY_HEALTH_BOOTSTRAP=true` for the first deploy)**: during the very first full sweep only, a `resource_missing`/`dead` result quarantines at **M=1** (provisional), confirmed at the next cycle. This clears the ~500–600 standard-source dead items (thirdweb/coinbase) in days instead of the ~3 weeks that M=3 weekly hysteresis would take. Without it, those items stay visible-but-flagged for ~3 weeks — an acceptable but slower path. Pick one deliberately; the header "Effect" assumes bootstrap ON.

## 3. Cadence & budget

| Tier (from WS-C) | Cadence | Population | Load |
|---|---|---|---|
| first_party + vip | every 15 min, **capped per-origin** | ~10–150 URLs | negligible for us; see partner note |
| verified/listed tail | every 7 days (spread uniformly) | ~21k after WS-A | ~2.1 probes/min average |
| probation sources (unproven) | probe-on-import + weekly | payai bulk | bounded by global cap |
| unprobeable (templates, a2a) | never probed | 380+ | 0 |
| quarantined | per-backoff schedule | ~500–600 standard + payai fails | decaying |

**Per-origin cadence collapse (completeness #15)**: the 15-min VIP cadence is per *listing*, but Tenjin has 121 items on one origin → 121×96/day ≈ 11.6k req/day at `tenjin.blog`. Negligible for us, rude to the partner. Cap VIP probes at **N per origin per cycle** (e.g. 5, rotating through the origin's URLs) so a many-item partner origin sees a bounded, sampled probe rate. The per-origin *concurrency* cap (§below) does not bound *volume* — this does.

Note the "~16k initially" figure from earlier drafts was wrong: those items are probation-hidden `unknown`s (never earned a first probe), not `quarantined`. Most never transition to `quarantined` at all — they simply stay hidden until/unless a probe promotes them.

Scheduling: a `tokio` task (`start_health_task`, mirroring `start_aggregation_task` wiring at `src/main.rs:222-297`) wakes every `DISCOVERY_HEALTH_TICK` (default 60s), pulls the due set (next_probe_at ≤ now), groups **by resolved destination IP /24** (NOT hostname — many attacker hostnames can resolve to one victim IP; F7), and probes with global concurrency `DISCOVERY_HEALTH_CONCURRENCY` (default 15), **max 2 in-flight per destination /24**, plus a **global outbound rate limit** `DISCOVERY_HEALTH_MAX_RPS` (default 20). Honor `Retry-After` on 429. Unknown/new aggregation sources default to `probation`, not `standard` (F7).

**Initial full sweep**: on first deploy nothing has health state; the due set is the whole catalog. At 15-way concurrency ≈ 3–4 hours (measured basis: audit probe). Run it as the normal task simply draining a large backlog — no special code path. Order the backlog: first_party/vip → self_registered → questflow → thirdweb/coinbase → payai/aurracloud, so the valuable listings get badges first.

## 4. Security probes (from "Five Attacks on x402")

On every `alive` probe, diff the live 402 body's accepts against the stored listing:
- **`payTo` changed** → security event: quarantine immediately (skip hysteresis), keep the old record, log at WARN with both addresses, increment `bazaar_paytoswap_total`. Auto-unquarantine only after the new payTo has been stable for 7 days (a legit rotation) — or manual admin release (WS-C admin API).
- `asset`/`network` changed → update stored accepts (normal evolution), log at INFO.
- Amount changed → update; track for the UI's price display.

## 5. Storage — health overlay (NOT on the resource struct)

WS-A's merge fix protects inline fields, but health writes are high-frequency and must not contend with `bazaar/resources.json` at all:

- In-memory: `HashMap<String /*url*/, HealthState>` behind its own `RwLock` inside a new `HealthTracker` (`src/discovery_health.rs`).
- Persisted: **separate S3 object** `bazaar/health.json` (same bucket — IAM already grants `bucket/*`, terraform `main.tf:633-655`), written as a **debounced full snapshot** every `DISCOVERY_HEALTH_PERSIST_SECS` (default 300) *only if dirty*, plus one final write on graceful shutdown. Loss tolerance: worst case we re-probe 5 minutes of results — acceptable by design.
- `HealthState`: `{ status, last_checked, http_status, latency_ms, consecutive_fails, consecutive_ok, transitions_7d, next_probe_at, quarantined_at?, last_alive_at?, tombstone?, probed_accepts?, probed_accepts_at?, daily_history }`.
- **`daily_history` (completeness #4)**: a 90-slot ring buffer of `{day, ok, fail}` counters. Required so WS-C's "30-day uptime ≥ 99%" VIP bar and WS-E's on-chain `uptimeBps` can be computed — instantaneous counters alone cannot express a 30-day window. Must land in WS-B or the data won't exist when later phases need it.
- **Probe-observed accepts precedence (feasibility #8)**: an `alive` probe's 402 body carries the live `accepts`. Store them as `probed_accepts` **in the health overlay**, with `probed_accepts_at`. On read, `probed_accepts` (when fresher than the resource's feed `last_updated`) is the source of truth for display — this resolves the contradiction where a stale feed re-import would otherwise overwrite probe-refreshed payTo and then trip the drift alarm on the next probe (oscillating quarantine). The resources store is **not** written by the prober (keeps §5's "no contention" claim literally true); drift detection (§4) compares live-402 vs `probed_accepts` baseline, and for manifest URLs vs `expectedPayTo`.
- GC: health entries whose URL left the registry are dropped at snapshot time.

## 6. Read-side integration

- `DiscoveryResponse` items gain an optional `health` object (`#[serde(skip_serializing_if = "Option::is_none")]`, additive — existing clients unaffected): `{"status": "alive|degraded|auth_gated|unknown|quarantined", "lastChecked": 1784800000, "latencyMs": 240}`.
- **Visibility is a `VisibilityPolicy` in `list()`, NOT a filter param (feasibility #5)**: the no-query-params request — the exact default view this workstream is about — returns `Option<DiscoveryFilters> = None` and short-circuits `matches_filters` (`handlers.rs:254-275` → `discovery.rs:456-458`), so quarantine/probation/suppression hiding CANNOT ride on `matches_filters`. Implement a `VisibilityPolicy` applied in `list()` independent of the user filter: default hides `quarantined`, probation-source `unknown`, and suppressed URLs. `health=any` (and admin views) opt out. Update tests that assume unfiltered listing (e.g. `test_facilitator_resource_type`).
- **Locking (feasibility #2 — this is where the project shipped a prod bug of the same shape, v1.49.2)**: `list()` holds `resources.read().await` (`discovery.rs:326`) while calling the SYNC `matches_filters`/sort. The `HealthTracker` is behind its own `tokio::RwLock` — reading it inside `list()` would hold the resources guard across `.await` (the exact guard-across-await hazard). Required design: **snapshot a small status-only map from the HealthTracker (clone / `ArcSwap`) BEFORE acquiring the resources guard**, and pass it (plus a precomputed per-item tier map) into the sync filter/sort. Never `.await` inside the resources-guarded section. The passive-health settlement hook (§2) has the same constraint — it runs under `resources.write().await` (`discovery.rs:556`); it must write the health overlay via a channel/spawn, not by awaiting a lock inside the guard.
- Query param `health=` accepts **any status value** (`alive`, `degraded`, `auth_gated`, `unknown`, `quarantined`) plus `any` (completeness #9), so the UI's health filter pills work. `health=any` restores today's full view for debugging.
- `pagination.total` reflects the filtered view (total is computed post-filter, `discovery.rs:340`) — correct, but note it now also reflects the `VisibilityPolicy`, not just user filters.
- OpenAPI: document `health` param + response field (`src/openapi.rs:1004-1046` — being rewritten anyway, see WS-C).
- **Near-dupe note**: WS-A defers near-dupe (242 trailing-slash/query variants) collapse to here — no explicit dedup pass is needed; the prober quarantines the dead variants organically, and `canonical_url` (08 §0) already collapses trailing-slash/default-port variants at the key level.

## 7. Env vars

| Var | Default | Meaning |
|---|---|---|
| `DISCOVERY_ENABLE_HEALTH` | `true` | kill-switch (`ENABLE_REGISTER_RECOVERY` precedent). **When false, `probation` degrades to `standard`** (items visible) — otherwise probation items could never earn their promoting probe and ~80% of the catalog would vanish with no path back (completeness #10) |
| `DISCOVERY_HEALTH_TICK` | `60` | scheduler wake interval (s) |
| `DISCOVERY_HEALTH_CONCURRENCY` | `15` | global concurrent probes |
| `DISCOVERY_HEALTH_MAX_RPS` | `20` | global outbound probe rate cap (F7) |
| `DISCOVERY_HEALTH_TAIL_DAYS` | `7` | tail sweep period |
| `DISCOVERY_HEALTH_PERSIST_SECS` | `300` | overlay snapshot debounce |
| `DISCOVERY_HEALTH_BOOTSTRAP` | `true` (first deploy) | M=1 provisional quarantine during the initial sweep (§2) |

## 8. Files touched

| File | Change | Est. LOC |
|---|---|---|
| `src/discovery_health.rs` (new) | `HealthTracker`, prober, state machine, scheduler, security diff | ~450 |
| `src/discovery_store.rs` | generic keyed-overlay S3 snapshot helper (reused by curation state) | ~60 |
| `src/discovery.rs` | health-aware `matches_filters` + default exclusion; settlement→passive-health hook | ~50 |
| `src/types_v2.rs` | `health` response field + `HealthStatus` enum | ~40 |
| `src/handlers.rs` | `health` query param | ~10 |
| `src/main.rs` | task wiring + env | ~25 |
| `src/openapi.rs` | param + field docs | ~15 |
| terraform | env vars (no infra changes) | ~10 |

## 9. Tests

- State machine unit tests: hysteresis (2 fails ≠ quarantine, 3 = quarantine), recovery K=2, backoff progression, flap pinning, settlement-as-alive reset, tombstone resurrection rules.
- Probe classification table-driven test (every row of §1).
- payTo-drift → immediate quarantine + WARN.
- SSRF: probe target resolving to `169.254.169.254`/`10.x` refused (mock resolver).
- Overlay snapshot: dirty-flag debounce; GC of departed URLs.
- No live-network tests in CI — probe transport mocked (`--test-threads=1` convention).

## 10. Verification after deploy

```bash
# health fields appear
curl -s '.../discovery/resources?limit=1' | jq '.items[0].health'
# default view is alive-only; totals drop to ~5k
curl -s '.../discovery/resources?limit=1' | jq .pagination.total            # ~4600-5000 after sweep completes
curl -s '.../discovery/resources?limit=1&health=any' | jq .pagination.total # ~21700
# sweep progress in logs
aws logs tail /ecs/facilitator-production --since 1h | grep "health_sweep"
```
