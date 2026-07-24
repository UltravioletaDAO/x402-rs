# WS-D — Bazaar UI (`GET /bazaar`)

**Ships as**: v1.53.0 (UI phase 1 has ZERO backend-logic dependencies and can be built anytime; badges light up as WS-B/C deploy)
**Source**: two independent UI-scout reports converged on the same recommendation — high confidence.

## 1. Integration decision

**Standalone `static/bazaar.html`, compile-time embedded, served by a new `get_bazaar()` handler at `GET /bazaar`.**

- Follows the exact existing pattern: `get_index()` uses `include_str!("../static/index.html")` (`src/handlers.rs:425` area). Zero new infrastructure, no build system, single binary, `fast-build.sh` unchanged.
- Keeps the protected landing page (`static/index.html`, CLAUDE.md protected file) untouched except a nav link.
- Rejected: section-inside-index (bloats the 3,532-line protected landing + its EN/ES tables; slows first paint) and SPA (repo deliberately has no frontend build system).
- Cost: ~15 duplicated `:root` token lines + font links + small i18n bootstrap. Acceptable; extract a shared CSS later only if it hurts.

## 2. Code touchpoints (verified against v1.50.1)

1. **NEW** `static/bazaar.html` (~900–1,100 lines, vanilla HTML/CSS/JS, EN/ES).
2. `src/handlers.rs` — handler after `get_index()` (~line 437):
   ```rust
   /// `GET /bazaar`: Bazaar resource explorer page.
   #[instrument(skip_all)]
   pub async fn get_bazaar() -> impl IntoResponse {
       let html = include_str!("../static/bazaar.html");
       Response::builder()
           .status(StatusCode::OK)
           .header("content-type", "text/html; charset=utf-8")
           .body(html.to_string())
           .unwrap()
   }
   ```
   (No emojis in Rust code — repo rule.)
3. `src/handlers.rs::routes()` (~line 116): `.route("/bazaar", get(get_bazaar))`. **`src/main.rs` DOES change** for this workstream — §4.3 requires attaching a read-side `GovernorLayer` to the discovery listing/stats/bazaar routes (they inherit none today, `main.rs:385`). The `/bazaar` HTML page itself is a static asset, but `/discovery/resources`, `/discovery/stats` and `q=` are the DoS surface that needs the limiter.
4. `src/openapi.rs`: `path_bazaar_ui` stub (tag `Bazaar` exists at line 104); add to `paths(...)` list after `path_bazaar_register` (~line 134). Build fails until `bazaar.html` exists — create the HTML first.
5. `static/index.html`: nav link in the header flex div (~line 1150, same `.lang-btn` pattern as the `/docs` link at line 1151) + `"nav.bazaar"` key in **both** `translations.en` and `translations.es` tables (ES table ends ~line 2970 — project gotcha: always update both).

## 3. Page structure

```
header (logo, Home, /docs, EN/ES switcher — copied patterns)
├── metrics band: 4 .stat-card — Total | Alive % | Networks | Sources
├── Featured band, two rows: "Our products" (first_party: Execution Market, MeshRelay, 402Milly)
│     then "Partners" (vip/verified curated: Tenjin) — do NOT lump Tenjin in with first-party (completeness #21)
├── controls: search box · filter pills (network / source / facilitator / tier / health)
├── card grid: repeat(auto-fill, minmax(320px, 1fr)), 24/page
│     card: hostname title · truncated path · network logo chip + price ·
│           source badge · tier badge (gold=first_party, purple=vip, teal=verified) ·
│           health badge (green=alive, grey=unknown, amber=degraded, blue=auth_gated, red=quarantined[admin view only]) ·
│           nonTls / template chips · relative lastUpdated
├── pager: Prev / pages / Next — "Showing 25–48 of N"
└── item detail (<dialog>): full URL+copy · all accepts[] (network name+logo,
      asset explorer link, formatted amount, payTo, timeout) · source/firstSeen ·
      settlementCount · health detail · metadata when present
```

### Data-availability map

| UI element | Backend | Status |
|---|---|---|
| Total, per-source counts | `pagination.total` (+1 `limit=1` probe per source) | EXISTS |
| network/source/sourceFacilitator filters | query params (verified live) | EXISTS |
| Card/detail fields | item fields | EXISTS (descriptions 99.5% empty — hostname-as-title mandatory) |
| Free-text search | `q=` param | **NEW — §4.1** |
| Metrics without probe-spam | `GET /discovery/stats` | **NEW — §4.2** |
| Health badge, Alive metric, health filter | `health` field/param | WS-B |
| Tier badge/filter, server-side VIP | `curation` field, `tier` param | WS-C |

**VIP hydration strategy**: first-party cards are **hardcoded** in page JS (name, EN/ES blurb, homepage, logo) so they render instantly and never vanish on aggregation hiccups; they hydrate live data (price, health, lastUpdated) from matching registry items — by `curation.tier == "first_party"` once WS-C ships, by URL-prefix match before that. The API never fabricates items; only the UI pins.

## 4. Small backend deltas (this release)

