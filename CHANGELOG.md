# Changelog

## [2.49.0] - 2026-10-06

### Changed

- **Zama `fhe-transfer` is switched off, and switchable** (decision 171).
  `ENABLE_ZAMA` (Terraform `enable_zama`, default `false` in production and in
  `production.auto.tfvars`) decides whether this facilitator offers the scheme;
  only `true` or `1` turn it on, anything else is off. Off: `fhe-transfer` is
  gone from `/supported` (and with it from `/networks.json`, `/accepts` and the
  MCP `x402_supported` tool); `POST /verify` and `/settle` answer such a
  payment `400` with `invalidReason`/`errorReason` `unsupported_scheme` and
  never call the FHE Lambda; `/discovery` marks an `fhe-transfer` offer
  `settleable: false` with the new reason `scheme-not-served`; and the landing
  card, `/networks`, `/x402`, `/bazaar`, `/docs`, the MCP tool schemas,
  `llms.txt`, `llms-full.txt`, `index.md`, `skill.md` and `.well-known/x402`
  stop naming it (`src/zama.rs`, one table of cuts applied when served; the
  skills index re-stamps the digest of the `skill.md` it serves). On, every
  one of those is byte for byte what it was. The proxy code stays.
- **The Zama stack is one switch** (`terraform/environments/zama-testnet`,
  `enable_zama`, default `false`): every resource and data source is counted,
  `moved.tf` keeps the existing addresses, the artifacts bucket gains
  `force_destroy` and the RPC secret is deleted without a recovery window, so
  `false` leaves the state empty and `true` rebuilds it. Order: deploy the
  facilitator off first, then destroy the stack; the README's "On/off" has the
  one-time apply that has to come before the destroy and the way back.

## [2.48.0] - 2026-10-04

### Changed

- **The payTo drift check compares recipients per network.** A recipient the
  listing declares is never a drift, on any network. One it does not declare
  is a drift when it is offered on a network the listing declares, or on a
  network the prober cannot name (fails closed: an EVM chain is its chain id,
  every other family its CAIP-2 namespace, and an unknown namespace such as
  `aws:base` names nothing). One offered on a network the listing does not
  declare is an extra way to pay, not a changed one, while the same transport
  still offers a payable option to a declared recipient on the network it is
  declared on: the listing is verified and stays listed, the extra recipient is
  logged at WARN with its network (`paytoswap: live 402 adds a payment option
  on a network the listing does not declare`) and kept in the observed terms
  when the catalog can read the option, and it is never adopted into the
  listing's `accepts`. A mention of the declared address does not count -- in
  an option nobody can pay, loose in the document, on another chain, or only in
  the other transport -- and without that offer the extra options are a drift.
  Each transport is judged on its own and the worse verdict stands. Measured on
  2026-10-04: api.losbeto.xyz (Base, plus Solana) and one host serving 183
  listings (Base, Arbitrum and Polygon, plus `stacks:1`) were held for adding a
  network; x402.tavily.com/search pays another Base recipient than every
  aggregated copy declares, and stays held.
- The declared offer that lets an extra network pass is matched as a client
  matches it: the exact CAIP-2 network (Solana devnet does not stand for
  mainnet), the asset and recipient as written outside EVM (a Solana mint in
  another case is another mint; only an EVM address, written with a literal
  `0x`, is compared in any case), and a scheme the protocol's `Scheme` takes
  literally (`EXACT`, ` exact ` do not count). Anything else is a drift.
- That offer must sit in the same list as the extra option: `accepts` and
  `paymentRequirements` of one document are judged apart, like the two
  transports, so the declared offer in one does not vouch for an option in the
  other. A v1 network name counts only as the wire name the derived serde of
  `Network` reads (`base`, not `base-mainnet`, `bnb` or any other
  `Network::from_str` alias).
- An upstream page that does not parse no longer panics when its error preview
  would cut a multibyte character at byte 500.
- **The `PAYMENT-REQUIRED` header is decoded as forgivingly as the clients
  that pay it.** It used to accept only padded standard base64 or unpadded
  URL-safe base64, so a header in any other spelling Node's `Buffer` or the
  browser's `atob` reads (unpadded standard, padded URL-safe, mixed alphabets,
  whitespace or stray characters, extra padding or bytes after it, non-zero
  trailing bits, a dangling final symbol, invalid UTF-8 in the JSON) counted as
  absent, and the hijack check judged the body alone: a body keeping the
  declared offer hid a header paying another recipient. Now either alphabet is
  read, padding is optional, characters outside the alphabets are skipped,
  decoding stops at the first `=` and the bytes become text with invalid UTF-8
  replaced, as `Buffer` does.
- A drift hold no build with the per-network rule has judged is probed once
  more straight away instead of after its 72-hour backoff. It still needs two
  clean challenges in a row to come back.
- **A newer copy that declares other recipients is re-probed at once.** When an
  import replaces a listing with a copy whose `(network, payTo)` set differs,
  the change is logged (`paytoswap: declared recipients ... refreshed from a
  newer copy`, at WARN when the listing is held for drift) and the listing is
  queued for revalidation, so a drift hold is
  re-judged against the source's new terms within the hour. The baseline only
  ever comes from a catalog source, never from the 402 it is checked against.
- **The aggregator reads past the per-source cap for copies of listings it
  already holds.** For the sources in `DISCOVERY_SCAN_SOURCES` (default
  `coinbase`), after the first `maxItemsPerSource` items it reads up to
  `DISCOVERY_SCAN_PAGES_PER_CYCLE` (8) pages of `DISCOVERY_SCAN_PAGE_SIZE`
  (1 000) a cycle, resuming where the previous cycle stopped, and keeps only
  copies of URLs the catalog holds; nothing new enters the catalog this way.
  Those copies go through the import's usual rules: the newer copy's terms win,
  and descriptive gaps are filled from whichever copy has the text. A page that
  does not parse is stepped over; one the source does not serve is retried next
  cycle. Measured on 2026-10-04: Coinbase publishes 32 701 resources; all 144
  listings held from thirdweb are in it, none in the first 1 000; its copy is
  the newer one for all 144 and names the same recipients on the same networks
  (two spell Solana `solana:mainnet`, the same family for the drift check), and
  it carries the description for 139 and the input schema for all 144.
  Published in `GET /discovery/config` as `catalog.scanPastCap`.
- `POST /discovery/register` documents `extensions.bazaar` (`info.input`,
  `info.output`, `schema`) in `/docs`; it was already stored verbatim and is now
  pinned by tests through the HTTP route.

## [2.47.0] - 2026-10-02

### Changed

- **The Bazaar exposes only what is verified alive.** `GET /discovery/resources`,
  `GET /discovery/stats`, the `/bazaar` page and the uptime attestation now
  cover only listings whose last probe, made with the request the listing
  declares, read a valid x402 challenge in a 402 within the observed-terms
  freshness window, and that are not quarantined. The 402 has to be the
  listing's own answer to that request -- not one another host gave after a
  redirect, nor one a 301/302/303 reached by turning the request into a GET --
  and its challenge has to offer at least one payment option the facilitator
  can read; a challenge with none is not read as one. Auth-gated, degraded,
  quarantined, unprobeable and never-probed listings are no longer listed or
  counted on any public listing surface (`/discovery/config` still reports how
  many records a task holds), and no parameter lists them (`health` can only
  narrow). They stay in the catalog and keep being probed; the first probe that
  verifies one promotes it. Every listing served is `alive`, and `stats.visible`
  equals a full offset walk of the default listing.
- **An MCP endpoint is verified by its handshake.** Its probe sends
  `initialize`, the `initialized` notification and `tools/list`, with the
  session the server assigns sent back to it, and reads the answers as JSON or
  as an event stream. It is verified alive when `tools/list` lists at least one
  named tool, within the same window and out of quarantine; nothing is called.
  The `initialize` and `tools/list` answers have to be the endpoint's own, like
  a 402 (no redirect to another host, no change of method). A
  handshake that lists nothing or does not complete leaves it pending. The
  handshake shows the server is up; it reads no payment terms. So an MCP listing
  on a curated product's host, whatever the scheme or path of its URL, is shown
  only when every option it declares pays one of that product's own recipients
  (`expectedPayTo` in `config/bazaar_curation.json`).
- The persisted liveness overlay is read record by record: a record a build
  cannot read is left out instead of the whole overlay.
- The `/bazaar` page shows one number, the listings verified alive, and drops the
  health filter, the "Listed" tier and the catalog health, sources, networks and
  tiers sections. Featured products appear only while one of their listings is
  verified alive. The landing page's Bazaar block shows that same single number
  (it showed listed endpoints, alive, first-party + VIP and aggregated
  facilitators).
- The `/bazaar` search box sends `q` up to the cap the server publishes in
  `GET /discovery/config` (`search.maxQueryChars`, 400) instead of cutting it at
  128 characters.
