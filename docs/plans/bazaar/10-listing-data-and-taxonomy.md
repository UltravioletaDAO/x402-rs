# 10 — Listing data and taxonomy

**Status**: implemented, not deployed · **Date**: 2026-10-02 · **Origin**: a router partner's
Bazaar feedback of 2026-10-01 ("keep seller text and schemas when aggregating", "a kind field and
a fixed category list", and two data corrections), approved by the owner on 2026-10-02.

A router choosing a paid service on an agent's behalf needs, besides the price, three things from
a listing: what it is (description), what to send (declared input) and what comes back (declared
output). It also needs to tell a tool from a paid essay, and to filter by one vocabulary.

## 1. Measured before the change

Catalog snapshot `bazaar/resources.json`, version of 2026-10-01T20:03:18Z (the one in force when
the feedback was taken at 20:45Z), 2 000 records. Read-only copy, never committed (third-party
data). Counts through the real code paths:

```text
BAZAAR_SNAPSHOT=<copy> cargo test --test bazaar_listing_data snapshot_report -- --ignored --nocapture
```

| Measure | Value | Feedback said |
|---|---|---|
| Records by source | thirdweb 873, self_registered 788, coinbase 201, payai 138 | 1 999 listings |
| Empty description | 873 — every one from thirdweb | 861 from thirdweb |
| No input schema (`bazaar.info.input` or `bazaar.schema.properties.input`) | 1 646 (thirdweb 873, self_registered 773) | 1 609 of 1 999 |
| Listings under `tenjin.blog/api/read/` resolving to `vip` | 379, all self-registered (the host's two other listings, `/api/answer` and `/api/phone-lookup`, are tools) | 379 essays |
| A category declared (any of the three places) | 328 listings (16.4 %), in 26 spellings | 17 %, spelled several ways |

Where the data was lost, by ingestion route (sha `ff0c6404`):

| Route | Entry | What it dropped |
|---|---|---|
| Aggregator | `src/discovery_aggregator.rs:982` `convert_resources` → `:920` `convert_single_resource` | `accepts[].description` and `accepts[].outputSchema`: `DeclaredPaymentOption` (`src/discovery_price.rs:641`) has no field for either, so serde ignores them. Only the resource-level `description` (`:948`) and `extensions` (`:954`) were read. A feed in the x402 v1 `DiscoveredResource` shape has neither at the resource level. Decided on purpose for CDP on 2026-09-10 (`docs/handoffs/2026-09-10-bazar-precios-p0.md:291-293`), where the resource level does carry both. |
| Merge | `src/discovery.rs:1428-1462` (`bulk_import`, `ImportVerdict::Replace`) | "incoming wins for content": a newer copy with an empty description or no `extensions` replaced a copy that had them. `Keep` dropped the incoming copy whole, text included. |
| Crawler | `src/discovery_crawler.rs:262` `to_discovery_resource` | nothing (description, metadata and extensions are copied) |
| `POST /discovery/register` | `src/types_v2.rs:1744` `into_resource` | nothing |

The 873 thirdweb records were stored with an empty description, no `extensions` and
`metadata: {}`, the v1 shape. The 400 `market.datapackvibe.com` listings are all among them.

## 2. What changed

### 2.1 x402 v1 fields are lifted to the resource

`DeclaredPaymentOption` gained `description` and `outputSchema`, both tolerant (a non-string
description, an out-of-bounds schema are dropped, never fatal to the page). The aggregator lifts
them per the bazaar spec's v1 mapping (`accepts[].description` → `description`,
`accepts[].outputSchema` → `extensions.bazaar`), in `lift_v1_resource_fields`:

- **Fill-only.** What the resource declares itself wins (the CDP case).
- **Verbatim, input and output apart.** `outputSchema.input` / `.output` become
  `extensions.bazaar.info.input` / `.output` unchanged — the spec's own shape, which is where every
  reader already looks. A bare object (no `input`/`output` keys) is the response schema, so it goes
  to `info.output`. A v1 `bodyFields` stays `bodyFields`: building an example body from a field
  list would be inventing a request.
- **Bounded.** A description over `MAX_DESCRIPTION_LEN` (2 048) is not lifted (lifting it would get
  the listing refused at ingestion); a lifted schema that would push `extensions` over
  `MAX_EXTENSIONS_BYTES` leaves `extensions` exactly as it was.

The health prober reads `extensions.bazaar.info.input.method`, so a v1 listing that declares POST
becomes probeable with its declared method once both changes ship.

### 2.2 Nothing replaces something with nothing

`DiscoveryResource::fill_descriptive_gaps_from` fills a record's missing description, `bazaar`
extension and tags from another copy of the same listing. Never `metadata.category` or
`metadata.provider`: those are this record's seller's own declaration (see 2.3). In `bulk_import`:

- before the verdict, the incoming copy fills its gaps from the held one, so an empty
  re-download of an enriched record compares as **unchanged** and is not rewritten every cycle;
- on `Keep` (the held terms stand), the held record fills its gaps from the incoming copy and is
  written (`updated`, logged as `enriched`).

Only a source at least as authoritative as the record fills it (`may_fill`, the same provenance
ladder as the verdict): two aggregated copies complete each other, but a third party's copy never
completes an owner's own registration or the origin's own document. What the owner left unsaid is
not a feed's to say on its behalf, and a feed must not be able to put a request schema on a
listing it does not own.

Price, dates and provenance are untouched: which copy's terms stand is still `import_verdict`'s
call, by authority and date. Text is never replaced, only never lost — so a publisher can revise
its description, but an empty copy cannot erase one. Nothing is written that no source published.

### 2.3 `kind`, `categories` and `categorySource`

Response-only fields on every listing, resolved at read time from `config/bazaar_taxonomy.json`
(compiled in, `src/discovery_taxonomy.rs`) and never stored. They sit **beside**
`metadata.category`, which is served exactly as the seller declared it: other systems admit or
refuse a listing on that declaration, and must never read one nobody made.

- **`kind`**: `api` (a paid call to a tool) or `content` (a paid piece of content, the same for
  every buyer).
- **`categories`**: ids of the closed list below, in the order found, without repeats; absent when
  nothing maps.
- **`categorySource`**: `declared` (every id is the seller's own value, spelled as the id),
  `normalized` (the seller's value in another spelling: `Data`, `data_processing`, `twitter`) or
  `inferred` (assigned by an operator override, for a listing whose own data names no category).
- **`hasInputSchema`**: whether `extensions.bazaar` declares the input. Derived; the declaration
  itself stays in `extensions.bazaar`.

**Since 2.49.0** a listing that declares no category at all is placed by deterministic rules over
its host, path, description and schema, as `inferred` -- see
`docs/plans/bazaar/11-categories-upstream-usage-ranking.md`. What follows is the declared half.

Resolution order: an operator **override** (host-exact + path-boundary, the curation manifest's
matcher) decides first; otherwise every value the seller declared — `metadata.category`, then
`extensions.bazaar.category`, then a `bazaar.category` inside an option's `extra` — normalized
(lowercase, `_` and spaces to `-`) and mapped through `aliases`, each contributing its id once. A
spelling that maps to nothing adds nothing: no category is ever guessed. Tags are not read (the
seller that tags itself `market-data` sells data about the x402 market, not about markets).
`kind` is `content` when an override says so or any declared value is a content spelling
(`article`, `essay`, `blog`, ...).

| Category | Covers |
|---|---|
| `people` | Information about a person: profiles, work email, phone and identity lookups. Returns personal data: whoever buys it answers for having a lawful basis to process it. |
| `company` | Information about a company: records by domain, firmographics, who works there. |
| `web-search` | Search the open web and get ranked results. |
| `page-read` | Fetch, scrape or extract the content of a page you name. |
| `social/x` | Read or search X (Twitter). |
| `social/reddit` | Read or search Reddit. |
| `finance` | Markets and money outside crypto: stock quotes, FX, financial statements, market data. |
| `crypto` | Blockchain and token data: prices, balances, on-chain activity, DeFi. |
| `rpc` | Blockchain node access: JSON-RPC methods sent to a chain's node (getLatestBlockhash, eth_call, eth_blockNumber). Added in 2.49.0 (doc 11). |
| `weather` | Weather conditions and forecasts. |
| `image` | Generate, edit or analyse images. |
| `human-work` | Work done by people on request, with evidence of it: physical errands, on-site checks, data collection. |
| `ai` | Model inference and AI agents: completions, embeddings, agent tasks. |
| `data` | Datasets and data utilities that fit no narrower category. |
| `developer-tools` | Tools for software developers: code, repositories, packages, documentation audits. |
| `security` | Security checks, scanning, threat and risk data. |
| `research` | Research and analysis produced on request. |
| `reputation` | Reputation and trust scores for wallets, agents and services. |
| `communication` | Messaging, notifications and channels. |
| `compliance` | Regulatory and payroll compliance checks. |
| `advertising` | Advertising and promotion placements. |
| `infrastructure` | Infrastructure for agents and apps: payment facilitation, databases, hosting. |

The first ten are the feedback's list; `human-work` is what the house already sells (Execution
Market); the other ten are what sellers in the snapshot actually declare. Spellings measured in the
snapshot and where they land (`aliases` in the JSON): `Data`, `data_processing`, `data-enrichment`
→ `data`; `developer` → `developer-tools`; `Inference` → `ai`; `payroll-compliance` →
`compliance`; `payment-facilitator` → `infrastructure`; `search` → `web-search`; and, new on
2026-10-02, `chain-data` → `crypto`. Left unmapped on purpose: `market-data` (its main seller sells
data about the x402 market, the other a prediction market: neither is `finance`), `web`,
`verification`, `mcp`, `test`, `sustainability`, `marketing`, `utility`, and `execution`
(Execution Market's own listing declares `execution`, its product name; an override places its two
routes in `human-work`, as `inferred`).

The single place to change any of this is `config/bazaar_taxonomy.json`. Its loader refuses an
inconsistent file (an alias or override naming an unknown category, an alias shadowing a category,
an override that declares nothing), and a test holds the shipped file, the `/discovery/resources`
docs and this table to the same list.

### 2.4 Content never holds a curated tier above the tools

`CurationManifest::resolve_listing`: a `content` listing never gets `first_party` or `vip`. It
gets what any listing earns on its own — `verified` when alive, `listed` otherwise — and keeps its
`curation.label`. The manifest itself is unchanged; `GET /discovery/stats` `byTier` counts the
tier the listing shows.

### 2.5 Data corrections from the feedback

| Listing | Correction | Mechanism |
|---|---|---|
| 379 `tenjin.blog` essays in `vip` above every API | `kind: content`, tier capped (2.4) | override in `config/bazaar_taxonomy.json` with its evidence; no host in code |
| 388 `market.datapackvibe.com` with empty descriptions, text present in the Coinbase Bazaar | taken from the source that has it | 2.1 (its own v1 options) and 2.2 (any other aggregated copy). No text is typed in by us. |

The other five listings in the feedback are liveness (probe method), handled by the health prober
work, not here.

## 3. API surface (additive only)

- `GET /discovery/resources`: new response-only `kind`, `categories`, `categorySource` and
  `hasInputSchema` on every item; `metadata.category` unchanged. `?category=` matches the seller's
  own `metadata.category` as before (case-insensitive) **or** any spelling of a closed-list id
  against the listing's `categories` — a superset of the old match.
- `GET /discovery/stats`: new `byKind`, `byCategory` (`none` for no category; a listing in two
  categories counts under both), `noDescription` and `noInputSchema`, counted over exactly the set
  `visible` counts — a public count describes only what the listing exposes; `byTier` counts the
  tier the listing shows.
- `kind`, `categories`, `categorySource` and `hasInputSchema` are resolved for every listing (a
  pure function of the record and the vocabulary) and published only on what the listing returns.
- Nothing is renamed, retyped or removed; the health vocabulary is untouched.

## 4. Consumers

Checked against each consumer's default branch (read-only):

- **emporium** `rust/src/modulos/directorio/bazar.rs` — deserializes `description`, `metadata`
  (`category`, `tags`, `provider`), `extensions` (key names only), `health`, `priceFreshness`,
  `source`, `sourceFacilitator` and ignores unknown fields; pages with `limit`/`offset` only. Its
  admission reads the declared category alone (`categorias.rs`, `admision.rs`), and that is
  exactly what it still gets: `metadata.category` is neither rewritten nor filled in from another
  copy. More descriptions and more `extensions.bazaar.info` keys are more data in the fields it
  already reads.
- **karmakadabra** `agents_sdk/bazaar.py` — `?q=`/`?health=`, ranks by `health.status` then
  `curation.tier` (`vip` before `verified`). Same fields, same values; an essay now reads
  `verified` or `listed` instead of `vip`, which is the correction itself.
- **meshrelay** `multibrain/payments.js` — a seller: it builds its own 402 with
  `extensions.bazaar`; it does not read the catalog.

## 5. Verifying after deploy

```bash
B=https://facilitator.ultravioletadao.xyz
curl -s "$B/discovery/stats" | jq '{noDescription, noInputSchema, byKind, byCategory, byTier}'
# The same two counts broken down by source (pages through the whole catalog):
python scripts/bazaar_audit.py --json | jq '.listingData'
```

The snapshot is the catalog as stored, after aggregation lost what it lost, so the description and
schema counts only move after the first aggregation cycle on the new build. Whether thirdweb's v1
options actually carry text for the `datapackvibe` listings is not observable from the stored
catalog; the first post-deploy count answers it. If they do not, those listings fill only when
another aggregated copy of them is fetched, which the per-source fetch cap (`maxItemsPerSource`,
1 000) and the catalog cap decide.

**Measured after the 2.47.0 deploy (2026-10-04)** and closed in 2.48.0: they did not. 138 of the
141 visible thirdweb listings had no description and all 141 no input schema. Coinbase's feed
publishes 32 701 resources; all 144 thirdweb-held URLs are in it, none in its first 1 000 (median
position ~2 981), and its copies carry the description for 139 and the input schema for all 144.
The aggregator now reads past the cap for the sources in `catalog.scanPastCap.sources` (Coinbase by
default), a bounded number of pages a cycle, keeping only copies of URLs the catalog already holds,
which then go through the import's usual rules: the newer copy's terms win (Coinbase's copy is the
newer one for all 144, with the same recipients on the same networks), and 2.2 fills what either
copy lacks. Nothing new enters the catalog that way.
