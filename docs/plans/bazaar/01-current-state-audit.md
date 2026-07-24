# Current State Audit — Bazaar (Evidence Base)

**Date**: 2026-07-23 · Live production snapshot: 26,233 items from `https://facilitator.ultravioletadao.xyz/discovery/resources` (263 pages, API caps `limit` at 100). Probe: stratified 300-URL sample, single-pass GET, 8s timeout, 15-way concurrency, UA `uvd-bazaar-health/1.0`.

All `file:line` references are against `main` at v1.50.1.

## 1. Code facts (what the system does today)

### Data flow
- **Aggregator** (`src/discovery_aggregator.rs`): 12 hardcoded sources (`FacilitatorConfig::all()` `:386-401`), all enabled, no env override (`with_facilitators()` `:526` exists but is never called). Runs at startup then every `DISCOVERY_AGGREGATION_INTERVAL` (default 3600s). Prod: enabled.
- **Crawler** (`src/discovery_crawler.rs`): fetches `{seed}/.well-known/x402`; default **disabled**, no targets configured in prod.
- **Registry** (`src/discovery.rs`): in-memory `HashMap<url-string, DiscoveryResource>` behind `RwLock`, persisted to a **single JSON array** at `s3://facilitator-discovery-prod/bazaar/resources.json` (`src/discovery_store.rs:205-206`, terraform `main.tf:815-821`).

### The five structural defects

1. **Validation bypassed on both feed paths**: `bulk_import(resources, /*skip_validation=*/true)` at `src/discovery_aggregator.rs:884` and `src/discovery_crawler.rs:291`. The existing `validate_resource` (`src/discovery.rs:597-637` — http/https scheme check, SSRF IP-literal guard `is_disallowed_target_ip` `:652-722`, type whitelist, non-empty-accepts rule) only runs on `POST /discovery/register`. `track_settlement` (`src/discovery.rs:550-594`) also performs no validation. A resource whose `accepts` entries ALL fail conversion is still imported with `accepts: []` (`discovery_aggregator.rs:711-755`).
2. **Whole-struct "newer wins" merge**: same URL re-imported replaces the entire record when incoming `last_updated > existing` (`src/discovery.rs:403-417`) — `first_seen`, `settlement_count`, and any future curation/health fields stored inline are clobbered.
3. **Perpetual-churn bug**: sources omitting `last_updated` get `now` at every fetch (`discovery_aggregator.rs:728-733`), always winning the timestamp race → re-persisted every cycle, each a full-file S3 GET+PUT (`discovery.rs:424-438`).
4. **No liveness, no expiry, no removal path**: nothing probes listed URLs; `DiscoveryStore::health_check` has zero call sites; entries never expire; `unregister()`/`update()` (`discovery.rs:250,285`) are not exposed over HTTP (the 409 hint at `handlers.rs:375` references a `PUT` route that does not exist).
5. **Flat ordering**: `list()` sorts `last_updated DESC` only (`discovery.rs:337-338`), page cap 100 (`:329`). `settlement_count` is a latent reputation signal, unused.

### Existing hooks we will reuse
- Filters already implemented server-side: `category`, `network` (CAIP-2 exact), `provider`, `tag`, `source`, `sourceFacilitator` (`DiscoveryQueryParams` `handlers.rs:220-248`, `matches_filters` `discovery.rs:451-530`).
- Background-task pattern: `start_aggregation_task` / `start_crawl_task` wired at `main.rs:222-297`.
- SSRF guard: `is_disallowed_target_ip` (`discovery.rs:652-722`).
- Payment-path OFAC blacklist (`src/blocklist.rs`, `config/blacklist.json`) is **not** used by discovery — but its compile-time-config pattern is the template for the curation manifest.
- OpenAPI Bazaar docs are stale/wrong (`src/openapi.rs:1004-1085`): document a `type` param that doesn't exist, omit the real params, and show a v1-shaped response. Must be rewritten during WS-C/D.
- Aggregator pagination quirk: sources returning no `pagination` block only ever yield their first page (`discovery_aggregator.rs:614-618`).

## 2. Live data quality (full 26,233-item snapshot)

