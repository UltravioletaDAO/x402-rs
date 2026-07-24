# Curated Bazaar — Master Plan

**Status**: READY TO EXECUTE · **Created**: 2026-07-23 · **Owner**: Ultravioleta DAO
**Discovery inputs**: 5-agent parallel sweep (code map, live data audit + 300-URL health probe, ecosystem research, 2x UI scouts, first-party products inventory). Evidence documents: `01-current-state-audit.md`.

## 1. Vision

Turn the Ultravioleta Meta-Bazaar (`/discovery/resources`, ~26k aggregated items) into the **first curated x402 bazaar in the ecosystem**:

- **Every listed item is payable**: declares at least one valid network in `accepts` (today 15.3% declare none).
- **Every default-visible item is alive**: verified by a recurring 402 health probe (today only ~19% of the catalog answers HTTP 402).
- **Our products are first-class citizens**: Execution Market, MeshRelay and 402Milly always rank first, followed by curated VIP partners (e.g. Tenjin — pay-per-read blogs for agents).
- **Nobody else does this yet**: CDP ships `quality`/`curated`, Thirdweb ships volume metadata, but **no facilitator exposes probe-derived `health`/`lastChecked` fields** — and none has on-chain-attested curation. Both slots are open; we take them (health now, ERC-8004 attestation as the phase-4 differentiator).

## 2. Current state (one paragraph; full evidence in 01)

The import path is wide open: both the aggregator and the crawler call `bulk_import(resources, skip_validation=true)` (`src/discovery_aggregator.rs:884`, `src/discovery_crawler.rs:291`), bypassing the SSRF guard and the non-empty-`accepts` rule that exist in `validate_resource` (`src/discovery.rs:597-637`). Result: 26,233 items of which ~4,951 (18.9%) actually answer 402 today; 3,250 junk URLs (incl. 2,318 fake `monopoly://` scheme items, all from payai); 4,026 unpayable (empty accepts); ~16,358 host-alive-but-404; ~1,650 dead hosts; 86.3% not updated in >30 days. payai alone is 95.1% of the catalog and contributes ~96% of the junk. There is no liveness, no ranking beyond `last_updated DESC`, no tiers, and no delete path from feeds. MeshRelay is not listed at all; a leftover `__bazaar_debug__` entry is publicly listed.

## 3. Target architecture

```
                      ┌────────────────────────────────────────────────┐
 12 external feeds ──▶│ WS-A INGESTION FILTER (ImportPolicy)           │
 crawler ────────────▶│  junk-URL / unpayable / SSRF / per-source caps │
 self-register ──────▶│  + field-preserving merge + retention GC      │
                      └───────────────┬────────────────────────────────┘
                                      ▼
                        DiscoveryRegistry (S3: bazaar/resources.json)
                                      │
              ┌───────────────────────┼──────────────────────────┐
              ▼                       ▼                          ▼
   WS-B HEALTH PROBER      WS-C CURATION OVERLAY        WS-D /bazaar UI
   402-as-health, tiered   tiers: first_party > vip >   static bazaar.html
   cadence, hysteresis,    verified > listed >          + /discovery/stats
   quarantine (overlay:    quarantined; manifest:       + q= search
   bazaar/health.json)     config/bazaar_curation.json  + health/tier badges
              │                       │
              └───────────┬───────────┘
                          ▼
        GET /discovery/resources  — default: quarantined hidden,
        ordered tier → health → lastUpdated; health + tier fields exposed
                          │
                          ▼  (phase 4)
        WS-E ERC-8004 ATTESTED CURATION — prober-signed uptime/successRate
        feedback on-chain; VIP claims independently verifiable
```

**Item lifecycle**: `listed` → (probe pass) `verified` → (M consecutive probe failures) `quarantined` (hidden by default, retried with backoff) → (30 days continuously dead, or security event) `removed` (+ 90-day tombstone). `first_party`/`vip` are curation labels from the manifest, orthogonal to health; a quarantined VIP is still hidden (health always wins visibility).

## 4. Workstreams

