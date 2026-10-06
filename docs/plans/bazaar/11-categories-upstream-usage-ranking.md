# 11 -- Categories for most of the catalog, upstream, usage counters, and a ranking that answers the task (2.49.0)

**Status**: implemented, not deployed · **Date**: 2026-10-06 · **Origin**: a router partner
re-tested the Bazaar on 2.48.0 (1 765 listings) and will add it as a second source next to
Coinbase's. Its router only uses listings with an input schema and fields by query or body.

## 1. Measured on 2.48.0 (reported, not re-measured here)

| | |
|---|---|
| listings with a category | 200 of 1 765 (115 of them `data`), all from what the seller declared |
| listings with a usage count | 0 (`settlementCount` / `lastSettledAt` move only on `discoverable: true`) |
| an `upstream` field | none |
| dominant hosts | tenjin.blog 349 (content), agentstools.dev 106, rallylive.ca 100 |
| a Solana RPC | none |
| api.losbeto.xyz | 0 results, after 2.48.0 stopped holding it for a payTo drift |

Five requests the partner reproduced, each now a test in `tests/bazaar_search.rs` over the fixture
catalog plus the listings that made it wrong. **Run against 2.48.0, all five fail as reported**:

| request | 2.48.0 (`tests/bazaar_search.rs` on the base) | 2.49.0 |
|---|---|---|
| `keccak selector` | the CSS scraper first, the keccak tool second | the keccak tool; the scraper is not returned |
| `solana rpc getLatestBlockhash` (no Solana RPC) | 31 results, the HyperEVM RPC first | 0 results; with a Solana RPC, it is first |
| `phone number lookup` | an essay and two SMS senders ahead of a lookup | lookups first; no SMS sender |
| `stock quote` | an essay and a stock history in the top 3 | the three quote listings |
| `trending meme coins` | the meme generator second | the trending-coins listings; no generator |

## 2. Categories for the listings that declare none

Rules in `config/bazaar_taxonomy.json` (`inference`), compiled and checked strictly
(`src/discovery_taxonomy.rs`, `Inference`):

- **What is read**: the listing's host, path, description and the field names and descriptions
  of its declared schema (`extensions.bazaar`). Never tags, never the provider.
- **When**: only when the seller declared no category at all, and the listing is not content. A
  declaration that maps to nothing (`utility`, `market-data`) is the seller's word, left out, not
  reinterpreted (the decision of doc 10 stands).
- **How**: phrases normalized with the search tokenizer, matched as consecutive words of one
  field. 2 points per distinct strong phrase, 1 per distinct weak one; a category from 2 points,
  best first (ties in file order), at most two; `data` is a fallback, assigned only when no other
  category reaches the threshold. Served as `categorySource: inferred`.
- **Tests**: every rule against a listing it must place first and a near miss it must not place
  (`every_inference_rule_places_its_listing_and_not_its_near_miss`), plus silence-only, field
  boundaries, camelCase and digits, the two-category cap and a strict loader.
- **New category `rpc`**: blockchain node access (JSON-RPC methods). Aliases `json-rpc`,
  `jsonrpc`, `rpc-node`, `rpc-provider`.

Measured on the fixture catalog (shaped after the 2026-10-01 catalog; `tests/bazaar_search.rs`
`most_of_the_fixture_catalog_resolves_to_a_category`): **2 -> 1 154 of 1 999 listings (58 %)**,
1 154 of the 1 620 that are not paid essays (71 %). On the real catalog the share depends on what
its listings say; after deploy, `GET /discovery/stats` `byCategorySource` and `byCategory.none`
read it, and the ignored `snapshot_report` of `tests/bazaar_listing_data.rs` prints it for a copy
of the stored catalog.

## 3. `upstream`

A second closed list in the same file (`upstreams`): `exa`, `tavily`, `firecrawl`, `serpapi`,
`brave-search`, `jina`, `perplexity`, `hunter`, `apollo`, `fullenrich`, `coingecko`,
`coinmarketcap`, `openai`, `anthropic`. Response-only `upstream` + `upstreamSource`:

- `declared`: `metadata.upstream` (new, stored verbatim, 128 characters like the other metadata
  fields) or `extensions.bazaar.upstream`, an id or name of the list in any case. Any other value
  is kept and not published, and stops inference, like a category.
- `inferred`: the host is the vendor's own domain or a subdomain of it; a path segment or host
  label is the vendor's segment (`/api/hunter/...`); the description names it in a form that is
  not a common word (`hunter.io`, `perplexity ai`, never bare `perplexity`).
- Never for content. **Not done**: inferring it from response headers. The prober keeps the
  payment terms of a 402 and no other response header; reading vendor headers is a separate
  change to `discovery_health`.

## 4. Usage counters

`src/discovery_usage.rs`. `usage: {lastSettledAt, calls30d, uniquePayers30d, asOf}` on every
listing, from the settlements the transaction store records (`TransactionStore::settles_since`,
implemented for DynamoDB: one Query per day partition from the cursor, filtered to successful
settles, paginated, at most `MAX_WINDOW_SETTLEMENTS`). Every replica reads the store every
5 minutes, the first time the whole 30-day window, then only what is newer than the newest record
it holds, so all replicas serve the same counts. A listing whose URL has a query is counted by that
exact URL; one without, over every query its buyers sent. An EVM payer in two spellings is one
payer.