- An alive listing is re-probed before its verification leaves the window. A
  record written before this release keeps its listing on the reading the
  observed-terms overlay took in the same probe, when that reading offered
  something to pay, and is re-probed at once; an
  alive MCP record from before this release is re-probed at once by its
  handshake.
- Until a task has read the persisted liveness overlay, an import protects every
  listing it holds (a full catalog takes no newcomer) and the overlay is not
  uploaded; a read that fails is retried.
- **The Bazaar health prober asks each listing the way the listing says it is
  called.** The method comes from the `bazaar` extension
  (`info.input.method`, else the schema's method, else POST when a body is
  declared), and for a listing that declares nothing, from the `resource.method`
  of its own 402. A body method is sent `{}` first; the listing's own JSON
  example (at most 8 KiB) only when `{}` gets a 400 or 422. A listing that
  declares nothing is probed with GET plus one POST `{}` when the GET answers
  405, 400 or 404, and the method that answered is remembered. Still unpaid: no
  payment header, nothing from the listing in the URL or the headers, and our
  own origin -- or a prefix whose owner opts out in `probeGetOnly` of
  `config/bazaar_curation.json` -- only ever gets a GET from an HTTP listing's
  probe (an MCP listing's is the handshake above, our own fixed messages, sent
  to any origin). Until now every listing
  got a GET, so a POST-only service answering 405 or 404 was shown as
  auth-gated or hidden as quarantined; an external router measured 14 of 18
  sampled auth-gated and 15 of 25 quarantined listings answering 402 to a POST.
- At most one extra request per listing per cycle, counted in requests against
  the per-host cap and the tick's budget, on-demand revalidation included: a
  probe that may send one reserves two slots. A request carrying a body follows
  a 307/308 only on its own host; a 301/302/303 becomes a GET without it.
- A health verdict reached with a different request no longer holds a listing
  back: the first cycle after the deploy re-probes those listings, and the
  first 402 to the right request lifts a fail-streak quarantine the GET built.
  A payTo-drift hold is lifted only by two clean challenges in a row: neither a
  new request nor an answer that is not a challenge (401, 405, 400) lifts it.
- A 405 to the method a listing declares reads as degraded rather than
  auth-gated.
- The payTo drift check compares every live recipient except a URN: a quote
  reference such as `urn:x402:agent-pay:see-quote` cannot be paid, and the
  catalog drops that option at import.
- A 402 body is read up to 256 KiB; the challenge header still counts past it.
- Observed terms record the request that drew them (`context.method`).

### Added

- `health.quarantineReason` (`fail_streak` | `pay_to_drift`) while a listing
  is quarantined (shown by the admin pending view, since a quarantined listing
  is not public), `health.probeMethod`, and `health.uptimeBps` /
  `health.probeCount` (the figure the uptime attestation publishes). All new
  optional fields; nothing in the listing is renamed or retyped, and the health
  vocabulary is unchanged.
- `health.verifiedAt`, when the last probe verified the listing, and
  `health.verifiedBy` (`x402_challenge` | `mcp_handshake`), how; `verifiedAlive`
  in `GET /discovery/stats`.
- `GET /discovery/admin/pending`: what is not exposed, with its health, behind
  `BAZAAR_ADMIN_TOKEN` like the other admin routes (404 when it is unset).
- `scripts/bazaar_probe_churn.py methods`: an offline report, from a local
  snapshot or the catalog object plus the health overlay, of which listings the
  change touches; `compare` now splits the probed listings into auth_gated ->
  alive, quarantined -> visible and unchanged, and `--by-method` per method.

### Bazaar: what a listing sells, from every source

- Listings aggregated from a feed in the x402 v1 shape now keep their description and their declared input and output: v1 publishes them on each payment option (`accepts[].description`, `accepts[].outputSchema`), and they are carried to `description` and `extensions.bazaar.info` verbatim, per the bazaar spec's v1 mapping. Only when the resource declares none of its own; nothing is rewritten into another shape.
- When two sources publish the same listing, a copy without a description, a `bazaar` extension or tags no longer erases another copy's: descriptive fields are only ever filled, and only from a source at least as authoritative: a feed's copy never completes the owner's own registration. The terms still follow authority and date as before, and nothing is written that no source published.
- `GET /discovery/resources`: every listing carries `kind` (`api` or `content`), `hasInputSchema`, and, when anything maps, `categories` from one closed list of twenty-one (`people`, `company`, `web-search`, `page-read`, `social/x`, `social/reddit`, `finance`, `crypto`, `weather`, `image`, `human-work`, and ten more the catalog uses) with `categorySource` (`declared`, `normalized` or `inferred`). All four are response-only, resolved from `config/bazaar_taxonomy.json`; `metadata.category` is still served exactly as the seller declared it. `?category=` also matches every listing that resolves to the given category, in any of its spellings, besides the exact seller spelling it matched before.
- Paid content (`kind: content`) never holds the `first_party` or `vip` tier: it gets `verified` when alive (`listed` otherwise, which only the pending queue shows), and keeps its label; the `/bazaar` page still features the publisher while one of its listings is exposed. Pay-per-read essays published under `tenjin.blog/api/read/` are content, at their publisher's request.
- `GET /discovery/stats` adds `byKind`, `byCategory`, `noDescription` and `noInputSchema`, counted over exactly the listings `visible` counts; `byTier` counts the tier each listing shows.
- `scripts/bazaar_audit.py` reports, per source, listings without a description or a declared input, and the `kind`/`category` counts.

### Bazaar search: `q` takes a request in plain words and ranks by relevance