| WS | Document | What | Ships as |
|---|---|---|---|
| Phase 0 | `06-rollout-and-ops.md` §2 | Housekeeping: **MeshRelay 7 channels registered (DONE 2026-07-23)**; queue `__bazaar_debug__` delist, register missing first-party REST endpoints, fix broken feed URLs, commit audit tooling | ops only, no deploy |
| WS-A | `02-ingestion-filter.md` | Ingestion filter: ImportPolicy, per-source config, churn+timestamp fix, field-preserving merge, retention GC, pagination fix | v1.51.0 |
| WS-B | `03-health-checker.md` | Health prober: 402-as-health, tiered cadence, hysteresis, quarantine, health overlay store, initial full sweep | v1.52.0 |
| WS-C | `04-curation-tiers.md` | Tier model + `config/bazaar_curation.json` manifest + tier-aware ordering + admin delist API | v1.52.0 (with WS-B) |
| WS-D | `05-bazaar-ui.md` | `/bazaar` UI + `q=` search + `GET /discovery/stats` + read-route governor | v1.53.0 |
| WS-E | `07-erc8004-attested-curation.md` | On-chain attested VIP tier (differentiator) | v1.54.0 |
| WS-SEC | `08-security-hardening.md` | Cross-cutting security (F1…F16); **F1 blocks WS-C, F2/F3 block WS-B** | folded into each WS |

Dependency graph: Phase 0 is independent. WS-A has no dependencies. WS-B depends on WS-A's **snapshot-persistence hygiene** (`02` §2.7) — NOT because imports clobber health (health lives in a separate overlay precisely so they can't), but because the prober and aggregator must not thrash the single S3 file with racy writes. WS-C's ordering needs WS-B's health field for full effect but the manifest + tier ordering can ship standalone. WS-D phase 1 (UI browsing existing filters) has **zero** backend dependencies and can be built any time; its health/tier badges light up when WS-B/C are deployed. WS-E needs WS-B (prober emits the attestations). **WS-SEC is not optional or last** — its two blockers gate WS-B and WS-C respectively.

## 5. Expected outcome (numbers)

| Stage | Default-visible items | Removed/hidden |
|---|---|---|
| Today | 26,233 | — |
| After WS-A (static filters + GC; spec-legal templates retained) | ~21,700 | ~4,500 junk + unpayable **deleted** |
| After WS-B initial sweep | **~4,600–5,000 verified-alive** | ~16,400 hidden: mostly probation-`unknown` (never earned a probe) + resource-gone/dead quarantined |
| Quarantine/probation recovery | grows back automatically as probes see 402s | |

Note the day-1 shrink to ~5k is driven by **probation-hiding** (payai/anyspend/aurracloud items hidden as `unknown` until a first successful probe) plus sweep promotions — NOT by quarantining, which takes weeks under M=3 weekly hysteresis (see `03` §2 bootstrap rule to accelerate). A ~5k-item all-alive, all-payable bazaar with our products pinned first beats a 26k-item catalog that is 81% noise — and history is retained (hidden, not deleted, except the ~4,500 static-junk purge) so recoveries resurface automatically.

## 6. Success criteria (session + rollout)

**Plan (this session)**: these 9 markdown files exist under `docs/plans/bazaar/`, cover ingestion filter + sweep/health + VIP tiers + UI + rollout + security with real `file:line` anchors, and survived a 3-reviewer adversarial critique round (feasibility/completeness/security) whose findings are folded in. The security acceptance checklist (`08` end) gates the WS-B/WS-C deploys.

**Rollout** (verify after each phase, see `06-rollout-and-ops.md` §6 for full commands):

```bash
# After WS-A: no junk schemes, no empty-accepts items (excluding type=facilitator self-listing, which is empty by design)
curl -s 'https://facilitator.ultravioletadao.xyz/discovery/resources?limit=100' \
  | jq '[.items[] | select(.type != "facilitator" and (.accepts | length) == 0)] | length'   # expect 0

# After WS-B: health fields present, default list is alive-only
curl -s '.../discovery/resources?limit=1' | jq '.items[0].health'  # non-null

# After WS-C: first item is first-party
curl -s '.../discovery/resources?limit=3' | jq '.items[].curation.tier'  # "first_party" first

# After WS-D:
curl -s https://facilitator.ultravioletadao.xyz/bazaar | grep -i bazaar
```

## 7. Key risks (details + mitigations in 06 §7)