**A floor, never a ledger**, said in the field's doc and in `/docs`: the record is written after a
settlement resolves and is lost when the store is unreachable; settlements through other
facilitators are not seen; a payment counts only when the URL bought is the listing's own. A
deployment without `TRANSACTIONS_TABLE_NAME` publishes no `usage` rather than zeros. Usage does not
enter the ranking: a count anyone can raise by paying themselves is not a relevance signal.

## 5. Ranking

All under `sort=relevance`; `sort=tier` (the substring search of 2.46.1) is untouched.

1. **Coverage.** BM25 is scaled by the square of the share of the request the listing covers,
   each word weighted by its idf. One word repeated in path, description and schema no longer
   beats a listing that says the whole request.
2. **Words as written.** A token that is letters followed by digits is also read without the
   digits (`keccak256` -> `keccak`); a camelCase word is also kept whole (`DeFi`, `LinkedIn`,
   `HyperEVM`), which the camelCase split had cut into pieces nobody types.
3. **Another chain is another request.** A request that names a chain never gets a listing that
   names only other chains (`CHAINS` in `src/discovery_search.rs`: names that are rarely anything
   else; payment networks are never read).
4. **Another category is another request.** The request's own words go through the inference
   rules; when they fall in a category, a listing in other categories does not answer it, and a
   listing in no category (or only `data`) must cover at least half of the request
   (`COVERAGE_FLOOR`). Content stays in such a result at a quarter of its relevance, behind the
   tools.
5. **Literal matches are kept.** A listing that contains the request word for word is never
   excluded by 3 or 4: everything the substring test of 2.46.1 kept is still kept.
6. **Grouping by recipient.** At the top of a relevance result a host keeps two places, a payTo
   two, a templated family one; the rest follow in relevance order. Ranking only: nothing is
   dropped, and a first-hand listing is never refused or evicted for it.
7. `ticker` left the price group of the lexicon: a ticker is a symbol, not a price.

Benchmark (`tests/bazaar_search.rs`, top 3 of 36 places, fixture with one seller per host):

| | 2.48.0 | 2.49.0 |
|---|---|---|
| intents as written, default order | 26 / 36 | 32 / 36 |
| intents as written, `sort=relevance` | 27 / 36 | 34 / 36 |
| 10 held-out paraphrases, EN and ES | 21 / 30 | 25 / 30 |

Latency, debug build, same fixture, two runs each on the same machine: a search p50 49-51 ms
(2.48.0) vs 47-49 ms; without `q` 82-95 ms vs 74-79 ms. The first search after a catalog write
rebuilds the index and now also classifies each listing for its category set: 149-155 ms vs
187-259 ms. The `?category=` filter reads categories resolved once per catalog generation.

## 6. The quarantine and a full catalog (api.losbeto.xyz)

`DiscoveryRegistry::protected_in` decides what trimming and admission may not evict. Until 2.49.0
it was the exposed set only, and a quarantined listing is never exposed: so a copy held in
quarantine ranked as pending and was the **first** thing a full catalog evicted -- and eviction also
prunes its health record, the hold itself. api.losbeto.xyz was held for a payTo drift (its
catalog copy declares Base, the live 402 added Solana; `src/discovery_health.rs` tests) while the
catalog sat at 2 000 of 2 000. Coinbase's feed lists it; if the catalog held it as an aggregated
copy, the code confirms the reported hypothesis is possible (a first-hand record is never
evicted, held or not). If its feed copy also sits past the per-source cap,
where 2.48.0 only reads copies of URLs the catalog still holds, an evicted copy does not come back
by that route; whether it does is read on a running task, not here.

Now a copy quarantined less than `QUARANTINE_PROTECTION_SECS` (7 days: two probes at the longest
backoff and a day to spare) is protected like an exposed one: a newcomer never displaces it, and
with no other pending copy left a full catalog takes no newcomer. Past the week it is a listing
that stayed broken, evictable like any pending copy. Tests:
`a_full_catalog_never_evicts_a_hold_being_judged` (`src/discovery.rs`) and
`a_hold_is_protected_for_its_window_and_no_longer` (`src/discovery_health.rs`).

This protects what is held from now on; it does not bring back what was already evicted. Whether
losbeto is held, pending or absent on a running task is read from `GET /discovery/admin/pending`.

## 7. Verifying after deploy

```bash
B=https://facilitator.ultravioletadao.xyz
curl -s "$B/discovery/stats" | jq '{visible, byCategorySource, byCategory, byUpstream}'
curl -s "$B/discovery/resources?q=keccak%20selector&limit=3" | jq '[.items[].url]'
curl -s "$B/discovery/resources?q=solana%20rpc%20getLatestBlockhash&limit=3" | jq '.pagination.total, [.items[].url]'
curl -s "$B/discovery/resources?limit=5" | jq '[.items[] | {url, usage, upstream}]'
```