| Metric | Value | % |
|---|---|---|
| Total items | 26,233 | 100% |
| `source=aggregated` | 26,105 | 99.5% |
| `source=self_registered` | 128 | 0.5% |
| `settlement` / `crawled` | 0 | — |
| Empty `accepts` (structurally unpayable) | 4,026 | 15.3% |
| Junk URLs (union of ≥1 flag) | 3,269 | 12.5% |
| — fake non-http schemes (`monopoly://` 2,318, `transfer://` 277, `solana-transfer://` 212, …) | 2,817 | 10.7% |
| — template placeholders (`/:var` 296, `{}`/`%7B` 84) — **spec-legal routeTemplates, not junk per se** | 380 | 1.4% |
| — localhost / private IP | 52 | 0.2% |
| `http://` (non-TLS) | 3,013 | 11.5% |
| Empty description | 26,105 | 99.5% (all aggregated; all 128 self-registered have one) |
| Not updated in >30 days | 22,652 | 86.3% |
| Exact duplicate URLs | 0 | near-dupes: 82 groups / 242 items |

Networks: 16 distinct; Base (`eip155:8453`) is 99.0% of all 22,222 accepts entries. Everything else is noise-level (Ethereum 56, Arbitrum 45, Polygon 33, Avalanche 4, …).

## 3. Health probe results (n=300 stratified, extrapolated)

Classes: `ALIVE_X402`=402 · `ALIVE`=200/201/4xx-not-404 · `RES_MISSING`=404/410 · `DEAD`=timeout/DNS/refused/5xx.

**Sample**: 402 → 49.7% · alive-non-402 → 1.3% · 404/410 → 29.7% · dead → 19.3%.

**Why the sample (49.7% alive) and the extrapolation (18.9% alive) differ**: the sample is stratified (min 10 / max 60 per source), so it *over-represents* the tiny high-quality sources and *under-represents* payai. The extrapolation applies each source's measured alive-rate to that source's actual population — and payai (95.1% of the catalog, ~20% alive) dominates the weighted result. Per-stratum probe counts are in §3's per-source table. The two numbers are consistent, not contradictory.

**Extrapolated to full catalog**:

| Class | Est. items | % |
|---|---|---|
| Answers 402 today | ~4,951 | 18.9% |
| Responds, non-402 | ~24 | 0.1% |
| Host alive, resource gone (404/410) | ~16,358 | 62.4% |
| Dead host | ~1,650 | 6.3% |
| Junk/unprobeable (excluded from sample) | 3,250 | 12.4% |

**The dominant failure mode is not dead hosts — it is live aggregator/proxy hosts whose generated resource paths no longer exist** (payai probes: 73% returned 404 on responsive hosts; 404s concentrate on a few mega-hosts like `orbisapi.com` and `madnodes.xyz`).

### Per-source quality ranking

| Rank | Source | Items | % of catalog | 402-alive rate | Signature problem |
|---|---|---|---|---|---|
| 1 | questflow | 62 | 0.2% | 100% | none — small but perfect |
| 2 | self-registered | 128 | 0.5% | 95% | only source with descriptions |
| 3 | thirdweb | 622 | 2.4% | 47% | half the resources 404 |
| 4 | coinbase | 336 | 1.3% | 41% | 39% dead hosts |
| 5 | anyspend | 43 | 0.2% | 32% | 56% junk URLs, 65% non-TLS |
| 6 | payai | 24,945 | 95.1% | ~20% | ~96% of all junk, ~98% of est. 404s |
| 7 | aurracloud | 97 | 0.4% | 14% | 77% hard-dead hosts |

### Filter cascade (static = exact; dead-host stage = probe-extrapolated)

This "drop ALL junk" cascade (incl. templates) is the theoretical maximum; **the actual WS-A filter (`02` §2.2) retains the 380 spec-legal route-template URLs**, so it drops ~2,889 junk and lands at ~21,700, not 21,344. Both views below:

| Stage | Remaining (drop-all-junk) | Removed |
|---|---|---|
| Raw catalog | 26,233 | — |
| Drop junk URLs (incl. templates) | 22,983 | −3,250 |
| Drop unpayable (empty accepts) | 21,344 | −1,639 |
| Quarantine 404/410 + dead hosts (needs probing) | **~4,623** | ~−16,700 (63.7%) |

WS-A implemented cascade (templates kept): 26,233 → −2,889 junk → 23,344 → −1,639 unpayable → **~21,700**, then WS-B probing hides ~16,400 down to ~4,600–5,000 visible.

## 4. Feed-source transport health (live-verified)

| Source | Status |
|---|---|
| coinbase, payai, thirdweb, anyspend | OK (anyspend uses non-standard `{success, data:{items}}` envelope — handled by `parse_discovery_response`) |
| questflow | HTTP 500 (leaked Mongo query shows they serve only `validationStatus.isValidated: true` — they curate internally) |
| x402.rs | returns HTML landing page — JSON endpoint dead/moved |
| virtuals | 404 `NotFoundError` |
| openx402 | TLS cert mismatch |
| aurracloud, polymer, meridian | reachable; aurracloud content quality is worst-in-class |

