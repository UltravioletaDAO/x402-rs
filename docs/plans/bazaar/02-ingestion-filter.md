# WS-A — Ingestion Filter (Import Curation)

**Ships as**: v1.51.0 · **Depends on**: nothing · **Unblocks**: WS-B (merge changes), WS-C
**Effect**: catalog drops 26,233 → **~21,700** at import time (junk minus retained spec-legal templates, minus empty-accepts); junk and unpayable items never enter; churn bug fixed; local state survives re-imports.
**Security**: this workstream implements the ingest half of `08-security-hardening.md` — timestamp clamp (F5), length caps R7 (F12), control-char sanitization (F14), the shared `canonical_url` primitive (§0), and the hardened aggregator/crawler HTTP connector (F15). Read 08 alongside this doc.

## 1. Goals

1. Nothing enters the registry without at least one **valid, payable** `accepts` entry (network parses to CAIP-2, amount > 0, plausible asset/payTo).
2. Junk URLs (fake schemes, localhost/private IPs, hostless, malformed) are rejected at conversion time.
3. Per-source policy is **config-driven** (`config/bazaar_sources.json`), not hardcoded — sources can be disabled, capped, or marked probe-gated without a code change.
4. Re-imports **merge**, not replace: `first_seen`, `settlement_count` (and later health/curation) survive.
5. A **retention GC** removes already-stored items that fail the new rules (one-time cleanup of the existing 26k + ongoing hygiene).

## 2. Design

### 2.1 `ImportPolicy` — replace the `skip_validation` boolean

Today: `bulk_import(resources, skip_validation: bool)` (`src/discovery.rs:379-448`), called with `true` by both feeders (`src/discovery_aggregator.rs:884`, `src/discovery_crawler.rs:291`).

Replace the boolean with a policy enum so intent is explicit and future-proof:

```rust
pub enum ImportPolicy {
    /// POST /discovery/register — full validate_resource() (strict, 4xx on failure)
    Strict,
    /// Aggregator/crawler — apply CurationFilter, silently drop failures (log + count)
    Filtered,
}
```

`bulk_import(resources, ImportPolicy::Filtered)` runs each resource through `CurationFilter::check()` (below) and drops failures with a per-rule counter (exposed as tracing fields + Prometheus counters, see 06 §5). `track_settlement` (`src/discovery.rs:550-594`) also moves to `Filtered` — settlement-sourced entries currently bypass all validation too.

### 2.2 `CurationFilter` rules (evaluated in order, first failure wins)

Rejection counts below are **marginal** (items each rule is the *first* to reject, "first failure wins") — they do not double-count. R2's hostless bucket, for example, is almost entirely `monopoly://` items already rejected by R1, so R2's marginal count is small.

| # | Rule | Marginal rejects | Notes |
|---|---|---|---|
| R1 | URL scheme is `http`/`https` | 2,817 (`monopoly://`, `transfer://`, `solana-transfer://`, …) | scheme check is `validate_resource` `discovery.rs:599-605` — reuse |
| R2 | Host is public: not IP-literal-private per `is_disallowed_target_ip` (`discovery.rs:652-722`), host has a `.` or is a public IP | 52 localhost/private + ~10 hostless-not-already-R1 | reuse existing guard; **NOT the SSRF boundary** (see 08 §2) |
| R3 | ≥1 valid accepts entry after conversion: CAIP-2 network, `amount > 0`, parseable `asset`/`payTo` | ~1,639 net empty-accepts | exception: `type == "facilitator"` may have empty accepts (existing rule, `discovery.rs:631-633`) |
| R4 | URL sanity: no whitespace, no userinfo (reject any `@` in authority, not just `user:pass@`), length ≤ 2048 | ~10 | userinfo rejection is a security requirement (08 §0/F1/F13) |
| R5 | `type` ∈ {`http`, `mcp`, `a2a`, `facilitator`} | few | existing whitelist (`discovery.rs:624`) |
| R6 | **Template URLs (`/:param`, `{}`, `%7B`) are ALLOWED** but tagged `route_template: true` | 0 rejected (380 tagged, kept) | spec-legal per `bazaar.md` routeTemplate; prober treats them as `unprobeable`, not dead |
| R7 | Length caps: `description` ≤2 KiB, each tag ≤64, ≤20 tags, `provider`/`category` ≤128 | truncate-or-reject | anti-bloat (08 §12/F12); no caps today (`discovery_aggregator.rs:739,746-751`) |

