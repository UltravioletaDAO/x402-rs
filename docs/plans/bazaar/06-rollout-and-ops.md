# Rollout & Operations — Curated Bazaar

## 1. Sequencing

| Phase | Version | Content | Gate to next |
|---|---|---|---|
| 0 | none (ops only) | Housekeeping registrations + external-side fixes + commit audit tooling (04 §6, §2) | none — do anytime |
| 1 | v1.51.0 | WS-A ingestion filter + source config + merge/churn/timestamp fixes + retention GC + hardened aggregator/crawler connector (08 §15) | catalog ~21.7k, zero junk-scheme/empty-accepts in listing; aggregation cycle stable for 48h |
| 2 | v1.52.0 | WS-B health prober + WS-C tiers/manifest/admin. **Security gate: 08 checklist green (F1/F2/F3 mandatory)** | initial sweep complete (~3–4h); default view ~5k (probation-hiding ~20k `unknown` day-1 + sweep promotions, NOT quarantining); first_party pinned |
| 3 | v1.53.0 | WS-D `/bazaar` UI + `q=` + `/discovery/stats` + read-route governor (F6) | UI live, deep links work |
| 4 | v1.54.0 | WS-E ERC-8004 attested curation | attestations on-chain for first_party (kill-switch default off) |

Standard delivery per project convention: `/ship` pipeline (CI on push to main → ECR → ECS; fallback `fast-build.sh` + targeted terraform apply). **Never compile/deploy automatically — the user builds and deploys manually.** Before each release: check deployed version first (`curl -s https://facilitator.ultravioletadao.xyz/version`), bump from THAT. Cargo.lock gotcha: hand-sync the x402-rs version line (CI runs `--locked`).

## 2. Phase 0 detail