## 5. Ecosystem curation patterns (what others do)

- **Spec freedom**: `coinbase/x402 specs/extensions/bazaar.md` does NOT define the discovery response schema, health, trust or delisting — curation fields are facilitator-defined and precedented. `routeTemplate` (`/:param`) is the spec's catalog-key contract → template URLs are legal, unprobeable directly.
- **CDP Bazaar**: `quality: {l30DaysTotalCalls, l30DaysUniquePayers, lastCalledAt}` on every item; `curated: true` + `skillUrl` + continuous probes + ~99% availability for the curated tier; listing triggered by first settlement; **delisting by 30-day inactivity, not probing**; requires ≥1 valid `accepts` network.
- **x402scan** (Merit Systems, open source): cron ping route probes every resource — **GET then POST, never HEAD; healthy = HTTP 402 with parseable x402 body**; batches of 50; on failure they delete the cached 402-response record but keep the resource row (quarantine-like). Their facilitator sync is currently **paused** (`FACILITATOR_SYNC_PAUSED = true`) — an instructive signal about uncurated firehoses. They filter out Vercel preview deployments as a junk heuristic.
- **Thirdweb**: per-item `metadata: {uniqueBuyers, totalPayments, totalVolumeUsd}`.
- **Nobody ships probe-derived `healthy`/`lastChecked` fields in discovery responses today.** Open differentiation slot.
- Security motivation: "Five Attacks on x402" (arxiv 2605.11781) — unvetted listings enable payTo-swap and schema-poisoning attacks; probe-time `payTo` drift detection is a security control, not just health.

## 6. First-party products & VIP candidates (live-verified 2026-07-23)

| Product | Bazaar status | Verified payable endpoint | Notes |
|---|---|---|---|
| **Execution Market** | LISTED (1 item: `https://mcp.execution.market/mcp/`, self-registered, $0.01 USDC, 9 chains) | `POST https://api.execution.market/api/v1/tasks` (OpenAPI declares 402; live anonymous probe returns 401 — auth precedes payment) | REST endpoint NOT listed |
| **402Milly** | LISTED (1 item: `https://mcp.402milly.xyz/mcp`, self-registered, $1.00 USDC, 10 EVM chains) | `POST https://api.402milly.xyz/purchase` — **live 402 verified** ($1.00/100px) but replies v1-style JSON, and advertises chain IDs 998/1301 (look wrong: HyperEVM=999, Unichain=130) | REST endpoint NOT listed; non-EVM rails absent from bazaar accepts |
| **MeshRelay** | **NOT LISTED (0 of 26,233)** | `POST https://api.meshrelay.xyz/payments/access/{channel}` — **live 402 verified, proper x402 v2 body**, Base + SKALE Base; 7 premium channels ($0.10–$1.00) via `GET /payments/channels` | Landing meta tag `agent:payments-endpoint` points to `/turnstile` which 404s — fix on MeshRelay side |
| **"Tangent"** | NOT FOUND anywhere (NXDOMAIN on tangent.fun; 0 hits in CDP, PayAI, our bazaar, awesome-x402, x402scan) | — | **Likely intended: Tenjin** (below). Confirm with user |
| **Tenjin** (`tenjin.blog`) | LISTED — 121 self-registered items (`/api/read/{handle}/{slug}`), largest self-registered publisher in our bazaar | live 402 verified with v2 `PAYMENT-REQUIRED` header; USDC on Base; $0.05–$0.10/article; payTo varies per creator | Pay-per-read blogs for AI agents — matches the user's "blogs for agents" description exactly |

**Housekeeping found**: `https://facilitator.ultravioletadao.xyz/__bazaar_debug__` (internal debug entry, publicly listed — delist); `https://facilitator.ultravioletadao.xyz/` self-listing has empty accepts (fine for `type=facilitator` but currently typed as a plain listing — verify type).

## 7. Raw artifacts

Scratchpad (session-local, regenerate via the audit script when needed): full snapshot `bazaar-items.json` (26,233 items), `probe-results.csv` (url, source, http_code, class), `analysis.json`, stratified sample `probe-urls.tsv`. The audit methodology is reproducible: paginate `/discovery/resources` at limit=100, then single-pass GET probe with the parameters at the top of this document.