Zero-amount note (R3): the aggregator currently coerces unparseable amounts to `0` (`discovery_aggregator.rs:775-778`). Change to reject the entry instead — a zero-price entry is either broken data or a free endpoint that doesn't belong in a payment catalog. If ALL entries of a resource are rejected, the resource is dropped (today it is imported with `accepts: []`, `discovery_aggregator.rs:711-755` — this is the single biggest correctness fix).

**Cascade recomputed with R6 template-retention** (the audit's raw junk union is 3,269; keeping 380 spec-legal templates means junk-dropped ≈ 2,889):

| Stage | Remaining | Removed |
|---|---|---|
| Raw catalog | 26,233 | — |
| Drop junk (R1/R2/R4/R5, templates kept) | ~23,344 | −2,889 |
| Drop unpayable (R3 marginal empty-accepts) | **~21,700** | −1,639 |

The verification gates in §5 test **direction + invariants** (empty-accepts == 0, no bad schemes, total materially reduced into the 21k–22k band), NOT a brittle exact number.

Where the rules live: extract the reusable checks from `validate_resource` into `CurationFilter` in a new `src/discovery_curation.rs` (shared by strict and filtered paths) so there is exactly one implementation of each rule.

### 2.3 Per-source policy — `config/bazaar_sources.json`

Wire via the existing-but-unused `DiscoveryAggregator::with_facilitators()` (`src/discovery_aggregator.rs:526`).

**Config-loading decision (be explicit — no exact precedent exists):** `config/blacklist.json` is loaded at **runtime** via `fs::read_to_string` in `src/blocklist.rs`, NOT via `include_str!` (the feasibility review corrected this — there is no `include_str!` of any config JSON in the repo). Choose deliberately: adopt the **same runtime-file pattern** as blacklist (hot-swappable without rebuild; requires `config/` to be present in the Docker image — verify the Dockerfile COPYs it) with env override `BAZAAR_SOURCES_PATH` (default `config/bazaar_sources.json`). This is the recommended choice — it matches the existing blacklist mechanism and lets ops disable a misbehaving source without a full rebuild. (`include_str!` embed is the alternative; reject it here because per-source enable/disable is exactly the kind of thing ops needs to change fast.)

```jsonc
{
  "sources": [
    { "id": "coinbase",   "enabled": true,  "trust": "standard" },
    { "id": "payai",      "enabled": true,  "trust": "probation", "maxItems": 30000 },
    { "id": "thirdweb",   "enabled": true,  "trust": "standard" },
    { "id": "questflow",  "enabled": true,  "trust": "trusted" },
    { "id": "aurracloud", "enabled": false, "trust": "probation", "note": "77% dead hosts (audit 2026-07-23); re-enable after they clean up" },
    { "id": "anyspend",   "enabled": true,  "trust": "probation" },
    { "id": "openx402",   "enabled": false, "note": "TLS cert mismatch — broken endpoint" },
    { "id": "x402rs",     "enabled": false, "note": "endpoint now returns HTML — find new URL or drop" },
    { "id": "heurist",    "enabled": true,  "trust": "standard" },
    { "id": "polymer",    "enabled": true,  "trust": "standard" },
    { "id": "meridian",   "enabled": true,  "trust": "standard" },
    { "id": "virtuals",   "enabled": false, "note": "404 NotFoundError — broken endpoint" }
  ]
}
```

Trust levels (consumed by WS-B/WS-C):
- `trusted` — listed immediately, probed on the normal tail cadence (audit: questflow 100% alive).
- `standard` — listed immediately, probed on the normal tail cadence.
- `probation` — imported but **hidden from default listing until first successful 402 probe** (`health.status == alive` promotes them). This is the payai gate: their ~20% alive-rate means ~80% of their items would otherwise pollute the default view between import and first probe. Requires WS-B; until WS-B ships, `probation` behaves as `standard` (items visible) — acceptable for the one release gap, since WS-A already removed the junk.

URLs stay hardcoded next to their ids in `FacilitatorConfig` — the config toggles/annotates them. Unknown ids in config → startup warning; sources in code but missing from config → default `standard`/enabled (fail-open so a config mistake doesn't silently empty the bazaar).

### 2.4 Field-preserving merge in `bulk_import`

Replace whole-struct replacement (`src/discovery.rs:403-417`) with:

```rust
// incoming wins: url(key), resource_type, description, accepts, metadata, source_facilitator, last_updated
// existing wins: first_seen (min of both), settlement_count (max of both)
// source: existing wins if it is self_registered/settlement (NEVER downgrade to aggregated on collision) — 08 §4/F4
// manifest-matched URLs (first_party/vip): accepts/payTo are AUTHORITATIVE FROM THE MANIFEST —
//   an aggregated import may NOT mutate them; log an alert if a feed tries (08 §4/F4)
// never touched by imports: health overlay, curation overlay (separate stores, WS-B/C)
// control chars in description/url stripped before storage (08 §14/F14)
```

Keep the "newer `last_updated` wins" trigger for feed fields, but merge instead of replace. This also fixes: `settlement_count` clobbered to `None` by aggregated overwrites, and `first_seen` resetting on every winning re-import. **Security-critical**: the `source` no-downgrade rule and the manifest-authoritative rule prevent a hostile feed from hijacking a first-party/VIP listing's payTo via URL collision (08 §4) — do not omit them.

### 2.5 Churn fix + timestamp clamp (F5)

`convert_single_resource` sets `last_updated = now` for sources that omit timestamps (`discovery_aggregator.rs:728-733`), so those records always win the merge race and are re-persisted every hour. **The fix cannot live in `convert_single_resource`** — the aggregator holds no registry reference, and `last_updated` is a bare `u64` (`types_v2.rs:1029`) so the "was-synthetic" bit is already destroyed there. Plumbing + logic:

1. **Plumb the missing-timestamp flag**: `from_aggregation` takes `Option<u64>` (raw upstream value); add a `timestamp_synthetic: bool` (or keep `Option` through to the merge). This adds `src/types_v2.rs` to WS-A's file-touch list (§3).
2. **Clamp on ingest (F5)**: `last_updated = min(upstream_value, now + 300s)`; reject records whose upstream `last_updated > now + 300s` as malformed (`bazaar_import_dropped_total{rule="future_timestamp"}`); floor negatives. This defeats the far-future-timestamp poisoning attack (08 §5).
3. **Churn/no-op in the merge** (`bulk_import`, not the aggregator): when incoming is synthetic-timestamped and an existing record for the same URL exists, inherit the existing `last_updated` and **skip the write entirely if content is unchanged** (compare a content hash of `(resource_type, description, accepts, metadata)`). Only genuinely-changed records persist.

### 2.6 Retention GC

New `DiscoveryRegistry::apply_retention(&self, filter: &CurationFilter)`:
- Runs once at startup after `load_all()` (this is the **one-time cleanup of the existing 26k**), then at the end of every aggregation cycle.
- Removes stored items that fail R1–R5 (i.e., rules changed → old junk leaves).
- Removes items whose source is `enabled: false` **only when** `purgeOnDisable: true` is set for that source in config (default false — a disabled source stops updating but its items persist and decay via WS-B health, avoiding mass-delete on transient upstream breakage).
- Uses one snapshot `save_all` at the end instead of per-item deletes (avoids N full-file S3 rewrites, `discovery_store.rs:283-287`).

### 2.7 S3 write path hygiene (prerequisite for WS-B)

`bulk_import` currently persists each changed resource with its own full-file GET+PUT (`discovery.rs:424-438`). Change to: mutate cache, then **one `save_all` snapshot per aggregation cycle** (`S3Store::save_all` is at `discovery_store.rs:289-311`; `save()` at `:267-281` is the per-item read-modify-write we are replacing). This collapses hundreds of racy writes per cycle into one, and is the pattern the health prober will also use for its own overlay object.

### 2.8 Aggregator pagination fix (promised in 00 §8)

`fetch_from_facilitator` exits the page loop on `batch_count < 100 || offset >= total` with `total = pagination.total.unwrap_or(0)` (`discovery_aggregator.rs:614-618`) — a source returning items but **no `pagination` block** yields only its first page forever. Fix: when pagination metadata is absent, continue while `batch_count == limit` (full page = probably more), with a hard `max_pages` safety cap (e.g. 500 → 50k items) to bound a misbehaving/hostile source. Counts toward the per-source `maxItems` cap (§2.3).

## 3. Files touched

| File | Change | Est. LOC |
|---|---|---|
| `src/discovery_curation.rs` (new) | `CurationFilter`, rules R1–R7, `canonical_url` (08 §0), source-config types + loader | ~320 |
| `config/bazaar_sources.json` (new) | per-source policy | ~40 |
| `src/discovery.rs` | `ImportPolicy`, merge-not-replace (+ F4 no-downgrade/manifest-authoritative), `apply_retention`, snapshot persistence, extend `is_disallowed_target_ip` (08 §2.3) | ~150 |
| `src/discovery_aggregator.rs` | reject-not-zero amounts, drop-not-import empty accepts, timestamp clamp+plumbing, pagination fix, `with_facilitators()` wiring, hardened connector (08 §15), per-source counters | ~130 |
| `src/discovery_crawler.rs` | `ImportPolicy::Filtered` + hardened connector (08 §15) | ~30 |
| `src/types_v2.rs` | `from_aggregation` takes `Option<u64>` + synthetic-ts flag; `route_template` field | ~30 |
| `src/handlers.rs` | `track_settlement` → `Filtered` | ~5 |
| `src/main.rs` | config load + wiring | ~20 |

## 4. Tests

- Unit: each rule R1–R6 accept/reject cases (incl. `monopoly://`, `http://gittipstream:8080`, `localhost`, template URLs tagged-not-dropped, zero-amount rejection, facilitator-type empty-accepts exception).
- Unit: merge preserves `first_seen`/`settlement_count`; timestamp-less re-import with unchanged content does not persist.
- Unit: source config — disabled source skipped; unknown id warns; missing id defaults standard.
- Integration (offline, `MemoryStore`): feed a captured payai page fixture (real junk from the audit) through `bulk_import(Filtered)` → assert counts match the audit cascade proportions.
- Regression: `POST /discovery/register` behavior unchanged (Strict path).
- **CI note**: tests run `--test-threads=1` (project convention).

## 5. Verification after deploy

```bash
# junk schemes gone from the catalog
curl -s '.../discovery/resources?limit=100&offset=0' | jq '[.items[].url | select(startswith("http") | not)] | length'  # 0
# no empty-accepts listings (except type=facilitator)
curl -s '.../discovery/resources?limit=100' | jq '[.items[] | select(.type != "facilitator" and (.accepts|length)==0)] | length'  # 0
# total dropped to ~21k
curl -s '.../discovery/resources?limit=1' | jq .pagination.total  # ~21700
# aggregation logs show per-rule drop counters
aws logs tail /ecs/facilitator-production --since 1h | grep "curation_filter"
```

## 6. Open decisions (deliberate, not blockers)

1. **payai `maxItems` cap**: 30k is a no-op guard today; tighten only if they balloon. The real payai control is `probation` + WS-B probe-gating.
2. **Non-TLS (`http://`) URLs**: NOT rejected (11.5% of catalog; some legit dev endpoints). WS-C surfaces a `nonTls: true` flag for the UI to badge; revisit rejection later.
3. **Near-dupes** (242 items, trailing slash/query variants): normalize-and-collapse deferred to WS-B (the prober will kill the dead variants anyway).