See `04-curation-tiers.md` §6 for ready-to-run payloads. Summary:
1. **DONE 2026-07-23** — MeshRelay's 7 premium channels registered via `POST /discovery/register` (each probed live for exact accepts, all 201). Verified: `curl -s '.../discovery/resources?source=self_registered&limit=100' | jq '[.items[].url|select(contains("meshrelay"))]'` → 7 items (alpha-test $0.10, kk-alpha $1.00, kk-consultas $0.25, kk-skills $0.50, abra-alpha $1.00, security-realtime $0.50, security-vip $1.00), each on Base + SKALE Base, payTo `0xe4dc963c56979E0260fc146b87eE24F18220e545`.
2. Register first-party REST endpoints: `api.402milly.xyz/purchase` (terms verified), `api.execution.market/api/v1/tasks` (confirm terms with EM first).
3. `__bazaar_debug__` delisting waits for WS-C admin DELETE (no exposed unregister today); it is in the manifest `suppressed[]` list as a backstop.
4. File external-side fixes: MeshRelay `/turnstile` meta tag 404; 402milly wrong chain IDs (998/1301) + v1-style 402 response + missing non-EVM rails in its bazaar entry.
5. **Commit the audit tooling to the repo before WS-A** (completeness #5 — the discovery-sweep data + scripts currently live in an ephemeral session scratchpad and will vanish):
   - `scripts/bazaar_audit.py` — paginate `/discovery/resources` at limit=100 → full snapshot; run the static quality analysis + the single-pass GET/POST health probe (methodology in `01` header). Doubles as the post-deploy re-audit tool for every verification gate in §6.
   - `tests/fixtures/bazaar/payai-page.json` — a captured real payai response page (with its junk: `monopoly://`, empty-accepts, 404-prone URLs) so WS-A's integration test (`02` §4) is runnable offline by a fresh engineer.

## 3. Environment variables (full inventory added by this plan)

| Var | Phase | Default | Prod value |
|---|---|---|---|
| `BAZAAR_SOURCES_PATH` | 1 | embedded config | unset |
| `DISCOVERY_ENABLE_HEALTH` | 2 | `true` | `true` |
| `DISCOVERY_HEALTH_TICK` | 2 | `60` | unset |
| `DISCOVERY_HEALTH_CONCURRENCY` | 2 | `15` | unset |
| `DISCOVERY_HEALTH_TAIL_DAYS` | 2 | `7` | unset |
| `DISCOVERY_HEALTH_PERSIST_SECS` | 2 | `300` | unset |
| `DISCOVERY_HEALTH_MAX_RPS` | 2 | `20` | unset |
| `DISCOVERY_HEALTH_BOOTSTRAP` | 2 | `true` (first deploy) | `true` for the first v1.52.0 deploy, then unset |
| `BAZAAR_SOURCES_PATH` | 1 | `config/bazaar_sources.json` | unset (uses default) |
| `BAZAAR_CURATION_PATH` | 2 | `config/bazaar_curation.json` | unset (uses default) |
| `BAZAAR_ADMIN_TOKEN` | 2 | unset (admin routes 404) | **AWS Secrets Manager** (never in task-def `environment` — project security rule) |
| `ENABLE_BAZAAR_ATTESTATIONS` | 4 | `false` | `false` until WS-E validated |
| `BAZAAR_ATTESTATION_KEY` | 4 | unset | **AWS Secrets Manager** (dedicated attestation wallet, WS-E §3) |

**Config-file packaging**: `config/bazaar_sources.json` and `config/bazaar_curation.json` are read at runtime (like `config/blacklist.json`) — the Dockerfile must COPY `config/` into the image (verify it already does for blacklist; if so, no change).

Existing knobs unchanged: `DISCOVERY_S3_BUCKET/KEY`, `DISCOVERY_ENABLE_AGGREGATION`, `DISCOVERY_AGGREGATION_INTERVAL`, `DISCOVERY_ENABLE_CRAWLER/CRAWL_URLS/CRAWL_INTERVAL`, `FACILITATOR_URL`.

Infra: **no new AWS resources.** New S3 objects (`bazaar/health.json`, `bazaar/curation-state.json`) live in the existing `facilitator-discovery-prod` bucket; the task-role policy already grants `bucket/*` (terraform `main.tf:633-655`). Terraform change = env vars + one secret reference only.

## 4. Backup & rollback

- **Before Phase 1 first deploy**: snapshot the current registry — `aws s3 cp s3://facilitator-discovery-prod/bazaar/resources.json ./backups/resources-pre-curation-$(date +%Y%m%d).json`. The retention GC deletes ~4.9k items permanently; this is the undo.
- Rollback strategy per phase: all read-side changes are additive (`health`/`curation` fields optional; new params ignored by old clients); redeploying the previous image restores prior behavior. Overlay objects are ignored by older builds (unknown S3 keys) — no migration needed either direction.
- Kill-switches: `DISCOVERY_ENABLE_HEALTH=false` (prober off, health filters degrade to `any`), `DISCOVERY_ENABLE_AGGREGATION=false` (freeze catalog), per-source `enabled:false`.

## 5. Observability

Prometheus (existing metrics stack; observability toggle in terraform):
- `bazaar_import_dropped_total{source, rule}` — WS-A per-rule drop counters (incl. `rule="future_timestamp"`, F5)
- `bazaar_registry_items{source, tier, health}` — gauge per cycle
- `bazaar_probes_total{outcome}` / `bazaar_probe_duration_seconds` — WS-B
- `bazaar_quarantine_transitions_total{direction}` — flap visibility
- `bazaar_paytoswap_total` — security events
- `bazaar_s3_persist_total{object, outcome}` — overlay + snapshot writes

**Security alerting must NOT depend on Prometheus** — the observability stack is behind `enable_observability`, default OFF ($0/mo mode, per the observability runbook). The `payTo`-swap / first-party-drift signal (F4) needs an **always-on** path: a **CloudWatch Logs metric filter** on the `paytoswap` WARN log line + a CloudWatch alarm (SNS to the ops channel). Wire this at WS-B deploy regardless of the Prometheus toggle. Log greps for manual checks: `curation_filter`, `health_sweep`, `paytoswap`. CloudWatch: no emojis in any log strings (repo rule); `Authorization` headers redacted (08 §8).

## 6. End-to-end verification matrix

```bash
V=https://facilitator.ultravioletadao.xyz
# Phase 1 (gate on invariants + direction, NOT a brittle exact count)
curl -s "$V/discovery/resources?limit=1" | jq '.pagination.total'                      # in 21000..22000 band (~21700)
curl -s "$V/discovery/resources?limit=100" | jq '[.items[] | select(.type != "facilitator" and (.accepts|length)==0)] | length'  # 0
curl -s "$V/discovery/resources?limit=100" | jq '[.items[].url | select(startswith("http")|not)] | length'                       # 0 (no monopoly:// etc)
# Phase 2
curl -s "$V/discovery/resources?limit=1" | jq '.items[0].health.status'               # "alive"
curl -s "$V/discovery/resources?limit=1" | jq .pagination.total                       # ~4600-5000 (after sweep)
curl -s "$V/discovery/resources?limit=1&health=any" | jq .pagination.total            # ~21700
curl -s "$V/discovery/resources?limit=5" | jq '[.items[].curation.tier]'              # first_party leads
curl -s "$V/discovery/resources?limit=100" | jq '[.items[].url | select(contains("__bazaar_debug__"))] | length'  # 0
# Phase 3
curl -s "$V/bazaar" | grep -ci bazaar                                                 # >0
curl -s "$V/discovery/stats" | jq '.total, .byHealth, .byTier'
curl -s "$V/discovery/resources?q=tenjin&limit=3" | jq '.items | length'              # >0
curl -s "$V/api-docs/openapi.json" | jq '.info.version'                               # matches release
# Always
curl -s "$V/version"; curl -s "$V/health"
```

## 7. Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| S3 lost-update between aggregator snapshot and overlay writes | med | listings/health silently stale | separate objects per writer (resources=aggregator, health=prober, curation-state=admin); single snapshot per cycle; dirty-flag debounce |
| Retention GC mass-deletes on a transient upstream format change | low | catalog shrinks wrongly | GC only enforces static rules R1–R5 (deterministic on stored data, not on fetch success); `purgeOnDisable` default false; pre-phase-1 S3 backup |
| Probe task hammers a mega-host / gets us blocklisted / weaponized as DDoS | med | probe data degrades; abuse complaint | per-destination-IP/24 cap 2, global concurrency 15 + `MAX_RPS` 20, weekly tail cadence, per-origin VIP sampling, honest UA, honor Retry-After, port allowlist (08 §7/§16). Probeable population ~21k after WS-A (not 23k) |
| A first-party product has a real outage → hidden from own bazaar | med | embarrassing | UI hardcodes first-party cards (render regardless); API-side health still honest — this is a feature, verify wording ("temporarily unavailable" badge) |
| payai feed balloons or turns adversarial | med | noise floods back | probation trust: hidden-until-402-verified; `maxItems` cap; per-source kill-switch |
| Registry memory growth (26k → ?) | low | ECS memory pressure | ~26k items ≈ tens of MB worst case in a 2GB task; `bazaar_registry_items` gauge alerts on trend |
| Admin token leak | low | catalog vandalism | Secrets Manager only, constant-time compare, governored router, suppress/release reversible, tombstones auditable (08 §8) |
| "Tangent"≠Tenjin misidentification | — | wrong VIP shipped | Tenjin ships as `verified` (not vip) until user confirms + origin proof (04 §3, 08 §10) |
| **First-party impersonation via register + naive prefix match** | **high if unmitigated** | **attacker showcased as our product, paid** | **F1 (08 §1): host-exact + path-boundary match on parsed URL — BLOCKS WS-C** |
| **SSRF to AWS task-role creds via prober fetching attacker URLs** | **high if unmitigated** | **credential theft** | **F2/F3 (08 §2/§3): DNS-resolve-check-pin connector + manual redirects — BLOCKS WS-B** |
| **Feed-poisoning payTo hijack / far-future timestamp poison** | med | VIP payment redirected / immortal poison record | F4/F5 (08 §4/§5): manifest-authoritative payTo, no source-downgrade, timestamp clamp |

## 8. Definition of done (whole initiative)

1. `/discovery/resources` default view contains only payable (≥1 valid network) resources with **no quarantined and no probation-unverified items**; non-alive states (`degraded`/`auth_gated`/`unknown` from standard sources) remain visible but flagged. Ordered first_party → vip → verified → listed. (Precisely: the default `VisibilityPolicy` hides quarantined + probation-`unknown` + suppressed; it does not require every item to be `alive` — that would hide the legitimately auth-gated Execution Market REST endpoint.)
2. Junk schemes, empty-accepts, localhost/malformed: 0 in default view (spec-legal templates retained but tagged); quarantined items retrievable via `health=any`.
3. Execution Market, MeshRelay, 402Milly listed (REST + MCP), pinned first, payTo-allowlisted. Tenjin (user-confirmed) as first VIP.
4. `/bazaar` UI live in EN/ES showing metrics, tiers, health badges, search, filters.
5. Recurring probes keep the view fresh without operator action; quarantine/recovery automatic; security drift alarms wired.
6. OpenAPI docs match reality (including the previously-stale Bazaar section).
7. README + `docs/CHANGELOG.md` updated per release (project convention).
