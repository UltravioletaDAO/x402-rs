# Bazaar search ranking, router filters and the host share (2.47.0)

**Why**: a router partner tested the Bazaar as a live source on 2026-10-01 (v2.46.1, 1 999
listings): twelve everyday agent intents sent as written returned nothing, a one-word query came
back ordered by curated tier, routers had only exact-match filters, and four hosts held 1 043 of
the 2 000 slots. This is the search half of that report; keeping seller text and schemas, and
probing with the declared method, are separate changes.

## 1. What `q` was (measured at `ff0c6404`)

- `src/handlers.rs:2634` -- `MAX_SEARCH_LEN = 128`; `:2802` answers 400 above it.
- `src/discovery.rs:1171-1175` -- the needle is trimmed and ASCII-lowercased once;
  `:1674-1700` -- kept if url, description, provider, category or a tag CONTAINS it.
- `src/discovery.rs:1210-1226` -- ordered by tier, then liveness, then `lastUpdated`. No
  relevance at all; ties fell to the map's iteration order.
- `src/discovery.rs:1605-1672` -- every filter is equality (`category`, `provider`, `tag`,
  `source`, `sourceFacilitator` ignoring case; `network` exact).
- `src/discovery.rs:160` and `src/discovery_config.rs:59` -- the 2 000 cap;
  `:193-212` admission by date, `:238-269` eviction of the oldest aggregated copy. Nothing per host.

## 2. What it is now

`src/discovery_search.rs` holds all of it; `src/discovery.rs::list` calls it.

- **Relevance**: BM25 (k1 1.2, b 0.75) over host, path, description, provider, category, tags and
  the field names and descriptions of `extensions.bazaar`, weighted per field (path 1.5, tags and
  category 2, schema 0.5). A fixed lexicon joins the catalog's own vocabulary across English and
  Spanish; an alternative scores at half weight and a listing is credited once per typed word, with
  the best of its group. No model, no embedding, no call out.
- **Tokenization is Paarce's**, copied byte for byte from emporium
  `rust/crates/paarce/src/baseline.rs@748cf72` (`tokens()`, the 197 `PALABRAS_VACIAS`,
  `es_significativo`), between markers, so the two can become one crate. This module adds only a
  camelCase split before it, a 32-character cap and light stemming after it.
- **Tier is a multiplier** (first_party x1.3, vip x1.2, verified x1.1), never the order.
- **Diversity**: in relevance results a host keeps at most two places at the top; the rest of its
  results follow every other host's. Nothing is dropped.
- **Compatibility**: without `q` the listing is exactly what it was, plus a final tie-break by
  `url` (deterministic pages across replicas). A one-word `q` keeps 2.46.1's substring match and
  order by default; a `q` of two or more words, or over 128 characters, gets relevance. `sort=tier`
  and `sort=relevance` force either. Under relevance a `q` of up to 128 characters still keeps every
  listing the substring test kept, after every scored one.
- **Index**: an inverted index stamped with the catalog generation, which every write moves
  (`DiscoveryRegistry::write_catalog`); the first search after a write rebuilds it.

## 3. Router filters

`maxPriceUsd` (any dollar-stablecoin option at or below, compared in atomic units),
`method` (`GET` | `POST` | `PUT` | `PATCH`, as declared; GET when an HTTP listing declares
none), `hasInputSchema`, `kind` (`api` | `content`), `excludeHost` (comma-separated, with
subdomains). A value that cannot be applied is a 400 that names the parameter.

**One reading per fact** (settled when the three changes of the release were joined): `kind` is
the listing's classified kind (`discovery_taxonomy::classify`, the value served as `kind`);
`hasInputSchema` is `DiscoveryResource::has_input_schema`, the rule behind the field a listing
reports; `method` is the health prober's reading of the declaration
(`discovery_health::declared_request`: `info.input.method`, else the JSON Schema input's method,
else POST when a body is declared), so `method=POST` finds the listings the prober sends a POST.
That reading turns a declared `HEAD` or `DELETE` into `GET`, so `DELETE` is not a value the
filter accepts. A method declared only in a 402's `resource.method` is not stored and so not
seen here.

## 4. The host share: an order of eviction, not a cap

When a FULL catalog has to make room (`enforce_capacity`, and admission mirroring it so nothing
churns), every aggregated copy the public surface does not show goes before any copy it does --
the curated bazaar exposes only verified-alive listings, and a pending one (never probed, or not
answering a valid 402 to its declared method) never displaces an exposed one; a newcomer is
pending by definition. Within each group: the duplicates of a templated family, an exposed member
kept over a pending one and then the newest; then a host's copies beyond
`DISCOVERY_MAX_HOST_SHARE_PCT` (default 5 %, 100 at the default cap, never fewer than 50), pending
ones first; then the oldest copy, as before. What counts as exposed is asked in one place,
`DiscoveryRegistry::exposed_in`, the verified-alive rule the listing applies. A catalog with room is never trimmed
for it, and a first-hand listing is never evicted. `GET /discovery/stats` `topHosts` and
`GET /discovery/config` `catalog.maxPerHost` show it on a running task.