1. **S3 single-file read-modify-write races** (`src/discovery_store.rs:267-281`) — prober + aggregator writing concurrently lose updates. Mitigation: health lives in a **separate overlay object** with debounced snapshot writes; aggregator moves to one `save_all` snapshot per cycle.
2. **Curation state clobbered by re-imports** — `bulk_import` replaces whole structs ("newer wins", `src/discovery.rs:403-417`). Mitigation: field-preserving merge (WS-A) + overlay stores keyed by normalized URL.
3. **Upstream feed volatility** — 3 of 12 sources are already broken at transport level; QuestFlow 500s. Aggregator must treat per-source failure as routine (it does). The retention GC only enforces the deterministic static rules R1–R5 on already-stored data (not on fetch success), and `purgeOnDisable` defaults false — so a transient source outage never mass-deletes; the pre-phase-1 S3 backup is the ultimate undo.
4. **Probe volume** — ~23k probeable URLs; weekly tail sweep ≈ 2 req/min average. Trivial for us; per-origin batching keeps us polite to mega-hosts (orbisapi.com hosts thousands of listings).
5. **"Tangent" identity — RESOLVED** — no x402 product named "Tangent" exists (NXDOMAIN, 0 hits in CDP/PayAI/our bazaar/awesome-x402). User confirmed 2026-07-23 the intended product is **Tenjin (tenjin.blog)**, already 121 self-registered items in our bazaar, live-402-verified. Ships as `verified` tier; promotes to `vip` once Tenjin serves a `.well-known/x402` origin proof (08 §10).

## 8. File map (all workstreams)

| File | WS | Change |
|---|---|---|
| `src/discovery_aggregator.rs` | A | ImportPolicy filters; config-driven source list via `with_facilitators()`; churn+timestamp-clamp fix; pagination fix; hardened HTTP connector (08 §15) |
| `src/discovery_crawler.rs` | A | `ImportPolicy::Filtered`; hardened HTTP connector (08 §15) |
| `src/discovery.rs` | A,B,C | field-preserving merge (+ F4 no-downgrade/manifest-authoritative) in `bulk_import`; retention GC; `VisibilityPolicy` + tier/health-aware sort in `list`; extend `is_disallowed_target_ip` (08 §2) |
| `src/discovery_store.rs` | B | overlay store objects (`bazaar/health.json`, `bazaar/curation-state.json`, sha256-keyed evidence); snapshot `save_all` path |
| `src/discovery_health.rs` (new) | B | prober task, SSRF connector, state machine, hysteresis, backoff, daily-history |
| `src/discovery_curation.rs` (new) | A,C | `canonical_url` (08 §0), `CurationFilter` rules, manifest loading, safe tier matcher, admin endpoints |
| `src/discovery_attestation.rs` (new) | E | ERC-8004 attestation writer + summary reader (08 §11) |
| `config/bazaar_sources.json` (new) | A | per-source trust/enable/caps |
| `config/bazaar_curation.json` (new) | C | first-party + VIP manifest |
| `src/handlers.rs` | C,D | new params (`health`, `tier`, `q`), `/discovery/stats`, `/bazaar` page, admin delist, all-settlement→health hook |
| `src/types_v2.rs` | A,B,C | `from_aggregation(Option<u64>)`, `route_template` field; `health` + `curation` response fields (`#[serde(skip_serializing_if)]`), filter fields |
| `src/openapi.rs` | C,D | document everything new + **fix already-stale Bazaar docs** (`src/openapi.rs:1004-1085`) |
| `static/bazaar.html` (new) | D | the UI (escapes all fields, no trust from free-text — 08 §14) |
| `static/index.html` | D | nav link + i18n keys (EN+ES) |
| `scripts/bazaar_audit.py` (new) | 0 | reproducible snapshot+probe audit + re-audit tool |
| `tests/fixtures/bazaar/payai-page.json` (new) | 0 | offline WS-A test fixture |
| `src/main.rs` | A,B,C,D | task wiring, env vars, read-route governor (F6), admin router wiring (08 §8) |
| `terraform/environments/production/main.tf` | B,C,E | new env vars + `BAZAAR_ADMIN_TOKEN`/`BAZAAR_ATTESTATION_KEY` secret refs (same S3 bucket, IAM already covers `bucket/*`) + CloudWatch metric-filter alarm for paytoswap (F4) |

## 9. Documents in this plan set

- `00-MASTER-PLAN.md` — this file
- `01-current-state-audit.md` — evidence: code map, live data audit, probe results, ecosystem research, product inventory
- `02-ingestion-filter.md` — WS-A design
- `03-health-checker.md` — WS-B design
- `04-curation-tiers.md` — WS-C design (incl. Phase 0 housekeeping payloads)
- `05-bazaar-ui.md` — WS-D design
- `06-rollout-and-ops.md` — sequencing, env vars, metrics, verification, risks
- `07-erc8004-attested-curation.md` — WS-E design (differentiator)
- `08-security-hardening.md` — cross-cutting security (F1…F16); canonical URL primitive; **F1 blocks WS-C, F2/F3 block WS-B**; security acceptance checklist