### 4.1 `q=` free-text search (~25 LOC)
`q: Option<String>` in `DiscoveryQueryParams` (`src/handlers.rs:222`) and `DiscoveryFilters` (`src/types_v2.rs:1257`); case-insensitive substring over `url`, `description`, `metadata.provider`, `metadata.tags` in `matches_filters` (`src/discovery.rs:451`), against **precomputed lowercased fields** (computed once at import, not per request). **Cap `q` ≤128 chars → 400 otherwise; reject control chars** (08 §6/F6). Full scan of ~21k in-memory items is <5ms *for bounded q at a bounded rate* — which is why the rate limit below is mandatory. Client: 300ms debounce, `AbortController`, reset offset on change.

### 4.2 `GET /discovery/stats` (~60 LOC)
On `discovery_routes()` (`src/handlers.rs:204`): one read-pass over the registry → `{ total, visible, bySource, bySourceFacilitator, byNetwork, byTier, byHealth, lastAggregation }`. **Served ONLY from a 60s in-process cache — never recomputed on demand** (08 §6/F6). Replaces all client-side metric probing; also powers filter option lists (sourceFacilitator dropdown has no enumeration endpoint today).

### 4.3 Rate-limit the read routes (F6 — MANDATORY)
`main.rs:385` merges `discovery_routes()` with **no `GovernorLayer`** today. `GET /discovery/resources`, `q=`, `/discovery/stats`, `/bazaar` are unauthenticated and ungoverned → a loop of `?q=<128 chars>` is a cheap full-catalog-scan + read-lock-contention DoS that also slows the aggregator/prober writers. Attach a read-side `GovernorLayer` (~30 req/min/IP, `SmartIpKeyExtractor` as `main.rs:338`) to these routes. See 08 §6.

## 4.5 UI trust & injection defense (F14 — MANDATORY)

- **Never confer trust styling from free-text.** A verified/first-party badge, logo, or homepage link is rendered ONLY from server-set `curation.tier` — never from the attacker-controllable `metadata.provider` or `description`. `POST /discovery/register` accepts `provider: "Execution Market"` on any URL; without this rule a phishing listing borrows our brand.
- **HTML-escape every item field** (`url`, `description`, `provider`, tags, `payTo`, asset) before insertion — use `textContent`/`createElement`, never `innerHTML` with interpolated item data. XSS defense-in-depth for a page rendering 21k attacker-supplied strings.
- Health/tier badges reflect only server-authoritative fields.

## 5. Client-side rules (26k→~21k items)

- **Never bulk-fetch the corpus** — server-side pagination/filtering only (cap 100/page; UI uses 24).
- Tiny LRU cache (~20 query-keyed entries) for instant Prev/Back; skeleton loaders; explicit empty/error states — never an eternal "Loading…" (project landing-page lesson).
- Deep-linkable state in `location.search` (`/bazaar?network=eip155:8453&facilitator=payai&q=trading&page=3`); `replaceState` for filter changes, `pushState` for page changes; language in `localStorage`, not URL.
- Amount formatting: `amount` is atomic units without decimals info — ship a small known-asset map (USDC/USDT/EURC/AUSD/PYUSD/USDG = 6 decimals, sourced from `config/supported_tokens.json`) with raw-units fallback.
- Network chips reuse already-served logo routes (`/base.png`, `/avalanche.png`, `/solana.png`, … — `src/handlers.rs:148-177`).

## 6. Visual consistency — copy from `index.html`

- `:root` tokens (lines 30-44): `--bg-dark #0a0a0f`, `--bg-card #13131a`, `--accent #00d4ff`, `--success #10b981`, `--border #1f1f2e`, etc. Fonts: Inter + JetBrains Mono (same Google Fonts links).
- Selectors to copy: `.bg-grid`+`gridMove`, `.container`, header set, `.lang-btn`, `.gradient-text`, `.stats`/`.stat-card`/`.stat-value`/`.stat-label`, `.network-badge` shine hover (basis for `.resource-card`), `.endpoint`/`.method.get`/`.method.post` (perfect for accepts rows), `.tab-button` filter pills, `.section-title`, `.fade-in`+IntersectionObserver.
- VIP band: gold-gradient card recipe (index.html:527) for first_party; ERC-8004 purple accent (`rgba(139,92,246,…)`, lines 2025/2055) for vip.
- i18n: `data-i18n` attributes + page-local `translations = {en:{…}, es:{…}}` (~30 keys) + `updateTranslations()` loop + browser autodetect — the exact index.html mechanism (lines 2975-3059).
- Same gtag snippet (`G-4FSNQNPMZX`) if analytics wanted.

## 7. Tests / verification

- `cargo build` embeds the page (fails if file missing — that IS the test).
- Manual: EN/ES toggle, all filters against prod data, deep links, mobile (grid collapses to 1 col).
- Post-deploy:
```bash
curl -s https://facilitator.ultravioletadao.xyz/bazaar | grep -ci bazaar          # >0
curl -s '.../discovery/resources?q=tenjin&limit=5' | jq '.items | length'         # >0
curl -s '.../discovery/stats' | jq '.total, .byHealth'
curl -s '.../api-docs/openapi.json' | jq '.paths."/bazaar" != null'               # true
```