A family is the host plus the path with each variable segment written `*`: a segment with a
digit (`0042`, `0x8335...`, `v1`), an all-capitals ticker (`AAPL`), a placeholder (`{id}`,
`:id`), or -- as the LAST segment -- a generated slug of four or more words, which stands for its
first word (`demand-company-oracle-revenue` -> `demand-*`). The slug rule exists because of what
the live catalog actually holds; the digit rule alone found almost nothing there.

Measured on a copy of the catalog as persisted at 2026-10-01 20:03Z (40 minutes before the
partner's dump; the copy stays outside the repository), with the same rules re-implemented to
count them:

| | |
|---|---|
| records / hosts | 2 000 / 182 |
| first-hand (`self_registered`) / aggregated | 892 / 1 108 |
| four largest hosts | 400 data packs (aggregated, no description), 381 essays (first-hand), 167 ranking pages (first-hand), 106 API endpoints (first-hand) |
| aggregated copies that are family duplicates | 470 -- 398 of them the data-pack host, whose 400 listings are one template |
| hosts over a 5 % share once families are collapsed | three, all first-hand: none of it evictable |

So on that catalog the family rule is what makes room -- up to 470 slots for coverage the feeds
offer -- and the host share adds nothing yet, because every host above 100 listings registered
them first-hand. The share stays as the second rung for when an aggregated host crowds without
a template. `tests/bazaar_search.rs::snapshot_report` (ignored by default) runs the real code
over such a copy: `BAZAAR_SNAPSHOT=<copy> cargo test --test bazaar_search -- --ignored`.

## 5. The benchmark

`scripts/bazaar_search_fixture.py` writes `tests/fixtures/bazaar/search-catalog.json` (1 999
listings shaped like the measured catalog) and `search-intents.json` (the twelve intents, the URLs
that do each job, keywords, ten held-out paraphrases in English and Spanish). JSON so Paarce and
KarmaKadabra can run the same set. `tests/bazaar_search.rs` runs it; numbers in §6.

## 6. Measured

Same fixture, same file, run against `ff0c6404` (2.46.1) and against this change, over the rows
the curated bazaar exposes (the fixture marks the rest `pending`; the generator says how that is
modelled, and the numbers are the same over the whole catalog); "does the job" is the fixture's
strict judgement (top three, 36 places):

| | 2.46.1 | 2.47.0 |
|---|---|---|
| intents as written, default order | 4 / 36 | 25 / 36 |
| intents as written, `sort=relevance` | -- | 26 / 36 |
| ... with paid content not in the VIP tier | -- | 27 / 36 |
| ... with `excludeHost` on the content host | -- | 34 / 36 |
| best of 2-4 hand-picked keywords per intent | 20 / 36 | 20 / 36 (`sort=tier`) |
| 10 held-out paraphrases, EN and ES | 0 / 30 | 21 / 30 |

Every place 2.47.0 misses in the 12 is one of twelve VIP essays titled with an intent's own words;
lexically they match as well as the service does, and telling them apart is the `kind` field's
job, not ranking's. The held-out misses include a gap in the shared stopword list (no `what`,
`how`, `get`), noted for the shared tokenizer rather than patched in the copy.

Latency, debug build, same catalog: a search 5-7 ms p50 and 9-12 ms p95 (2.46.1: 1.2-1.5 ms); the
first search after a catalog write 50-70 ms (index rebuild); without `q` unchanged.

On a copy of the real catalog (2026-10-01 20:03Z, health overlay absent) the 2.46.1 search
returns nothing for 9 of the 12 intents as written, and paid essays for one of the other three;
`snapshot_report` prints both searches' top three for a person to judge.

## 7. Should the cap go up?

Not as part of this. Memory is not the constraint it was: ~22 KB of RSS per record
(`discovery.rs`, `DEFAULT_MAX_RESOURCES`), so 4 000 would be ~100 MB of a 2 GiB task. The probe
budget is: 2 probes/s x 60 s = 120 per tick (`discovery_config.rs`), so a full sweep of 2 000 takes
~17 minutes and 4 000 would take ~34. And on the measured catalog the room is not where a bigger
cap would put it: 892 of the 2 000 slots are first-hand registrations, 654 of them from three
hosts, and no eviction rule touches those by design. Raise the cap only after the family collapse
has run for a cycle, if `topHosts` shows the freed room filled with distinct services and the
import log still shows `over-capacity` refusals. It is one variable, `DISCOVERY_MAX_RESOURCES`,
and `defaults_match_what_the_incident_settled_on` pins its default on purpose. Whether a host may
register hundreds of listings first-hand is a curation decision, not a capacity one.