- `GET /discovery/resources?q=` accepts up to 400 characters (was 128) and, for a request of two or more words, ranks by relevance: BM25 over the host, path, description, provider, category, tags and the field names and descriptions a listing declares in `extensions.bazaar`, with a fixed English/Spanish vocabulary (`weather` finds a listing that says `forecast`, `precio` one that says `price`). Nothing is sent anywhere to rank. The curated tier multiplies relevance (`first_party` x1.3, `vip` x1.2, `verified` x1.1) instead of ordering the result, and no host keeps more than two places at the top of a relevance result. Measured over the listings the curated bazaar exposes in a 1 999-listing fixture shaped like the live catalog (`tests/fixtures/bazaar/search-*.json`): the twelve intents of the partner's benchmark, sent as written, put 26 of 36 top-three places on services that do the job, against 4 with the 2.46.1 search and 20 with its best hand-picked keyword per intent. The ten it misses are paid essays in the VIP tier titled with the intent's own words: 27 of 36 when they are not VIP, 34 when the router leaves their host out with `excludeHost`. Ten paraphrases in English and Spanish that the vocabulary was not written against: 21 of 30, against 0 with 2.46.1. Latency over that catalog in a debug build: 5-7 ms per search at the median and 9-12 ms at p95 (the 2.46.1 substring search: 1.2-1.5 ms), 50-70 ms for the first search after the catalog changes, which rebuilds the index; a listing without `q` is unchanged.
- Compatible by default: a listing without `q` keeps its order and its content, and a one-word `q` keeps 2.46.1's substring match and order. `sort=relevance` and `sort=tier` (2.46.1's search, `q` up to 128) choose either explicitly. Under relevance a `q` of up to 128 characters still returns every listing the substring match returned, after every listing a word scored. Clients of `uvd-x402-sdk` for Python get the longer `q` once the SDK lifts its own 128-character check.
- The relevance index covers only the listings the Bazaar exposes and does not suppress. Each field enters cut to the length the import filter lets it have (the declared schema to a description's length), and no more tags than it lets a listing have. The index has budgets of terms and of text, shared evenly among its listings down to a floor; past the floor the listings last in line -- curated tiers first, then the longest held -- are matched by the substring test alone. It is rebuilt off the request's thread, by a task that completes even when the search that started it is gone, one rebuild at a time and at a bounded rate, and only when the catalog or that set of listings changed: a registration refused as a duplicate, or an unregistration of a URL the catalog does not hold, keeps it. While a rebuild runs, or none is allowed yet, searches rank with the index there is.
- A one-word `q` keeps 2.46.1's order even when it splits into several terms (`stock-quote`, `tenjin.blog`); relevance is the default for two or more words.
- The catalog order now breaks its last tie by `url`, so a page walked by `offset` is the same on every replica.
- New router filters, each a 400 that names itself when its value cannot be applied: `maxPriceUsd` (a dollar-stablecoin option at or below it, compared in atomic units), `method` (`GET`, `POST`, `PUT` or `PATCH`, the declaration as the health prober reads it; `GET` for an HTTP listing that declares none), `hasInputSchema` (the rule behind the listing's own `hasInputSchema`), `kind` (`api` | `content`, the listing's own `kind`) and `excludeHost` (comma-separated, with subdomains).
- Tokenization is the one Paarce (Emporium) uses, copied with its provenance so the two can be merged later.

### Bazaar catalog: a full catalog makes room from templated families and crowded hosts first

- When the catalog is at its cap and a new aggregated listing has to displace one, every copy the public surface does not show (not verified alive) goes before any copy it does: a newcomer, never probed, can only take the place of another pending copy, never of an exposed one. Within each group, the duplicates of a templated family go first, keeping one member (an exposed one before the newest); then a host's copies beyond its share of the catalog (`DISCOVERY_MAX_HOST_SHARE_PCT`, default 5 %: 100 listings at the default cap, never fewer than 50); then the oldest copy, as before. A family is a host and a path whose variable segments -- a digit (`/packs/0042`, `/token/0x.../whales`), a ticker (`/stock-history/AAPL`), a placeholder, or a last segment that is a generated slug of four or more words (`/x402/demand-company-oracle-revenue`) -- are read as one template. Admission follows the same order, so nothing comes back as new every cycle. A catalog with room is never trimmed for it, and a first-hand listing is never evicted: the order is among aggregated copies, and a first-hand listing counts against the cap whatever its health.
- Measured on a copy of the catalog as persisted on 2026-10-01: of its 2 000 listings, 470 aggregated copies are family duplicates (398 of them one data-pack template), so a full catalog can take 470 new services without losing a distinct one. The host share frees nothing there yet: every host above 100 listings registered them first-hand.
- `GET /discovery/stats` adds `topHosts` (the ten hosts holding the most listings) and `GET /discovery/config` adds `catalog.maxHostSharePercent` and `catalog.maxPerHost`. The cap itself (`DISCOVERY_MAX_RESOURCES`, 2 000) is unchanged.

### Rate policy: an IP allowlist next to the stack keys

- A request whose client address is on the IP allowlist skips what a recognized `X-UVD-Stack-Key` skips: every per-IP budget and the per-address in-flight ceiling. Its response carries `x-ratelimit-exempt: ip-allowlist` and no `RateLimit-Policy` or `RateLimit`. The body deadline, the body size limit, the task's ceiling of concurrent requests and the ERC-8004 daily write cap still apply to it, as they do to a stack key.
- The address is the one every budget keys on: the last `X-Forwarded-For` entry, the one the load balancer appends. An address written in front of it is never read.
- The exemption is for the operator's own tools, not for pages a browser loads: a request carrying `Origin` or `Sec-Fetch-Site` is charged like anybody's, budgets, budget headers and per-address ceiling included, whatever its address.
- The list lives in a Secrets Manager secret named by `UVD_IP_ALLOWLIST_SECRET` and is re-read every `UVD_IP_ALLOWLIST_REFRESH_SECS` (default 300, between 30 and 3600), so a new address takes effect without a deploy. Its form is a JSON array of strings; entries separated by commas, semicolons or white space are read too, and a document that starts like JSON but is not an array (an object, or JSON that does not parse) is an empty list. Entries are addresses or CIDR prefixes no broader than /24 (IPv4) or /48 (IPv6); addresses that are not public are refused. Unset, the list is disabled and nothing calls AWS; a secret that does not exist or holds no value is an empty list, and so is a document that is not a list, even over a list read before; a read with no answer within 10 seconds is a failed read, and after three failed reads in a row the list is emptied (a role allowed to read the secret only by name, as the task's is, gets an access denial for a secret that does not exist: that reads as `failing`, not `missing`, and empties the list after those three reads); a list not read for four refresh cycles counts as empty whatever happens to the refresher.
- No address from the list reaches an application log, a response or `GET /config`. `/config` publishes `ipAllowlist`: whether it is on, how many entries are in force and how its last read went (`ok`, `unreadable`, `missing`, `failing`, `stale` or `never`), as it stands at each request. A rejected stack key sent from an allowlisted address is logged without the address.
- `emporium` joins the stack services (`UVD_STACK_KEY_SHA256_EMPORIUM`), inactive until its digest is configured.
- New dependency: `aws-sdk-secretsmanager` 1.101; no other crate in `Cargo.lock` changes.

### Terraform

- `aws_secretsmanager_secret.stack_key_digest_emporium`, declared without a value, applied by hand (the pipeline cannot create secrets). The allowlist secret (`var.ip_allowlist_secret_name`, default `uvd/allowlist/home`) is one for the whole stack, created and loaded by hand outside Terraform: no repository declares it.
- `aws_iam_role_policy.ip_allowlist_read`: `secretsmanager:GetSecretValue` on the allowlist secret, by name, for the task role. The image deploy applies it with the other task-role policy (the only role whose inline policies the pipeline may write). Until it is applied the task cannot read the list and exempts no address.
- The task definition gains `UVD_IP_ALLOWLIST_SECRET` (the secret's name, never its contents).
- Emporium's digest reaches the task definition and the execution role only with `var.stack_key_emporium_loaded = true`.

## [2.46.1] - 2026-09-29

- `POST /settle`: a successful x402r escrow settle (`escrow` / `commerce`) or `refund`-extension deposit is now kept under its `Idempotency-Key`, as an `exact` settle is: a retry with the same key and body gets the first response back, byte for byte, with `Idempotent-Replayed: true`, and the same key with another body gets `409 idempotency_key_conflict`. Only successes are kept, so a failed settle can still be retried.
- `POST /settle`: a retry of a successful `exact` settle with the same `Idempotency-Key` and body gets the first response back, byte for byte, with `Idempotent-Replayed: true`. The same holds for responses kept before this release.
- `SettleResponse` reads the settlement hash under `transaction`, `transactionHash` or `transaction_hash`, and under all three at once, which is how the facilitator writes it; `x402-axum`'s `FacilitatorClient::settle` now reads the facilitator's settle response. A document whose names carry different hashes is refused.

## [2.46.0] - 2026-09-27

### `GET /health/ready`: each chain is warned by what its gas costs

- A signer reads `degraded` at its chain's own warning or fewer settles, no longer below one number for every chain. That warning is as many settles as $20 of the chain's gas pays for, at the fee cap the probe just read, never fewer than 20 nor more than 100. At today's costs every chain keeps 100 except Ethereum, whose settle reserves 0.00065 ETH at its 5 gwei floor fee cap: it reads `degraded` at 20 settles or fewer and `ok` above. When a chain's fee cap rises its warning falls toward 20, and when it falls it rises toward 100. `down` is unchanged: below `minSettles` (10).
- The comparison is now inclusive: a signer with exactly its warning's number of settles is `degraded` (on a chain warned at 100, 100 settles read `ok` before and read `degraded` now).
- The dollar cost of a settle comes from a table of reference prices for each gas currency, declared and dated in `src/readiness.rs` (read 2026-09-27). Nothing is fetched while probing. A testnet, a chain whose gas currency the table does not price, and a chain that prices gas at zero get 100. Native Hedera is priced at its max transaction fee, the reservation its settles are counted by.
- Each row carries `warnSettles`, the warning its signers were graded against (absent when the chain could not be read). `thresholds` keeps `warnSettles`, now the ceiling, and adds `warnSettlesFloor`, `warnBudgetUsd`, `warnPricesAsOf` and `warnOverrides`.
- New settings: `HEALTH_READY_WARN_SETTLES_FLOOR` (default 20, between `HEALTH_READY_MIN_SETTLES` and the ceiling) and `HEALTH_READY_WARN_SETTLES_<NETWORK>` (one chain's warning instead of the rule's, e.g. `HEALTH_READY_WARN_SETTLES_ETHEREUM`; raised to the minimum when set below it). `HEALTH_READY_WARN_SETTLES` is the ceiling, default 100 as before.
- Nothing else reads this state: `/supported`, `/networks.json` and Bazaar listings do not, and the landing's dot still lights only under 10 settles or for a reason other than gas.

### Low-balance alarms

- `alerts.tf` derives each chain's floor from that chain's warning (`warn_settles_by_chain`), not from one `warn_settles = 100`. Ethereum's threshold falls from 0.065 ETH (100 settles at 5 gwei) to 0.013 ETH (20 settles), still above its declared 0.0035; every other threshold is unchanged, and every derived alarm's description now names its own warning. A test fails when the map disagrees with the binary at the fee caps the file records, and prints the one to paste; it also fails if the deployment sets any `HEALTH_READY_WARN_SETTLES*` variable the alarms would not follow. Not applied by the deploy: `terraform apply -target=aws_cloudwatch_metric_alarm.chain_balance_low`.
- `scripts/gas_reserve_floors.py` prices each chain's floor at that chain's warning, read from `alerts.tf`.

## [2.45.0] - 2026-09-27

### `POST /register`

- Solana: a half-minted identity is resumed only by a retry of the request that minted it (same `agentUri` and `recipient`, within 24 hours; the record lives in memory, like the EVM recovery record); any other match is `409` with `mint.errorCode = "held_identity_not_resumable"` and `mint.status = "not_minted"`, and nothing is sent.
- Solana: a new mint goes out as one transaction or not at all. One that would not fit (more than 4 `metadata` entries, or over 1232 bytes with long values) is refused with `400` and `mint.errorCode = "mint_not_atomic"`, and a `metadata` key given twice with `400 metadata_duplicate_key`, both before anything is read or sent.
- EVM: when the delivery after a mint fails (the transfer reverts, or its receipt is fine but `ownerOf` is not the recipient afterwards), the identity is retired on the spot (its `agentURI` is set to `https://facilitator.ultravioletadao.xyz/erc8004/retired`, from the wallet that minted it) before the recovery record is written; not when the agent id came from the `totalSupply` fallback instead of the mint's `Registered` event. Repeating the same request within 24 hours puts the URI back and retries the delivery of that same identity: if the URI cannot be put back it is not delivered, and if the delivery fails again it is retired again. The response's `error` says whether the retire went through.

### `scripts/erc8004_custodied_identities.py`

- Sends its own `User-Agent` (public nodes answer `403` to urllib's), retries a call that got no answer (a dropped connection, a timeout, `429`, `5xx`) up to three times with a growing wait, halves an `eth_getLogs` range that times out as it already did one the node refuses, and when the node cannot answer `eth_getCode` at old blocks, including with an HTTP `400`, says to pass `--from-block`. A read that never got an answer stops the run rather than being read as a burned token.

## [2.44.0] - 2026-09-26

### `POST /register` no longer mints what it should not

- `agentUri` must be `https://` on a public DNS name, or `ipfs://<cid>`, and at most 2048 bytes. Refused with `400` and an `errorCode` before anything is locked, read from the chain or sent: `agent_uri_missing`, `agent_uri_too_long`, `agent_uri_malformed` (whitespace, control characters or a `\` anywhere, no host, a host no URL parser accepts, an `ipfs://` authority that is not a bare content id), `agent_uri_scheme` (`http://`, `data:` and anything else), `agent_uri_credentials` (a user or password before the host), `agent_uri_ip_literal` (an IP in any form a URL parser reads as one: dotted, decimal, hex, percent-encoded, IPv6), `agent_uri_non_public_host` (`localhost`, `.localdomain`, `.local`, `.internal`, `.lan`, `.home`, `.corp`, `home.arpa`, `.test`, `.invalid`, `.example`, `.onion`, a single label), `agent_uri_embedded_ip` (four octets inside the name, dash- or dot-separated, on any domain; and the wildcard-DNS services `sslip.io`, `nip.io`, `xip.io` and others) and `agent_uri_tunnel` (`ngrok`, `trycloudflare`, `cfargotunnel`, `loca.lt`, `serveo`, `localhost.run`, `pinggy`, Tailscale `ts.net`, Codespaces `app.github.dev` and others). The domain lists are `config/erc8004_agent_uri_rules.json`, compiled in. Every URI the stack's own callers build (`https://execution.market/{workers,publishers,agents}/<address>`) passes.
- `recipient` is required: `400 recipient_required`. The facilitator used to keep the identity when it was omitted. Naming one of the facilitator's own signers is `400 recipient_is_facilitator`, and the zero address `400 recipient_invalid`. On EVM a recipient with code must answer `onERC721Received` (simulated with `eth_call` from the registry, as `safeTransferFrom` will call it): otherwise the transfer would revert after the mint and leave the identity with the facilitator, so it is `400 recipient_cannot_receive`, and a code read that fails is `503 recipient_check_unavailable`, `retryable: true`. A recipient balance that cannot be read (the duplicate-identity check) is now `503` with no mint attempted instead of being skipped.
- The recipient is screened against the compliance lists `/verify` and `/settle` use (OFAC and the custom blacklist) before any path that delivers an identity. A blocked recipient gets `403 recipient_blocked` and nothing is minted or handed over; lists that cannot be read give `503 recipient_screening_unavailable`, `retryable: true`.
- Refusals keep the usual body (`success: false`, `error`, `network`), add `errorCode`, and on Solana carry `mint.status = not_minted`. `GET /register`, `/docs` and the `/erc8004` page say `recipient` is required and what `agentUri` may be.

### Retiring identities the facilitator holds

- `POST /erc8004/admin/retire-identity` (`{network, agentId, dryRun?}`): points the `agentURI` of an EVM identity one of the facilitator's signers owns at `https://facilitator.ultravioletadao.xyz/erc8004/retired`, through the running service: behind the `ERC8004_ADMIN_TOKEN` gate (404 when unset) and the writer lease, sent with `setAgentURI` from the owning signer through the provider's own nonce manager. It reads the owner and the current URI first: somebody else's identity is `409 not_held_by_facilitator` and nothing is sent; `dryRun` answers `would_retire` with the current URI and the `/register` rules it breaks; an identity already retired answers `already_retired` and sends nothing. A send whose receipt does not come back in time is `504 unconfirmed` with the transaction; repeating the call is safe.
- `GET /erc8004/retired`: the ERC-8004 registration file those identities point at, `active: false`.
- `scripts/erc8004_custodied_identities.py` (read-only): lists every identity the facilitator's wallet still holds on a network, found through the ERC-721 `Transfer`s into it and confirmed with `ownerOf`, with each `tokenURI` judged by the same rules as `/register` (tested against the same corpus as the Rust guard, `tests/fixtures/erc8004_agent_uri_cases.json`). Suspicious URIs are printed defanged; `balanceOf` is checked against the count.

### Infra and CI

- Terraform: `ERC8004_DAILY_WRITE_CAP_ETHEREUM=300` in the task definition, temporarily, while KarmaCadabra backfills its pending Ethereum ratings; the entry is deleted, and the built-in 100 applies again, once that backfill is done.
- The drift gate goes red when the deploy's targeted plan changes an `aws_iam_*` resource the CI user may not write (every role policy but the task role's is explicitly denied to it, on purpose), with the `terraform apply -target=...` to run by hand on 1.9.8 before merging. Until now such a change planned clean, merged green, and failed the deploy half-way. `scripts/drift_gate_iam.py` evaluates each change against the CI policy as it is live in the plan's state and never prints an account ID or an ARN.

## [2.43.0] - 2026-09-26

### Stack identities and admission

- Stack identities: a request carrying a recognized `X-UVD-Stack-Key` is not charged to its address by any per-IP rate limit, and the response names the service in `x-ratelimit-exempt`. One key per service (`execution-market`, `karmakadabra`, `describe-net`, `meshrelay` by default, `UVD_STACK_SERVICES` to change the list); the facilitator holds only the SHA-256 of each (`UVD_STACK_KEY_SHA256_<SERVICE>`, comma-separated during a rotation, empty to revoke) and compares it in constant time. An absent, malformed, unknown or revoked key is charged like any other caller, never answered `500`. It is logged with the caller's address and path, never its value. The key is marked sensitive on arrival and appears in no log line, response or `/config`. `scripts/stack_key.py` writes a key and its digest as two separate secret bodies, one for the client and one for the facilitator. Nobody is exempt until a digest is configured.
- Admission, checked before any route runs, with `/health` never refused. In order:
  - A ceiling of requests in flight per client address (`MAX_INFLIGHT_PER_CLIENT`, default 32). Past it the request answers `429` with code `too_many_concurrent_requests` and `retry-after: 1`. Stack identities skip it, like every per-IP limit.
  - The whole body must arrive within a total deadline (`REQUEST_BODY_DEADLINE_MS`, default 5000). Past it the request answers `408` with code `request_timeout` and the connection closes. A body cut by the 64 KiB limit answers a JSON `413 payload_too_large`.
  - A ceiling of concurrent requests per task (`MAX_INFLIGHT_REQUESTS`, default 512), for every caller, stack identities included. Past it the request answers `503` with code `overloaded` and `retry-after: 1`. The slot is taken only once the body is in, so an upload that never finishes holds none; it is held until the response is produced, not while a stream is written.
- `GET /config`: every per-IP budget in force (routes, period, burst, default, override variables), the stack identities by service name and how many credentials each holds, the admission limits above, and the ERC-8004 daily write limit per network.
- The eight per-IP budgets, their defaults and their overrides now live in `src/rate_policy.rs`, the one place every governor is built and mounted. No default changed. New overrides: `VERIFY_SETTLE_RATE_PER_MS`/`_BURST`, `DISCOVERY_REGISTER_RATE_PER_MS`/`_BURST`, `DISCOVERY_READ_RATE_PER_MS`/`_BURST`, `EVENTS_RATE_PER_MS`/`_BURST`, `ERC8004_WRITES_RATE_PER_MS`/`_BURST`; the identity, secondary-read and human-page variables keep their names.
- Unchanged, and not skipped by a stack identity: the ERC-8004 daily write limit (it protects the gas the facilitator pays) and the RPC provider throttle (`RPC_MAX_CU_PER_SECOND`). An MCP settle forwarded to the writer-lease holder carries `X-UVD-Stack-Key` along with `X-Forwarded-For`.
- Infra (2026-09-26, same 2.43.0 binary, no version change): the task definition maps `UVD_STACK_KEY_SHA256_EXECUTION_MARKET`, `_KARMAKADABRA`, `_DESCRIBE_NET` and `_MESHRELAY` from the `sha256` field of their own Secrets Manager secrets (`facilitator-stack-key-digest-<service>`) through `secrets`, never `environment`, and the execution role may read those four and none of the clients' key secrets. `GET /config` then reports `stackIdentities.active` = 4.

### Escrow on Arc

- Arc and Arc testnet join the escrow networks on the canonical commerce-payments v1.0.0 contracts, through the PaymentOperator v3 interface: `release` is a `capture` and `refundInEscrow` voids what is still capturable. A refund of nothing capturable answers `409 nothing_to_void`, an amount of 0 `422 amount_required_on_generation`, any other amount than the capturable one `422 partial_refund_unsupported_on_generation`, and a failed chain read `502 chain_read_unavailable`, retryable; none of them sends a transaction. `authorize` on Arc checks the payer's signature over `ReceiveWithAuthorization` under the token's domain before anything is sent.
- `/supported` announces an Arc escrow entry only once the declared operator has verified itself on that network: its `ESCROW()` is the declared escrow and its bytecode carries the v3 selectors, read at startup and every 10 minutes. Until then a new authorization answers `503 operator_not_verified`; `release`, `refundInEscrow` and `/escrow/state` never wait on it, and a failed read never stops the process. The Arc `commerce` entry names the canonical v1 contracts.
- Every v3 write on Arc (authorize, capture, void) checks, before signing, that it goes to `paymentInfo.operator` (else `400 operator_mismatch`) and that the address has code (else `422 operator_has_no_code`; a failed read is `502 chain_read_unavailable`, retryable). Every other escrow network keeps exactly the calls it made before.
- The landing's escrow grid and `/docs` list the escrow networks the code serves, Arc included; Arc's icon there comes from `/networks.json`, like every other.

### Interop manifest and RateLimit headers

- `GET /.well-known/uvd-stack.json`: the service's `uvd.stack/1` interop manifest, generated at runtime: `app`, `version` and `git_sha` of the running build (`FACILITATOR_GIT_SHA`, passed as a build argument; `0000000` without it), the `api` and `mcp` doors with `auth: ["none"]`, `service_signer: null`, `charges: false`, liveness and readiness URLs, links to the other agent documents, and `rate_limits`: every per-IP bucket the router mounts, as `limit` per `window_s` on the door it guards. Those are the budgets `GET /config` publishes, with the same numbers. Linked from the API catalog, `/llms.txt`, `/index.md`, `/skill.md`, `/auth.md` and the OpenAPI document.
- `RateLimit-Policy` and `RateLimit` (draft-ietf-httpapi-ratelimit-headers-11) on every response charged to a per-IP budget, `429` included: `"verify-settle";q=30;w=60` and `"verify-settle";r=29;t=2`. A caller that sends at most `q` requests in any `w` seconds is never refused. The name is the one `GET /config` gives the budget (its budget names are now `verify-settle`, `discovery-register`, `discovery-read`, `events`, `identity-read`, `secondary-read`, `human-pages` and `erc8004-writes`); `/mcp` and `/settle` share `"verify-settle"`. A recognized stack identity is charged to no budget and gets `x-ratelimit-exempt` and neither header. CORS exposes `ratelimit-policy`, `ratelimit` and `retry-after`. No limit changed.
- MCP: each tool declares its class in `_meta["uvd/clase"]` (`x402_supported` a read, `x402_accepts` and `x402_verify` the payment rail, `x402_settle` moves money); `x402_supported` publishes an `outputSchema` and returns the same document in `structuredContent`.

## [2.41.0] - 2026-09-24

- `GET /networks.json`: one row per network `/supported` serves, with what a network picker needs and `/supported` does not carry: `id` (the v1 name; the CAIP-2 id for native Hedera, as on `/supported`), `caip2`, `family`, `chainId` (EVM; `null` elsewhere), `testnet`, `displayName`, `explorer` (`base`, plus `tx` and `address` templates with a `{tx}` / `{address}` placeholder), `icon` (absolute URL of a PNG the facilitator serves), `schemes`, and `tokens` (`symbol`, `address`, `decimals`, `eip712`, `usdPegged`, `icon`). The rows are built from the `/supported` body itself: every identifier `/supported` publishes is some row's `id` or `caip2`, and no row names anything else. `eip712` is the domain `/verify` resolves for that deployment. Presentation comes from `config/supported_tokens.json`, now compiled in; a served network it does not describe keeps its row, with `explorer` and `icon` null. Cached five minutes. `/supported` is unchanged. Not to be confused with `/networks`, the HTML page.
- `config/supported_tokens.json` now says what is served: BSC lists AUSD only (its USDC has no ERC-3009 and was never served), Sui USDC only, XRPL testnet XRP, RLUSD and USDC, and Ethereum Sepolia no `exact` token (`exactServed: false`: production configures no RPC for it, and serves `fhe-transfer`, `escrow` and `commerce` there). Native Hedera mainnet and testnet are added. Every network gains `displayName`, `icon` and `explorerPaths`; every token `usdPegged` and `icon`. Two dead explorers are replaced: HyperEVM testnet (`testnet.purrsec.com` answers 404 even on `/`) by `explore-testnet.hyperpc.app`, and Algorand testnet (`testnet.allo.info` does not resolve) by the Pera explorer the landing already linked. Tests fail when the file lists a token `/supported` does not publish for a network, misses one, or misses a network.
- The landing, `/networks` and `/events/live` type no explorer and no icon; they read both from `/networks.json`, and a test fails if either comes back into `static/`. `/x402.js` (loaded as `?v=20260924`) loses `ICONO_DE_RED` and `ICONO_DE_TOKEN` and gains `loadNetworks` / `hydrateNetworks`: landing cards, wallet links and the escrow and upto contract links only name their network, and Sui's and native Hedera's addresses are the `feePayer` `/supported` publishes. `/events/live` links transactions on every network `/networks.json` describes; it knew 14. `/networks` looks as it did: its wallet table keeps its rows, order and labels, with each row naming its network for the icon and the explorer link, and Sui's addresses read from the `feePayer` in `/supported`. If `/networks.json` cannot be read, icons fall back to their monogram and addresses stay as text, never a guessed link, and the landing says its stablecoins are unavailable rather than listing none or counting 0. The Solana Devnet card on the landing opens its explorer again; its `onclick` had a syntax error.
- Infra: Arc mainnet settles through the `arc` key of the `facilitator-rpc-mainnet` secret, mapped into the task definition's `secrets` (`RPC_URL_ARC`), instead of the public `rpc.mainnet.arc.io`, which rate-limits by IP and answers 429 without `Retry-After`. The variable leaves `environment`. Arc testnet has no premium endpoint and keeps its public one, and the balances Lambda keeps the public endpoints for both: it only reads balances, and a Lambda has no `secrets` block to hold a URL with a key.

## [2.40.0] - 2026-09-23

- Durable facilitator receipts for Base: `exact` payments on `base` (`eip155:8453`, USDC and EURC, x402 v1 and v2) are admitted through the receipt service that Arc and native Hedera already use. `GET /receipts` and `/supported.facilitatorReceipts` list `eip155:8453`. Base Sepolia and every other network are unchanged. The before/after table for a Base `exact` caller is in `docs/facilitator-receipts.md`.
- A settlement under a receipt admission (Arc and Base) is no longer retried with a new nonce after the node refuses the prepared transaction on nonce grounds. An admission holds one transaction, so that retry could only allocate and sign a second one that the admission refused to store, and its nonce was never broadcast. It is answered as a failure after the send (2.39.6): `502 upstream_nonce_or_mempool` with `retryable: false`, the stored `transaction` and its `paymentId`, no `Retry-After`, and the receipt `unknown`, which resends reconcile or rebroadcast; the buyer resends the same request, never signs a replacement. The signer's next settlement takes the node's nonce at once, including after a refusal in a `gap` phrasing. Settlements outside an admission retry as before.
- `docs/facilitator-receipts.md`: the Base table also covers requests carrying `X-UVD-Purchase`, the 24-hour Idempotency-Key cache, admissions released before anything was sent (`reservation_abandoned`) and nonce refusals.
- `/skill.md`, `/index.md`, `/mcp.md`, `/llms-full.txt` and the README name Base in the receipts section.

## [2.39.6] - 2026-09-23

- `/settle`: every failure answered after the transaction may have left the facilitator says so in its body: `"retryable": false`, no `Retry-After`, and `transaction` with its `paymentId` whenever they are known. Failures answered before anything was sent keep their answers, including the receipt rail's `503` with `safeToRetry: true`. A client that read a `5xx` without either as transient resent it; for a payment that did mine, that resend fails verification and ends with the buyer signing a second authorization.
- EVM: `broadcast_uncertain` and `receipt_pending` now carry `retryable: false`. The send path fills and signs before sending, so the hash exists before the bytes leave: a send whose answer is lost (a timeout, a dropped connection, a gateway `5xx`, a null or unreadable answer, retries exhausted by the transport) or that the node answers `already known` returns `502 settlement_unconfirmed` with that hash and is not sent again. `AlreadyKnown`, `known transaction` and `already imported` are read the same way; unlike `already known`, those phrasings were not measured against a node. It used to answer `502 upstream_rpc_unavailable` or `upstream_nonce_or_mempool` with `Retry-After: 30`. A node that answered and refused keeps its answer, and nonce handling is unchanged. `already known` is also classified as `broadcast_uncertain` wherever a failure is classified from text.
- Solana, NEAR, Stellar, Algorand, Sui and XRPL answer a submission whose outcome is unknown with `502 settlement_unconfirmed` and the hash in the chain's own encoding: Solana when `sendTransaction` loses its answer, its preflight answers `AlreadyProcessed` (never read as a confirmed success; the RPC's wording was not measured), or a confirmation read fails after it was accepted, and the settlement-account sweep (`settleSecretKey`) the same way, which answered `400 contract_call_failed`; NEAR on the node's own timeout, a routed request or a lost answer; Stellar, Algorand, Sui and XRPL when the submit's answer is lost; Algorand on `already in ledger`, Sui on `-32050` and on `already finalized`, XRPL on `tefALREADY` and `tefPAST_SEQ`. NEAR, Stellar, Algorand, Sui and XRPL used to answer these `200` with `success: false` and no transaction. A refusal by the node keeps its answer. The hash is computed before sending: NEAR's `CryptoHash`, Stellar's signature payload hash, the group's first transaction id on Algorand, the Sui digest, and XRPL's SHA-512Half over `TXN\0` and the signed blob, tested against xrpl-rust's own vector.
- Hedera: a failed write of the signed record, before or right after it was stored, answers `settlement_unconfirmed` with the transaction id, since recovery sends stored bytes later.
- `upto`, `escrow` and the `refund` extension answer a transaction that may have left with `502`, `success: false`, `retryable: false`, `error` (`settlement_unconfirmed` with `transaction` and `paymentId`, or `broadcast_uncertain`) and `errorReason`. `upto` and `refund` answered `400` with the error text, and the escrow scheme answered without the hash. `fhe-transfer`'s `502` carries `retryable: false` unless the request never reached the FHE facilitator or it answered `4xx`.
- A settle forwarded to the task that holds the EVM writer lease, whose answer was lost after it was sent, answers `502` with `reason: forward_unconfirmed` and `retryable: false`, without `Retry-After`. A hop that never reached the holder keeps `503 forward_failed` with `Retry-After: 5`. This applies to every forwarded EVM write.
- Receipts: a failure answered after the send latched or its bytes were prepared, with no chain verdict, is answered and stored with `retryable: false`, without `Retry-After`, and with the prepared `transaction` and its `paymentId` when the answer named none. `502 receipt_response_unreadable` after the settlement ran carries `retryable: false`, the prepared transaction and the receipt; it carried `retryable: true`.
- Docs: `docs/settle-errors.md` groups every `/settle` failure by what it means for a seller; the OpenAPI description of `POST /settle` (table with a body column, the rule, the forward and alternative-scheme answers), `/skill.md`, `/index.md`, `/mcp.md`, `/llms-full.txt`, the MCP server instructions, `docs/facilitator-receipts.md` and `tests/x402/TROUBLESHOOTING.md` say the same.

## [2.39.5] - 2026-09-23

- Landing: the chain logo next to each network name, in the Mainnets and Testnets grids, is at least as large as the stablecoin icons on the same card. The 96 px network images carry 12 px of transparent margin per side, so their 32 px box showed a 24 px glyph (28.9 px for Hedera) next to stablecoins drawn at 32 px, 38.8 px with their ring. Each logo is now sized so its visible glyph is 40 px, using each image's measured opaque share (Arc 100 %, Hedera 90.3 %, Stellar 81.3 %, Polygon 70.8 %, the rest 75 %), and negative margins keep the row's old 32 px height: no card grows, the name stays centred on the logo, and the stablecoin icons, type, colours, borders, grid order and the stablecoin filter bar do not change. The Zama FHE card's logo, the one network logo drawn as SVG, goes from a 30 px ring to 40 px the same way, without moving its card. Measured in a headless browser at 1440 px and 390 px.
- A frontend test decodes every network image, measures its opaque share and fails if a card's painted logo glyph is smaller than a stablecoin pill, if the stylesheet's assumed share drifts from the image, if the logo's width or height stops coming from that computation, or if a card logo gets an inline width or height or returns to the fixed 32 px box.

## [2.39.4] - 2026-09-23

- A configured network stays in `/supported`, and so on the landing, whatever its health. Through 2.39.3 a native Hedera ledger whose health check failed at startup was left out until the next deploy, with no retry: on 2026-09-23 both 2.39.3 tasks started while Hedera's consensus probe timed out (13:30:55Z and 13:31:28Z), and Hedera mainnet was absent from `/supported` (155 entries instead of 156) and from the landing's mainnet grid until a forced redeploy at 13:46Z. An Arc RPC answering for another chain likewise left Arc out. Both are now served, with the same alert as before.
- The startup checks run in the background and never decide whether a network is served. A Hedera health check that fails at startup logs `hedera_health_failed_at_startup` and runs again every 30 s, then every 60 s, until it passes, and then logs `hedera_health_recovered`. An EVM RPC that does not answer `eth_chainId` at startup (Arc included: `arc_rpc_chain_id_unverified`) is asked again on the same schedule until it gives a verdict; a mismatch logs `evm_rpc_chain_id_mismatch` for every EVM network. The only things that keep a network out of `/supported` are not configuring it and not compiling it in.
- `GET /health/ready` lists every configured network with its state and reason, including one that is failing: until now a Hedera ledger that failed at startup was absent from it too. Each entry gains `caip2` next to `network`, so native Hedera, which `/supported` names only as `hedera:mainnet` / `hedera:testnet`, can be matched. EVM RPCs are asked for their chain id on every refresh and a wrong one is `down` with `rpc_chain_id_mismatch` (`rpc: wrong_chain`) instead of being graded on balances read on another chain. Native Hedera reports its own reasons: `rpc_timeout`, `rpc_unreachable`, `store_unavailable`, `signer_key_mismatch`, and a sponsor balance below one max fee is `signer_gas_critical` rather than `rpc_unreachable`.
- Landing: each network card in the Mainnets and Testnets grids shows a small blinking red dot in its lower-left corner while `/health/ready` reports that network `down`, `degraded` for a reason other than gas, or with a signer under 10 settles; it is labelled with the state, in the page's language, and the reason (for example `down: signer_gas_critical`, in Spanish `caída: signer_gas_critical`). Nothing is shown while the network is `ok`, while it is only `degraded` / `signer_gas_low` with 10 or more settles left (Ethereum mainnet read 24 on 2026-09-23: that is for `/health/ready` and the balance alarms, which do not change), for networks the route does not probe, or when the route cannot be read. It refreshes every 60 s, stops blinking under `prefers-reduced-motion`, and changes no card's size, order, type or colours. `/x402.js`, which carries the new helpers, is loaded as `?v=20260923`.
- OpenAPI for `/health/ready` and the `network-startup-probe` alarm's description say the same.

## [2.39.3] - 2026-09-23

- Receipts: a settlement that ends after its admission but before anything leaves the facilitator no longer leaves its authorization `unknown` for good. That covers the EVM writer lease moving between routing and signing, a read, gas estimate, fill or signature that fails, and bytes that cannot be stored; on Hedera, a native transaction ID that cannot be stored. The admission is closed in place: the answer is `503` with `Retry-After`, `retryable: true` and `safeToRetry: true`, and the receipt is `rejected` with `refusalReason: reservation_abandoned`, the provider's code in `diagnosticCode`, no `settlement`, and `retry.action: resend`. The same request, resent, is verified again and admitted again under the same receipt at its next revision, so a client holding the first receipt sees the same `receiptId`. Only that request takes it back: another purchase capability is still `409 receipt_request_conflict`, and concurrent resends admit exactly one payment. Before this, every resend without the admitting binding answered `409 authorization_in_flight` indefinitely and the authorization could not be completed.
- Nothing is released once a transaction may have left: the EVM send path latches before broadcasting, prepared bytes and any transaction named in the answer keep the receipt as it is, and the close is a compare-and-set on the admitting revision, so bytes stored by a write that looked failed also keep it. Those outcomes stay `unknown` and are resolved by reconciliation, as before.
- A reservation whose store write could not be confirmed is never run; if it landed anyway it is closed the same way instead of staying in flight. Admissions and re-admissions are written with an idempotency token, so a write the store client resends after losing its answer reports its original success instead of leaving an admission nobody runs.
- `503 receipt_store_unavailable`, `receipt_signing_unavailable` and `receipt_reservation_uncertain`, answered before anything is sent, now carry `Retry-After` and `safeToRetry: true`, on `/settle` and `/verify`.
- Operator command for admissions stranded by earlier releases: `x402-rs receipts release-abandoned` lists `unknown` settle records with nothing prepared (older than `--min-age-secs`, default 900, or with an expired authorization); `--write --receipt-id <uuid>` closes them the same way, signed with the service key and with the same compare-and-set, so a settlement still holding one cannot store bytes and never sends. Read-only by default.
- Docs: `docs/facilitator-receipts.md` gains "Admissions that sent nothing" and the operator section, and loses the known limit this removes. The OpenAPI `503` of `POST /settle`, `/index.md`, `/skill.md`, `/mcp.md` and `/llms-full.txt` say how to read `safeToRetry` and `reservation_abandoned`.

## [2.39.2] - 2026-09-23

- Low-balance alarms (`chain_balance_low`) never sit below the point where `/health/ready` calls a signer `degraded`: each alarmed EVM mainnet takes `max(SETTLE_GAS_BUDGET × fee cap × warnSettles, its declared floor)`, with fee caps read by `scripts/gas_reserve_floors.py` on 2026-09-23, and native Hedera prices a settle at its 1 HBAR max fee. The derivation only raises floors: ethereum-mainnet goes from 0.0035 to 0.065 ETH and hedera-mainnet from 10 to 100 HBAR, the other 14 keep theirs; each description says which floor applies and how many settles it buys. Tests tie `alerts.tf` to `src/readiness.rs` and fail if a floor falls below the operator's of 2026-09-23 or a fee cap is zero, and `scripts/gas_reserve_floors.py --hcl` refuses to print a zero or a partial map.
- Arc canary: the gas guard is 3× the base fee of the block it just read, never above 200 gwei, instead of a fixed 50 gwei that refused to run during a real spike (81.64 gwei at block 21,205,139 on 2026-09-16); Arc's per-settle gas figures leave `docs/CHANGELOG.md` and `docs/networks/arc-operations.md` for `docs/reports/arc-settle-gas.json`, which `scripts/arc_settle_gas_report.py` rebuilds from the chain.
- Bazaar: `settleable` is decided by the networks this process has a provider for, the map `/supported` iterates, not by the `Network` enum; an offer on Sei (`eip155:1329`) or XDC (`eip155:50`) now reads `settleable: false` with `network-not-served` (one XDC listing read `true` on 2026-09-23).
- `GET /reputation/{network}/{agentId}` (EVM): when the registry refuses one `getSummary` over every client, the route reads groups of 100 clients (at most 32 calls, 4 at a time), isolates the clients it cannot read and answers 200 with a `coverage` object instead of 500; Arc testnet agent 1 (1,315 clients, one with 77,447 entries) reads 1,314 of them in 28 calls. That answer is cached for 60 s per agent and query, one such read runs per network at a time, and `coverage.readAtUnix` says when the registry was read.
- EVM RPC chain id: every configured EVM RPC is asked `eth_chainId` once per task start, in the background, and a mismatch is an alert (`evm_rpc_chain_id_mismatch`, counted by the new CloudWatch alarm `network-startup-probe`) while the network stays in `/supported`; Arc's own check no longer stops the process: a mismatch leaves that Arc network out, a probe that does not answer serves it with an alert; a test fails if an EVM network enters without a declared chain id.
- Native Hedera that fails its health check at startup is left out of `/supported` and alerts (`hedera_health_failed_at_startup`, same alarm) instead of stopping the process for every network; recovery of payments admitted before the restart still runs. No other provider probes its network at startup.
- Docs: the Arc integration plan of 2026-09-15 and its evidence are restored as historical documents, with their 64-hex values abbreviated and the full files at tag `archivo/arc-plan-2026-09-15`; the Hedera plan's scope note moves out of its YAML frontmatter, which parses again.
- `scripts/verify_landing_canonical.py` reads its five files as UTF-8 and its header carries its own output of 2026-09-23; the Arc native-event test runs on both Arc networks; the EIP-6492 refusal no longer carries Arc's text ("Use an EOA signature on this network.").
- Docs: `docs/networks/superficies.md` inventories every public surface where a network appears, generated or by hand, compiled or not, and what test catches its drift, measured on 2026-09-23; a duplicated heading in the 2026-09-17 docs handoff is fixed.
- Public surfaces: Hedera joins the chain-family lists of the landing's meta description (en/es), the MCP server description, its server card and `ard.json`, with a test over `NetworkFamily`; the unreferenced `og-arc-hedera.png`/`.svg` and their route are retired; the README version badge reads the live `/version` and SDK versions move from prose to PyPI and npm badges; the sitemap's `lastmod` values follow their files' commit dates.

## [2.39.1] - 2026-09-23

- Two testnets were published under a chain id that is not theirs, and are now published under the one their RPCs answer for. `celo-sepolia` is chain 11142220 (`eip155:11142220`); through 2.39.0 it was `eip155:44787`, which is Celo Alfajores. `hyperevm-testnet` is chain 998 (`eip155:998`); through 2.39.0 it was `eip155:333`. Measured on 2026-09-23 against the RPCs production uses: `rpc.ankr.com/celo_sepolia` answers `0xaa044c` and `rpc.hyperliquid-testnet.xyz/evm` answers `0x3e6`. The v1 names do not change, and no other network's entry in `/supported` changes: a test republishes production's 156-entry `/supported` through the new code, and only these two networks' CAIP-2 ids differ.
- Both USDC EIP-712 domains are `name` = `"USDC"`, `version` = `"2"` with the real chain id, as the contracts return them. The static table said `"USD Coin"` with the wrong id, which matches nothing on-chain. Celo Sepolia `0x01C5…C44E`: `DOMAIN_SEPARATOR()` `0x23f4…9578`, the old domain `0x9a13…253d`. HyperEVM testnet `0x2B33…D8Ab`: `0xf26c…465e`, the old domain `0xf0e7…ea09`. A plain EOA signature is still judged by the token's own simulation, so it verified before too; the static domain is what the EIP-6492 validator and DX402's payer-key recovery hash against.
- A v2 request naming `eip155:11142220` or `eip155:998` is now a payment on that network. Before, the conversion refused it as `Invalid CAIP-2 format`, so a v2 client naming the real chain could not pay.
- `eip155:44787` is not kept as an alias. `/verify` and `/settle` answer it with `400` and code `network_retired`, carrying `network` and `replacement` (`eip155:11142220`), in v1 and v2 and before the idempotency cache. The message reads "`eip155:44787` is not served; `celo-sepolia` is `eip155:11142220`". `POST /accepts` keeps `reason: network_unknown` and names the replacement in `detail`.
- `eip155:333` gets no such pointer. 333 is registered to another chain, EthStorage Mainnet, so pointing its callers at HyperEVM would be the wrong advice; it is refused like any chain this facilitator does not serve.
- Nothing in the stack sends either old id: neither SDK (npm 2.97.0, PyPI 0.88.0) contains them, and the discovery catalog held no resource on 44787 or 333.
- The discovery catalog no longer maps a feed's `alfajores` / `celo-alfajores` onto celo-sepolia: they keep their own id, `eip155:44787`, and read as unsettleable.
- Static surfaces follow: `/.well-known/x402`, the icon map in `/x402.js`, the Celo Sepolia wallet card's explorer link (`sepolia.celoscan.io`; the Alfajores explorer redirects to its home page), `config/supported_tokens.json`, `.env.example` (neither testnet's RPC there resolves any more) and the README testnet table. A test now fails when `/.well-known/x402` or `/x402.js` stops matching `Network::to_caip2`.
- `/skill.md` (and `/llms-full.txt`, regenerated) said HyperEVM testnet's USDC is named `"USD Coin"`; it is `"USDC"`. Base is now the example of a name that flips between mainnet (`"USD Coin"`) and testnet (`"USDC"`).
- The ERC-8004 registries on Celo Sepolia are unchanged: read on 11142220 through two RPCs, all three are 130-byte proxies with the Base Sepolia implementations and `getVersion()` = `2.0.0`.

## [2.39.0] - 2026-09-23

- Receipts: a resend of an admitted authorization gets the original answer back only when it carries the `X-UVD-Purchase` capability or the `Idempotency-Key` that admitted it. That is how a lost response is recovered, and it is unchanged, including `202 settlement_in_progress` while the payment is in flight. Without that binding, `/settle` answers `409 authorization_already_settled` (confirmed) or `409 authorization_in_flight` (pending or unknown), carrying the receipt, and `/verify` answers `isValid: false` with the same reason, read from the stored receipt without simulating the authorization again. A storage fault while the binding is resolved answers `503 receipt_store_unavailable` on both, never a verdict. The Idempotency-Key must be the same value on `/verify` and `/settle`. Neither carries `success: true` or `Idempotent-Replayed`. A payment made under a purchase context still answers `409 receipt_request_conflict` to any other context, without its receipt, and a rejected payment is replayed as before. This applies to Arc and native Hedera, the networks with receipts today.
- Receipt admission never takes a request that `/settle` routes to its own settlement path, even when its inner requirements say `exact`: the `upto`, `escrow`/`commerce` and `fhe-transfer` schemes (top level, `paymentPayload` or x402 v2 `paymentPayload.accepted`) and the x402r `refund` extension.
- Receipts lowercase EVM addresses for every EVM network, not only for Arc by name. Nothing changes for Arc.
- Durable receipts for Base, prepared but not announced: the tests drive Base's `exact` path through admission (concurrency, replays, address normalization, the local DynamoDB test). `GET /receipts` and `/supported.facilitatorReceipts` still list Arc and native Hedera only, and a Base settle keeps its current responses.
- The shared signed vectors in `tests/fixtures/facilitator-receipts-v1.json` gain Base USDC and EURC. A new test rebuilds every vector from its synthetic inputs, so the file cannot drift from what the code signs.
- Docs: `docs/facilitator-receipts.md` gains "Replays of an admitted authorization"; the OpenAPI `202`/`409` of `POST /settle` and the receipts section of `/index.md`, `/skill.md`, `/mcp.md` and `/llms-full.txt` describe the binding.

## [2.38.0] - 2026-09-23

- Rater-signed ERC-8004 feedback on Arc mainnet: `POST /feedback/evm/prepare` and `/feedback/evm/submit` (and the response pair) serve `arc` through Execution Market's v4 `FeedbackDelegate` `0x955Cc9fB…84f1`. Read on two RPCs before it was added: 5857 bytes, `VERSION()` = 4, pinned to the mainnet reputation registry `0x8004BAa1…9b63`. `arc-testnet` still has no delegate and still answers 400 on these routes.
- Base Sepolia moves to its v4 delegate `0x9551263b…F787` (deployed by Execution Market on 2026-08-25). The table still pointed at the v3 `0x1AaEA468…5b45`, so Base Sepolia served the EIP-191 digest instead of typed data, and the response rail answered `relay_response_needs_v4` there. A rater still delegated to the v3 is reported as `delegated: false` and signs a fresh authorization.
- `/docs`: the relayed-feedback availability list names `arc`, and the `/feedback/evm/submit` example uses the Base Sepolia delegate that is actually served. `/erc8004` no longer says the rail is on `base-sepolia` alone, in English and Spanish. Tests now fail when either list, or the example, drifts from the delegate table.

## [2.37.1] - 2026-09-22

- Link previews are network-agnostic again. The landing's `og:description` goes back to its text from before #70 (83ac6d07), without the chain-family count: "Gasless x402 verify and settle. No fee, no account, no API key." Every page (`/`, `/x402`, `/dx402`, `/erc8004`, `/bazaar`, `/networks`, `/mcp`, `/integrar`, `/stats`, `/events/live`) goes back to `og:image` = `logo.png`, and loses the Arc/Hedera card's `og:image:width`, `og:image:height` and `og:image:alt` and the `twitter:card`/`twitter:image` tags that #70 added.
- The facilitator's own description no longer singles out Arc or Hedera: the A2A agent card (`/.well-known/agent-card.json` and `/.well-known/agent.json`), the opening of `/index.md` (and so of `/llms-full.txt`), `/mcp.md` and the "What it is" paragraph of `/mcp`, in English and Spanish.
- Network lists no longer single one out either: the family lists in `/index.md` and `/llms.txt` say `EVM` instead of `EVM (including Arc)`, and the family summary on `/networks` drops "including Arc and native Hedera" (English and Spanish).
- `/index.md` opens with its general sections (API, agent resources, links). The "Arc and native Hedera" section moves after them, next to the receipts section, with its content unchanged. `/llms-full.txt` is regenerated. Per-network instructions are unchanged everywhere.
- No page references `og-arc-hedera.png`/`.svg` any more. The files stay, and the `/og-arc-hedera.png` route still serves the PNG.

## [2.37.0] - 2026-09-22

- ERC-8004 identity and reputation on Arc mainnet (`arc`) and Arc testnet (`arc-testnet`), using the canonical registries: identity `0x8004A169…a432`, reputation `0x8004BAa1…9b63` and validation `0x8004Cc84…AB58` on mainnet; `0x8004A818…BD9e`, `0x8004B663…8713` and `0x8004Cb1B…4272` on testnet. All six were read with `eth_getCode` against the RPCs the facilitator deploys (`rpc.mainnet.arc.io`, `rpc.testnet.arc.io`): 130-byte proxies whose EIP-1967 implementation is the one Base and Base Sepolia run, `getVersion()` = `2.0.0`, `ownerOf(1)` answered on both. The ERC-8004 set grows from 21 to 23 networks (13 mainnets + 10 testnets). Arc payments are unchanged: still `exact` only, with no `upto`, escrow or relayed (EIP-7702) feedback, because no feedback delegate is deployed on Arc yet.
- `test_supported_networks_list` runs again. Its `#[test]` attribute had been duplicated onto the test above it, so it never ran and still asserted 20 networks while the list held 21.
- The OpenAPI ERC-8004 prose names every network in the set, and a test now fails when a network is missing from it or the stated count drifts.

## [2.36.5] - 2026-09-22

- The ERC-8004 Solana senders count a write against its network's daily limit before the transaction goes out, and give the place back only if the RPC refuses the transaction in preflight.
- `POST /register` on an EVM network refuses a recipient that is not an EVM address with 400, before anything is minted.
- The built-in daily ERC-8004 write limit for `arc` and `arc-testnet` is 100.
- Terraform: the Arc mainnet low-balance alert gets its own threshold in place of the default.
- A test pins the ERC-8004 write rate-limit period to exactly 12 seconds.

## [2.36.4] - 2026-09-22

- ERC-8004 writes that send a transaction (`/register`, `/feedback` and the `/feedback/*` submits) are limited per network per UTC day. Past the limit a write answers 429 with code `erc8004_daily_write_limit` and a `Retry-After` that runs to 00:00 UTC, without touching the chain. A write only keeps its place in the count if a transaction was broadcast. Limits: `ERC8004_DAILY_WRITE_CAP` for every network and `ERC8004_DAILY_WRITE_CAP_<NETWORK>` for one; `ENABLE_ERC8004_WRITES` still turns every write off.
- A test pins the period of the ERC-8004 write rate limit.

## [2.36.3] - 2026-09-22

- The ERC-8004 write routes (`/register`, `/feedback` and `/feedback/*`) draw on a per-IP budget of their own: 1 token every 12s, burst 30. `/discovery/register` and the bazar admin routes keep theirs (1 token every 12s, burst 250).
- The per-IP rate limiter accepts an IPv6 address in square brackets without a port, the form the load balancer appends.
- The MCP server copies every `X-Forwarded-For` line onto the request it forwards, not only the first.
- Terraform declares the load balancer's `xff_header_processing_mode = "append"`, the value it already runs with.

## [2.36.2] - 2026-09-22

- Per-IP rate limits key on the client address the load balancer appends to `X-Forwarded-For` (the header's last entry), and on the TCP peer when the header is absent; `X-Real-IP` and `Forwarded` are no longer read. The server now carries `ConnectInfo`, so a direct connection without the header is keyed on its peer instead of answering 500 `rate_limit_key_unavailable`. A test fails if any governor in `src/` keys on anything else.
- Docs: EURC on Arc mainnet is no longer described as pending. A funded 0.01 EURC x402 v2 payment was verified and settled on 2026-09-22 (tx `0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128`); Arc testnet funded acceptance remains pending.

## [2.36.1] - 2026-09-17

- Return HTTP 200 for authorized receipt lookups even when the original payment returned an HTTP error; preserve its signed status and the original POST response.
- Raise the Hedera testnet daily reservation ceiling to 12 HBAR for the expanded acceptance matrix; retain the mainnet ceiling of 10 HBAR.
- Document the required execution-role secret grant before deploying a new receipt signing key.

## [2.36.0] - 2026-09-17

- Add portable signed facilitator receipts for Arc exact USDC/EURC and Hedera USDC, including both mainnet and testnet.
- Bind receipts to purchase and authorization; preserve payment state independently of the merchant HTTP result.
- Persist and resume the original authorization, expose private receipt lookup, and verify Ed25519 provenance with trusted issuer keys.
- Document recovery limits, merchant propagation and the pending live EURC acceptance.
- Shuffle mainnet/testnet blockchain cards once per page load; filters and language changes retain that visit's order.
