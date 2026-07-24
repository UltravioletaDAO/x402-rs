# Changelog

## [1.55.2] - 2026-07-24

### Fix — WS-E verification cache keyed by manifest label (URL variants never matched)

`verification` still did not surface after 1.55.1: the cache was keyed by a URL
synthesized from the manifest (`https://mcp.execution.market/mcp`) while the
registry stores the real resource URL (`…/mcp/`, trailing slash), so the join
never hit. The cache is now keyed by the manifest entry **label**, which
`CurationInfo` already carries, making the annotation independent of URL
variants. Attested uptime is also now aggregated across every probed URL under
the product's prefix (`uptime_prefix`) rather than a single representative URL —
a curated product usually owns many resources (all MeshRelay channels, every
Tenjin article).

## [1.55.1] - 2026-07-24

### Fix — WS-E verification populates via ERC-8004 `ownerOf`

The `curation.verification` badge was never populating: the Reputation Registry's
`getSummary` reverts with `clientAddresses required` when called with an empty
client set (which it always was without a configured reviewer / any attestations).
Fixed by confirming the ERC-8004 identity via `IIdentityRegistry.ownerOf(agentId)`
(verified: Execution Market's agent 2106 exists on Base) and only calling
`getSummary` when a reviewer is configured. Verification now shows the on-chain
identity (`feedbackCount: 0`) even before any attestations are written.

## [1.55.0] - 2026-07-24

### Bazaar WS-E — ERC-8004 attested curation (on-chain verification, writes default-OFF)

The differentiator: probe-derived uptime turned into ON-CHAIN, independently
verifiable reputation. Nobody else ships this.

- **`src/discovery_attestation.rs`**: `attest_uptime` writes
  `giveFeedback(agentId, uptimeBps, 2, "uptime", endpoint, feedbackURI,
  feedbackHash)` to the ERC-8004 Reputation Registry (reusing the existing
  `/feedback` provider + EIP-1559/legacy gas path); `read_reputation` reads
  `getSummary` back. A daily task refreshes both.
- **ON-CHAIN writes are gated by `ENABLE_BAZAAR_ATTESTATIONS` (default OFF) — no
  gas is ever spent until you enable it.** The reputation READER runs regardless
  (RPC reads are free), so the new `curation.verification` field on
  `/discovery/resources` reflects any existing on-chain reputation
  (`{protocol, network, agentId, feedbackCount, uptime}`).
- Manifest gains an `erc8004: {network, agentId}` field; Execution Market is
  wired to its Base agent (id 2106, registry `0x8004A169…`).
- Uptime is computed from the health prober's cumulative probe counters.
- Evidence hosting: `GET /discovery/attestation/{hash}` serves the sha256-keyed
  evidence body committed on-chain (only `[0-9a-f]{64}` keys — F9); `nosniff`.
- Bazaar logo aspect-ratio fix (was squished into a square box).

Env (all optional; writes stay off unless the flag is set): `ENABLE_BAZAAR_ATTESTATIONS`,
`BAZAAR_ATTESTATION_REVIEWER`, `BAZAAR_ATTESTATION_INTERVAL`. Follow-ups: dedicated
attestation wallet + S3-persisted evidence before enabling writes.

Files: `src/discovery_attestation.rs` (new), `src/discovery.rs`, `src/types_v2.rs`,
`src/discovery_curation.rs`, `src/discovery_health.rs`, `src/handlers.rs`,
`src/facilitator_local.rs`, `src/main.rs`, `src/lib.rs`, `config/bazaar_curation.json`,
`static/bazaar.html`.

## [1.54.0] - 2026-07-24

### Bazaar WS-D — visual explorer at `/bazaar`

A branded, visual front-end for the curated bazaar, served at `GET /bazaar`
(standalone `static/bazaar.html`, compile-time embedded like the landing page).

- Metrics band (listed / verified-alive / first-party+VIP / sources).
- "First-class citizens" band pinning Execution Market, MeshRelay, 402Milly
  (first-party) and Tenjin (VIP).
- Paginated card grid over `/discovery/resources` with tier + health + network +
  price badges; per-item detail dialog (all accepts, health, source).
- Filters (tier, health, network, source), debounced search, deep-linkable URL
  state, EN/ES i18n, and the shared Ultravioleta dark design system. All item
  fields are HTML-escaped (no trust conferred from free-text provider/description).
- Nav link added to the landing page header.

Files: `static/bazaar.html` (new), `src/handlers.rs`, `static/index.html`.

## [1.53.0] - 2026-07-24

### Bazaar WS-C — curated tiers (first-class citizens)

Our products now rank first, and internal debug entries are delisted.

- **`config/bazaar_curation.json`** manifest (runtime-loaded, fail-open via
  `BAZAAR_CURATION_PATH`): `first_party` = Execution Market, MeshRelay, 402Milly;
  `vip` = Tenjin (user-confirmed). Matching is host-exact + path-boundary on the
  parsed URL (`match_manifest_prefix`), so `api.meshrelay.xyz.evil.com` can never
  impersonate a curated product (F1).
- **Tier-aware ordering** in `GET /discovery/resources`: `first_party` > `vip` >
  `verified` (health-alive) > `listed`, then by liveness, then `lastUpdated`.
  Each item is annotated with `curation { tier, label, firstParty }`. New
  `?tier=first_party|vip|verified|listed` filter.
- **Suppression**: the manifest `suppressed[]` list delists entries (the leftover
  `__bazaar_debug__` internal entry is now hidden).

Files: `src/discovery_curation.rs` (new), `config/bazaar_curation.json` (new),
`src/discovery.rs`, `src/types_v2.rs`, `src/handlers.rs`, `src/discovery_security.rs`,
`src/discovery_crawler.rs`, `src/main.rs`, `src/lib.rs`.

## [1.52.0] - 2026-07-24

### Bazaar WS-B — health prober (the pre-ping / dead-endpoint sweep)

The catalog is now not just payable but **alive**. A background prober checks
every listed URL and hides the dead ones from the default listing.

- **Prober** (`discovery_health`): every `DISCOVERY_HEALTH_TICK` (60s) probes the
  due URLs with the SSRF-hardened `safe_get` connector (no payment attached).
  `402` = alive; `401/403/405/415` = auth-gated (healthy for its design, e.g.
  Execution Market or POST-only endpoints); `200/201/429` = degraded; `404/410`
  / dead / 5xx / DNS-fail = fail; SSRF-refused / template / non-http =
  unprobeable.
- **Hysteresis state machine**: quarantine after 3 consecutive fails (with
  1h→6h→24h→72h backoff), recover after 2 consecutive alives. Liveness lives in
  a **separate S3 overlay** (`bazaar/health.json`), never inline on the resource,
  so the ingestion filter and retention GC can never clobber it.
- **Politeness / safety**: global concurrency (`DISCOVERY_HEALTH_CONCURRENCY`,
  15), average rate cap (`DISCOVERY_HEALTH_MAX_RPS`, 20), and max 3 probes per
  host per tick so a mega-host (e.g. thousands of listings on one origin) is
  spread across ticks instead of hammered. Kill-switch `DISCOVERY_ENABLE_HEALTH`.
- **Read side**: `GET /discovery/resources` now hides `quarantined` resources by
  default and annotates each item with its `health` (`status`, `lastChecked`,
  `httpStatus`, `latencyMs`). New `?health=alive|degraded|auth_gated|quarantined|
  unknown|unprobeable|any` filter (`any` restores the full view). The health
  snapshot is taken before the resources read guard to avoid a guard-across-await.

Over the first ~day after deploy the default view converges from ~21k to the
subset that actually answers 402, as the ~62% host-alive-but-404 listings hit
their third fail and quarantine.

Files: `src/discovery_health.rs` (new), `src/discovery.rs`, `src/types_v2.rs`,
`src/handlers.rs`, `src/main.rs`, `src/discovery_crawler.rs`, `src/lib.rs`.

## [1.51.0] - 2026-07-24

### Bazaar WS-A — ingestion filter + retention GC (curated catalog)

The core of the curated-bazaar plan: junk never enters the catalog, and the
historical ~5k junk items are cleaned out. Built on the F1/F2 primitives from
1.50.2.

- **`CurationFilter`** (`discovery_security::curation_check`, rules R1-R7):
  rejects non-http(s) schemes (`monopoly://` et al.), private/metadata/encoded
  IP hosts, bare no-dot hosts, userinfo/oversized/whitespace URLs, non-whitelisted
  types, oversized description/tags/metadata, and unpayable resources (no accepts
  entry with a non-zero amount; `facilitator` type exempt). Spec-legal
  `/:param` template URLs are kept, flagged unprobeable.
- **`ImportPolicy`** replaces the `skip_validation` bool in `bulk_import`:
  `Strict` (register path, unchanged) vs `Filtered` (aggregator + crawler now
  run the curation filter instead of importing everything).
- **Field-preserving merge**: on a colliding URL, incoming wins for content but
  `first_seen` keeps the earliest, `settlement_count` the max, and a
  self-registered/settlement record is never downgraded to aggregated (F4).
- **Future-timestamp reject** (F5): feed `last_updated > now+300s` is dropped,
  so a poisoned timestamp can't pin an item to the top or evade retention.
- **Retention GC** (`apply_retention`, runs after each aggregation cycle):
  removes already-stored resources that fail the static rules — deterministic on
  stored data, never on fetch success, so a transient upstream outage can't
  mass-delete. Disable with `DISCOVERY_ENABLE_RETENTION_GC=false`. Expected
  first-run removal ≈ 5,063 of 26,233 (verified against a full prod snapshot):
  scheme 2,817 + unpayable 2,193 + no-dot-host 43 + private-ip 10 → ~21,170 kept.
- **Snapshot persistence**: `bulk_import` and the GC now persist the full cache
  as one `save_all` (single S3 PUT), sequentially within the aggregation task,
  eliminating the per-item read-modify-write race that could re-add GC'd junk.
- **Aggregator pagination fix**: sources that return items with no pagination
  block are now paged until a short page (bounded by a 50k/source cap) instead
  of stopping after page one.

Files: `src/discovery_security.rs`, `src/discovery.rs`,
`src/discovery_aggregator.rs`, `src/discovery_crawler.rs`.

## [1.50.3] - 2026-07-23

### Bazaar — Phase-0 curation ops (source config, first-party REST, audit tool)

First curation quick-wins ahead of the full ingestion filter (WS-A):

- **Per-source config** `config/bazaar_sources.json` (runtime-loaded, fail-open
  via `FacilitatorConfig::all_with_source_config`): disables the three
  transport-broken aggregation feeds — **openx402** (TLS cert mismatch),
  **x402rs** (returns HTML, not JSON) and **virtuals** (404) — so they are no
  longer hit every aggregation cycle. Unknown ids / a missing or malformed file
  leave all sources enabled, so a config mistake can never empty the bazaar.
- **`scripts/bazaar_audit.py`**: reproducible discovery-quality audit (junk /
  unpayable / staleness / per-source quality + optional 402 health probe) — the
  tool behind the audit doc and the post-deploy verification gates.
- **`tests/fixtures/bazaar/payai-page.json`**: real captured junk sample for
  offline ingestion-filter tests.
- First-party REST endpoint `api.402milly.xyz/purchase` registered in the
  production bazaar (Execution Market `/tasks` pending its team's terms — it is
  auth-gated before the 402).

Files: `config/bazaar_sources.json` (new), `src/discovery_aggregator.rs`,
`scripts/bazaar_audit.py` (new), `tests/fixtures/bazaar/payai-page.json` (new).

## [1.50.2] - 2026-07-23

### Security — Bazaar discovery SSRF hardening + tier-matcher primitives (F1/F2)

Hardens the Bazaar discovery subsystem against the two CRITICAL findings from
the curated-bazaar security review (`docs/plans/bazaar/08-security-hardening.md`).
These are the reusable security primitives; the not-yet-built consumers (the
health prober / manifest tier resolver) will call them in their workstreams.

- **F1 (tier impersonation)**: `validate_resource` now rejects URLs carrying
  userinfo (`https://trusted@evil.com/` parsed with host `evil.com`). New
  `discovery_security::canonical_url` (single URL normalizer) and
  `match_manifest_prefix` (host-exact + path-boundary match on a *parsed* URL,
  never `starts_with` on a raw string) — defeats `api.host.xyz.evil.com`,
  `…@evil.com`, `…xyzevil.com`, and `/api-evil` prefix tricks.
- **F2 (SSRF to instance metadata)**: `is_disallowed_target_ip` extended for
  `240.0.0.0/4` and 6to4; `host_as_encoded_ipv4` rejects decimal/hex/octal IP
  encodings. New `check_url_target` (resolve DNS, reject if ANY resolved
  address is non-routable/private/metadata — a mixed answer is an attack) and
  `safe_get` (pin the socket to the checked address, disable auto-redirects,
  follow ≤3 redirects manually re-checking each hop, port allowlist
  {80,443,8080,8443}).
- **Wiring (F15)**: the discovery crawler now fetches via `safe_get`; the
  aggregator uses a redirect policy that refuses internal/userinfo/bad-port
  redirect targets.
- **Test (escrow)**: two regression tests pinning that the ERC-3009 signature
  travels verbatim as `collectorData` (EIP-7702-delegated / ERC-1271 wallets
  send a wrapped >65-byte signature that a length assert would silently break).

Files: `src/discovery_security.rs` (new), `src/discovery.rs`,
`src/discovery_aggregator.rs`, `src/discovery_crawler.rs`, `src/main.rs`,
`src/lib.rs`, `src/payment_operator/operator.rs`, `docs/plans/bazaar/*`.

## [1.50.1] - 2026-07-21

### Landing — stablecoin count 5 -> 6 (USDG)

- Hero badge "5 Stablecoins Supported" -> "6" (fallback + EN + ES i18n) and
  USDG icon added to both stablecoin icon rows (hero + x402r section) —
  v1.50.0 added the USDG token but missed these hero counters.
- New `docs/ROBINHOOD_CHAIN.md`: full integration reference (chain data, USDG
  gotchas, scheme matrix, upto-proxy bug history, verified bridge routes for
  funding, session log).

Files: `static/index.html`, `docs/ROBINHOOD_CHAIN.md`.

## [1.50.0] - 2026-07-20

### Feature — Robinhood Chain (mainnet 4663 + testnet 46630) settling Paxos USDG

Adds Robinhood Chain, the Arbitrum Orbit L2 by Robinhood (mainnet live since
2026-07-01), as payment network #21. The chain has NO Circle USDC (native or
bridged) — the settlement stablecoin is Paxos **USDG (Global Dollar)**, the
first non-USDC-only network in the facilitator:

- New `Network::Robinhood` (`robinhood`, `eip155:4663`) and
  `Network::RobinhoodTestnet` (`robinhood-testnet`, `eip155:46630`).
- New `TokenType::Usdg` + `USDGDeployment` registry: mainnet
  `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168`, testnet
  `0x7E955252E15c84f5768B83c41a71F9eba181802F` (both from Paxos official docs,
  6 decimals). EIP-3009 `transferWithAuthorization` verified on-chain
  (typehash + verified source + live dispatch probe).
- EIP-712 domain `{name: "Global Dollar", version: "1"}` cryptographically
  verified against on-chain `DOMAIN_SEPARATOR()` on BOTH chains. The static
  entry is mandatory: USDG's `version()` getter reverts (Paxos facet
  dispatcher), so the on-chain fallback cannot resolve it.
- `USDCDeployment::by_network` now returns `None` for Robinhood;
  `supported_networks_for_token(Usdc)` filters by real deployments instead of
  assuming `variants()`. The strict asset allow-list on Robinhood is USDG-only
  (the chain is full of impostor 18-decimal "USDC"/"PYUSD"/"USDT0" scam
  tokens, which remain rejected).
- Landing page cards (mainnet + testnet), balance lambda, OpenAPI docs,
  `config/supported_tokens.json`, terraform RPC env vars
  (`RPC_URL_ROBINHOOD`, `RPC_URL_ROBINHOOD_TESTNET`), stablecoin matrix.

### Security fix — upto scheme: wrong proxy address + vacuous-success guard

- **The hardcoded `UPTO_PERMIT2_PROXY_ADDRESS` was
  `0x4020633461b2895a48930Ff97eE8fCdE8E520002`, which has NO code on ANY
  chain** (miscopied at implementation time in v1.44.x). Corrected to the
  canonical upstream address `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002`
  (pinned by the x402 spec and @x402/evm; Sourcify-verified on Base;
  byte-identical bytecode confirmed on Base, Ethereum, Arbitrum, Optimism,
  BSC, HyperEVM, SKALE Base, Monad, World Chain, and Robinhood Chain). Our
  `settle()` ABI and `Witness{to,facilitator,validAfter}` struct match the
  verified source, so only the address needed fixing.
- **New `assert_proxy_deployed` guard in upto verify AND settle**: an
  `eth_call`/transaction against a code-less address succeeds vacuously, so
  before the fix an upto settlement would have returned `success=true` with a
  real tx hash while moving ZERO tokens (silent merchant fund loss). Now both
  paths hard-fail with a clear error on chains where the CREATE2 deployment
  has not been replayed (currently: Avalanche, Celo, Scroll, Unichain,
  Robinhood testnet).

Files: `src/network.rs`, `src/types.rs`, `src/chain/evm.rs`,
`src/chain/solana.rs`, `src/from_env.rs`, `src/handlers.rs`, `src/openapi.rs`,
`src/upto/types.rs`, `src/upto/permit2.rs`, `static/index.html`,
`static/robinhood.png`, `static/usdg.png`, `lambda/balances/handler.py`,
`config/supported_tokens.json`, `scripts/stablecoin_matrix.py`,
`terraform/environments/production/main.tf`, `.env.example`, `README.md`.

## [1.49.2] - 2026-07-10

### Reliability — nonce manager: no dashmap guard held across `.await`

Fixes a guard-across-`.await` hazard in `PendingNonceManager` found while
diagnosing a flaky CI test hang (the v1.49.1 `test` job hung for 33 min in the
async `test_reset_nonce_*` tests and was cancelled at the 35-min timeout).

- **Production `reset_nonce` (settlement failure path):** it held the dashmap
  shard read guard from `nonces.get(address)` across `nonce_lock.lock().await`.
  Holding a `dashmap` shard guard across an await point is a deadlock hazard — a
  suspended task keeps the shard locked and can block another task that needs the
  same shard. `reset_nonce` runs on `is_nonce_error` retries in `settle`, so this
  was on the hot path, not just in tests. It now clones the per-address
  `Arc<Mutex<u64>>` and drops the dashmap guard **before** awaiting, exactly as
  `get_next_nonce` already does (whose comment documents the same rule). No
  behavior change; strictly removes the hazard.
- **Tests:** the `test_reset_nonce_*` / `test_multiple_addresses_*` /
  `test_concurrent_reset_and_access` tests held a dashmap guard (`entry(...)` /
  `get(...)`) across `.lock().await`, which is what actually hung on CI. They now
  go through `set_nonce`/`read_nonce` helpers that clone the `Arc` and drop the
  guard before awaiting, removing the flaky hang at its source.

Files: `src/chain/evm.rs`. No API or protocol change.

## [1.49.1] - 2026-07-09

### Reliability — ERC-8004 `/register` stranded-record hygiene (FAC-1 #2 follow-ups)

Closes two low-severity record-keeping asymmetries in the v1.49.0 stranded-NFT
recovery, surfaced by the pre-deploy adversarial review. No change to the safety
model; purely tightens when a recovery record is created and cleared.

- **Symmetric record/recover key.** Recording a stranded self-mint and recovering
  it now share a single precondition, computed once as `recovery_key` in the
  register handler: it is `Some` only when a retry could actually reclaim the
  token (EVM recipient **and** non-empty, trimmed `agentURI`). Previously the
  record was written from `finalize_from_response` whenever the key carried a
  recipient, so an **empty-`agentURI`** (or non-EVM-recipient) registration could
  leave a record that the recovery path — gated on a non-empty URI — could never
  consume. Recording moved out of `finalize` into the handler's transfer-failure
  branch; `finalize` is back to job-status + in-flight-lock release only.
- **Clear on successful delivery.** Any successful transfer to the recipient now
  `clear_stranded`s the key. This removes the dangling record left when a
  transient recovery failure fell through to a fresh mint that then delivered
  successfully (after which the idempotency short-circuit would never revisit the
  key), so a record can no longer outlive the delivery it tracked. A repeated
  stranded mint for the same key overwrites the prior record (latest-wins).

Files: `src/handlers.rs`, `src/erc8004/register_jobs.rs` (new `record_stranded`;
removed the `finalize`-side recording and `key_has_recipient`). Still gated by
`ENABLE_REGISTER_RECOVERY` (default ON). Remaining documented residual: an NFT
orphaned in the facilitator's own wallet by a mid-flow process restart is
recoverable only out-of-band.

## [1.49.0] - 2026-07-09

### Reliability — ERC-8004 `/register` atomicity + a fund-safety fix (FACILITATOR security handoff FAC-1)

Addresses the KarmaCadabra 2026-07-09 facilitator handoff (`FAC-1` mint→transfer atomicity /
stranded-NFT recovery) and fixes a real pre-existing correctness bug surfaced while doing so. The
FAC-1 latency ask (async pollable `/register`) already shipped in 1.48.0.

- **Bug fix — reverted on-chain tx reported as success (mint + transfer):** `run_evm_registration`
  trusted both the mint and the transfer receipts without checking `receipt.status()`. A
  reverted-but-mined transaction still returns a receipt (`status == 0`), so (a) a `safeTransferFrom`
  that reverts (e.g. a recipient contract lacking `onERC721Received`) was reported as a successful
  transfer, and (b) a reverted mint — emitting no `Registered` event — fell through to a
  `totalSupply()` fallback that could resolve an **unrelated** `agentId` and transfer the wrong NFT
  to the recipient. Both are fixed: the transfer runs through a single shared `transfer_agent_nft()`
  helper requiring `status() == 1` (so the check can't drift between the normal and recovery paths),
  and the mint now rejects a reverted receipt outright instead of guessing an id.
- **FAC-1 #2 — stranded-NFT recovery (record-based, safe by construction):** if a registration
  mints the identity NFT but then fails to transfer it to the recipient, the NFT is stranded in the
  facilitator wallet and a naive retry re-mints (orphaning the stranded token and returning a
  different `agentId`). `/register` now records the stranded `agentId` keyed by the exact
  `network|agentUri|recipient` triple; a later retry for that same key reclaims that specific token
  (re-verifying on-chain `ownerOf == facilitator` **and** `tokenURI == agentURI` byte-exactly before
  transferring, status-checked) instead of minting anew, returning the **same** `agentId`. It is
  recipient-keyed and trusts only the facilitator's own recorded self-mints, so it cannot
  mis-deliver across recipients or be poisoned by a token an attacker transfers into the facilitator
  wallet — a deliberate rejection of the naive "scan facilitator-held tokens and match by URI"
  approach. Costs zero extra RPC on the happy path (view calls run only when a stranded record
  exists). Gated by `ENABLE_REGISTER_RECOVERY` (default ON). Cross-process/restart orphans remain a
  documented residual (recoverable only out-of-band).
- **FAC-2 — EIP-3009 timing (verified correct + regression-guarded):** re-verified `assert_time`
  against EIP-3009; the historical inverted-comparison bug is not present (validity is evaluated at
  `now + grace`, accepting `validAfter=now-60` and rejecting future-dated auths). Added 4 regression
  tests and corrected a stale doc comment (the code applies a forward settlement buffer on the
  expiry side — stricter than, not looser than, the spec).
- **FAC-3 — Base USDC "invalid signature" (verified resolved, no change):** `assert_domain` gives
  verified static EIP-712 metadata priority over client-provided `extra` for known tokens, so Base
  mainnet USDC resolves to `name: "USD Coin", version: "2"` (matching the on-chain FiatTokenV2_2) and
  a client cannot force a mismatched domain separator; the `transferWithAuthorization` call sites
  already log the full request parameters across all branches.

Files: `src/handlers.rs`, `src/erc8004/register_jobs.rs`, `src/chain/evm.rs`. Kill-switch:
`ENABLE_REGISTER_RECOVERY` (default ON).

## [1.48.0] - 2026-07-08

### Reliability — async pollable ERC-8004 `/register` (postmortem P1/P2/P3)

- **P1 — async pollable registration:** `POST /register` accepts `Prefer: respond-async` (or
  `X-Async: true`) → `202` + `jobId` in <2s; poll `GET /register/status/{jobId}`
  (`pending → mint_confirmed → done|failed`). Sync remains the default (SDK contract unchanged). New
  `src/erc8004/register_jobs.rs` in-memory job store; EVM core extracted to `run_evm_registration()`.
- **P2 — receipt-wait timeout:** register + transfer bound `get_receipt` with
  `TX_RECEIPT_TIMEOUT_SECS` (30s default; Base 90s, Ethereum 900s), like `/settle`.
- **P3 — in-flight lock:** a lock keyed by `network|agentUri|recipient` stops concurrent
  double-mints — async returns the existing job, sync returns `409 Conflict`.

## [1.47.0] - 2026-06-10

### Security — Multi-agent audit remediation (docs/security-audit-2026-06-10/)

Fixes the confirmed P0 + P1 findings from the 2026-06-10 security audit. Every non-EVM settle
path now binds recipient/amount/asset to the signed payload + payment requirements, mirroring the
EVM `assert_valid_payment` discipline.

- **P0 — Stellar self-drain (CRITICAL):** `src/chain/stellar.rs` Soroban auth-entry validation was
  inverted — it forced the transfer `from` to be the facilitator and accepted unsigned
  `SourceAccount` credentials, letting an unauthenticated `POST /settle` drain the facilitator's own
  Stellar USDC. Now rejects `SourceAccount` on the payment path, binds the credential address and
  transfer `from` to the declared payer, and hard-rejects facilitator-as-source. Legitimate
  payer-signed `Address`-credential payments (previously broken by the same inversion) now work.
  13 Stellar unit tests pass (incl. self-drain-rejected, SourceAccount-rejected).
- **P1 — Solana settlement-account forgery:** `verify_settlement_account` now requires the
  referenced tx to credit `pay_to` directly when there is no sweep (`settleSecretKey == None`), and
  the empty-settlement-ATA branch hard-errors instead of forging `success:true`. The legitimate
  Crossmint sweep flow is preserved. Adds `ENABLE_SETTLEMENT_ACCOUNT` kill-switch (default ON;
  set false to disable the niche path).
- **P1 — Sui coin-type confusion:** `check_balance` binds the spent coin object to the canonical
  USDC Move type, so a worthless `Coin<JUNK>` can no longer settle as USDC.
- **P1 — Algorand recipient/amount unbound:** `verify_payment_group` binds the signed ASA
  transfer's receiver/amount/asset/network/scheme to the payment requirements.
- **P1 — ERC-8004 gasless write abuse:** `/register`, `/feedback`, `/feedback/revoke`,
  `/feedback/response` are now rate-limited (~5 req/min/IP) and gated behind `ENABLE_ERC8004_WRITES`
  (default ON; set false to disable). Full proof-of-payment authorization (audit fix 02 Layer B) is
  tracked as follow-up (requires SDK/wire change).
- **P2 — RPC API-key leak:** escrow/commerce/upto/escrow-state transport errors are scrubbed via
  `redact::scrub_urls()` at the source, so API-keyed RPC URLs no longer reach client responses/logs.
- **Hardening:** char-safe truncation in the `post_verify` error path (prevents a UTF-8 boundary
  panic / DoS); `quinn-proto` bumped to 0.11.14 (RUSTSEC-2026-0037, CVSS 8.7).

### Deferred (see docs/security-audit-2026-06-10/MASTER_PLAN_EXECUTION.md)

ERC-8004 proof-of-payment gate (Layer B), compliance choke-point hoist for alt-scheme paths
(Phase 3A), broader non-EVM OFAC screening, remaining dependency bumps blocked by transitive pins,
and infra/Solidity hardening (Phase 4) — these require wire/SDK coordination, contract redeploys,
or `terraform apply` and are out of scope for this binary release.

## [1.45.2] - 2026-05-29

### Added - XRPL (XRP Ledger) Native Support

- XRPL (XRP Ledger) added as mainnet network #20, bringing coverage to 20 mainnets across 7 blockchain ecosystems (EVM, Solana, NEAR, Stellar, Algorand, Sui, XRPL).
  - Native XRPL family (NOT the EVM `xrpl-evm` / `eip155:1440000` sidechain). Network ids: `xrpl-mainnet` (CAIP-2 `xrpl:0`) and `xrpl-testnet` (CAIP-2 `xrpl:1`).
  - Native XRP asset, 6 decimals (drops; 1 XRP = 1,000,000 drops), no token contract. Issued tokens RLUSD and USDC also supported on mainnet as `{currency, issuer}` pairs.
  - Settlement model: clients submit pre-signed XRPL Payment transaction blobs; the facilitator submits them and pays the ~0.00001 XRP network fee.
  - Implemented in `src/chain/xrpl.rs`; `Xrpl` / `XrplTestnet` variants added to `src/network.rs` behind the `xrpl` feature flag.
  - Facilitator wallets: mainnet `rfADKkVXBNqK3z72tVSS3LVzAR3psYkonp`, testnet `rGhTioKAFHe75KgVnQtacRiKFuPv28Wbwk`.
  - Updated `config/supported_tokens.json` (added `xrpl-testnet`, summary now 37 total networks), `lambda/balances/handler.py` (XRPL balance fetch), README.md network tables, and `docs/CUSTOMIZATIONS.md`.

### Fixed
- Rate limiting: use `SmartIpKeyExtractor` for B8 governor (hotfix from v1.44.1).

## [1.37.0] - 2026-03-03

### Added - ERC-8004 Solana Full Support (Phase 1-3)

#### Phase 1: Read-Only Identity + Reputation
- **Solana Agent Registry integration** via QuantuLabs 8004-solana Anchor program:
  - `GET /identity/{network}/{agent_id}` - Read agent identity from on-chain PDA
  - `GET /reputation/{network}/{agent_id}` - Read reputation + ATOM Engine trust scores
  - `GET /identity/{network}/{agent_id}/metadata/{key}` - Read metadata entries
  - `GET /identity/{network}/total-supply` - Read total registered agents from RegistryConfig
- **ATOM Engine trust scoring** integrated into reputation responses:
  - Trust tiers (0-4: Unknown, Cautious, Neutral, Reliable, Trusted)
  - Quality scores, confidence, risk, diversity ratio
  - Feedback counts and last feedback slot
- New `AtomStatsResponse` type in reputation responses (`atomStats` field)
- `src/erc8004/solana.rs` module: Borsh deserialization, PDA derivation, RPC helpers

#### Phase 2: Feedback Submission
- **Solana branches** for all feedback endpoints:
  - `POST /feedback` - Submit feedback via Anchor `give_feedback` with ATOM Engine CPI
  - `POST /feedback/revoke` - Revoke feedback with SEAL v1 hash validation
  - `POST /feedback/response` - Append response with SEAL v1 hash integrity
- SEAL v1 hash computation for feedback/revoke/response operations
- Facilitator pays SOL gas as fee payer for all transactions
- `sealHash` field added to revoke/response requests (required for Solana)

#### Phase 3: Agent Registration
- **Solana branch** for `POST /register`:
  - Mints Metaplex Core NFT via Anchor `register()` instruction
  - Reads collection pubkey from on-chain RegistryConfig PDA
  - Generates new Keypair for NFT asset (dual-signer transaction)
  - Sets metadata PDAs if provided (`set_metadata_pda` instruction)
- `set_agent_uri` instruction builder for updating agent URI
- `set_metadata_pda` instruction builder with SHA256 key hashing

#### Cross-cutting Changes
- Agent ID parameter changed from `u64` to `serde_json::Value` across all ERC-8004 endpoints
  (accepts both JSON numbers and strings - backward compatible)
- `RegisterAgentResponse.agent_id` changed from `Option<u64>` to `Option<String>`
- `parse_agent_id_value()` helper for flexible agent ID parsing
- `keypair()` accessor on `SolanaProvider` for transaction signing
- Anchor instruction discriminators pre-computed via `SHA256("global:<fn_name>")[..8]`
- Metaplex Core program ID constant
- Program IDs: Agent Registry `8oo4dC4JvBLwy5tGgiH3WwK4B9PWxL9Z4XjA2jzkQMbQ`, ATOM Engine `AToMw53aiPQ8j7iHVb4fGt6nzUNxUhcPc3tbPBZuzVVb`
- ERC-8004 supported networks expanded from 16 to 18 (added Solana mainnet + devnet)
- OpenAPI documentation updated with Solana examples for all endpoints
- Landing page updated with Solana ERC-8004 badge and action button (EN + ES i18n)
- Technical documentation: `docs/ERC8004_SOLANA_INTEGRATION.md`, `docs/ERC8004_SOLANA_SDK_GUIDE.md`
- 16 new unit tests for Solana ERC-8004 module (32 total ERC-8004 tests passing)

## [1.36.0] - 2026-03-02

### Added - `/accepts` Endpoint (Faremeter Middleware Compatibility)

- **`POST /accepts`**: Negotiation endpoint that enables `@faremeter/middleware` integration
  - Receives merchant payment requirements, matches against facilitator capabilities
  - Enriches matched requirements with facilitator data (feePayer, tokens, escrow contracts)
  - Supports both v1 network names ("base") and v2 CAIP-2 format ("eip155:8453")
  - Merchant-provided `extra` fields are preserved (facilitator adds without overwriting)
  - Unsupported scheme+network combinations are silently filtered from the response
- Without this endpoint, servers using `@faremeter/middleware` received 404 errors
  and had to manually construct 402 responses (as discovered during Crossmint smart wallet testing)
- OpenAPI/Swagger documentation added for the new endpoint
- Landing page updated with endpoint entry (EN + ES translations)

### Added - Settlement Account Support (Crossmint Custodial Wallets)

- **Solana settlement account payloads** for custodial wallets that can only `sendTransaction`:
  - New `SettlementAccountPayload` type: `{ transactionSignature, settleSecretKey, settlementRentDestination }`
  - Added `SolanaSettlementAccount` variant to `ExactPaymentPayload` enum
  - **Verify**: fetches on-chain transaction via RPC, checks confirmation status, validates USDC
    transfer amount from pre/post token balances
  - **Settle**: verifies on-chain tx, then sweeps USDC from settlement account to payTo using
    the provided `settleSecretKey` (creates payTo ATA if needed, transfers, closes settlement ATA)
  - If `settleSecretKey` is not provided, returns original tx signature (funds already at payTo)
  - If settlement account has 0 balance, skips sweep (direct transfer mode)
- Enables Crossmint smart wallets and other custodial wallets that cannot `signTransaction`
- Automatic retry (up to 10 attempts with 2s backoff) for transaction fetching
- Compliance screening pass-through for settlement account payloads

## [1.35.0] - 2026-03-02

### Added - Solana Smart Wallet Support (Squads, Crossmint, SWIG)

- **Two-path verification for Solana transactions** enabling smart wallet payments:
  - **Path 1 (unchanged)**: Top-level TransferChecked detection for standard EOA wallets (~5ms)
  - **Path 2 (new)**: CPI inner instruction scanning for smart wallet transfers (~50ms)
  - Automatic fallback: tries Path 1 first, falls back to Path 2 if no top-level transfer found
- Smart wallets execute token transfers via Cross-Program Invocation (CPI), where TransferChecked
  appears as an inner instruction rather than a top-level one. This blocked all program-controlled
  accounts from using x402 payments on Solana.
- Now supports: Squads multisig, Crossmint custodial wallets, SWIG session wallets,
  SPL Governance DAOs, and any smart wallet that uses CPI-based token transfers
- Simulation now requests `inner_instructions: true` to capture CPI calls at all depths
- Inner instruction validation: verifies exactly ONE matching TransferChecked with correct
  amount, destination ATA, mint, and authority (prevents split/double transfer attacks)
- Added dependency: `solana-transaction-status-client-types` for inner instruction type parsing

### Hardened - Solana ComputeBudget Duplicate Rejection

- Reject duplicate `SetComputeUnitLimit` instructions (Solana applies last-wins, which could
  bypass facilitator caps)
- Reject duplicate `SetComputeUnitPrice` instructions (same last-wins bypass risk)
- References: [coinbase/x402#646](https://github.com/coinbase/x402/issues/646) RFC security model

### Context

- Requested by CryptoFede (Crossmint) for [lobster.cash](https://lobster.cash) integration
- Dexter and Faremeter already shipped closed-source implementations
- Ultravioleta DAO is the first open-source facilitator with smart wallet support
- Fully backward compatible: existing standard wallet payments work unchanged

## [1.29.0] - 2026-02-07

### Added - x402r Escrow Multi-Chain Support (9 Networks)

- **x402r escrow contracts configured for 9 networks** (from x402r-sdk A1igator/multichain-config):
  - Mainnets: Base, Ethereum, Polygon, Arbitrum, Celo, Monad, Avalanche
  - Testnets: Base Sepolia, Ethereum Sepolia
- Updated all Base contract addresses to match new SDK deployment
- `/supported` endpoint dynamically advertises escrow networks with deployed PaymentOperators
- Added `ESCROW_NETWORKS` constant as single source of truth for escrow support
- PaymentOperator deployment required on each network before settlement is active

### Fixed - ERC-8004 Network Name Consistency & Identity Lookup Robustness

- **BREAKING FIX**: `supported_network_names()` now derives names from `Network::Display` instead of hardcoded strings
  - Fixes "base-mainnet" vs "base" mismatch: `/feedback` returned "base-mainnet" but POST endpoints expected "base"
  - All API responses now use the canonical network names that serde/FromStr accept
- Removed `exists()` calls from identity lookup handlers (`/identity/:network/:agentId`, `/identity/:network/:agentId/metadata/:key`)
  - `exists()` is not part of standard ERC-721 and may not be implemented on all proxy contracts
  - Now uses `ownerOf()` revert detection for non-existent agents (returns proper 404)
  - Fixes "execution reverted" errors on Base and Ethereum identity lookups
- Added ERC-8004 section to README.md with 14-network table, API endpoints, and usage examples

## [1.28.1] - 2026-02-06

### Fixed - Avalanche ERC-8004 missing from /feedback API

- Fixed `supported_network_names()` not including "avalanche" and "avalanche-fuji"
- Updated all ERC-8004 tests to include Avalanche networks
- `/feedback` endpoint now correctly reports 14 ERC-8004 networks

## [1.28.0] - 2026-02-06

### Added - Avalanche C-Chain ERC-8004 Support (14 Networks)

- Added Avalanche C-Chain mainnet ERC-8004 contracts (CREATE2 deterministic addresses)
- Added Avalanche Fuji testnet ERC-8004 contracts
- Updated landing page ERC-8004 showcase: 8 mainnet badges, 14 total networks
- Updated all network counts (stats card, feature card, i18n EN/ES)
- On-chain bytecode verification confirmed for all 4 contracts

## [1.27.0] - 2026-02-05

### Improved - Landing Page ERC-8004 Showcase & Audit Fixes

- Added dedicated ERC-8004 showcase section with three-pillar design (Identity, Reputation, Validation)
- Added network badges with logos for all 7 ERC-8004 mainnets
- Added 4th stat card showing "12 ERC-8004 Networks" with purple gradient
- Added 4th feature card "On-Chain Reputation" with ERC-8004 highlight
- Updated SDK section from "14+ networks" to "19 mainnets supported"
- Full i18n support (EN/ES) for all new ERC-8004 content
- Fixed agent file parse errors (CRLF line endings in aegis-rust-architect.md, terraform-aws-architect.md)
- Removed invalid ralph-wiggum plugin references from global settings

## [1.26.0] - 2026-02-05

### Added - ERC-8004 Multi-Network Expansion (12 Networks)

This release expands ERC-8004 Trustless Agents support from 3 to 12 networks,
enabling cross-chain reputation across all major EVM ecosystems.

#### Supported Networks (12 total)

**Mainnets (7):**
| Network | Contract Addresses |
|---------|-------------------|
| Ethereum | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` / `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| Base | Same (CREATE2 deterministic) |
| Polygon | Same (CREATE2 deterministic) |
| Arbitrum | Same (CREATE2 deterministic) |
| Celo | Same (CREATE2 deterministic) |
| BSC | Same (CREATE2 deterministic) |
| Monad | Same (CREATE2 deterministic) |

**Testnets (5):**
| Network | Contract Addresses |
|---------|-------------------|
| Ethereum Sepolia | `0x8004A818BFB912233c491871b3d84c89A494BD9e` / `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| Base Sepolia | Same (deterministic) |
| Polygon Amoy | Same (deterministic) |
| Arbitrum Sepolia | Same (deterministic) |
| Celo Sepolia | Same (deterministic) |

#### Files Changed

| File | Change |
|------|--------|
| `src/erc8004/mod.rs` | Added 9 new network contracts, updated functions |
| `static/index.html` | Updated ERC-8004 section with 12 networks |

#### SDK Updates

- **Python SDK v0.8.0**: Added all 12 networks to `Erc8004Network` and `ERC8004_CONTRACTS`
- **TypeScript SDK v2.19.0**: Added all 12 networks with shared address constants

#### New Skill

Added `/add-erc8004-network` skill for automated ERC-8004 network integration.

---

## [1.25.0] - 2026-02-04

### Added - ERC-8004 Base Mainnet Support

This release enables ERC-8004 (Trustless Agents) reputation contracts on Base Mainnet.
The ERC-8004 contracts are now deployed on Base using CREATE2 deterministic addresses,
meaning the same addresses work across all supported chains.

#### Supported Networks for ERC-8004

| Network | IdentityRegistry | ReputationRegistry |
|---------|------------------|-------------------|
| Ethereum Mainnet | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| Ethereum Sepolia | `0x8004A818BFB912233c491871b3d84c89A494BD9e` | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| Base Mainnet | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |

#### Cross-Chain Reputation

With this update, AI agents can now:
- Make payments on Base Mainnet (via x402 protocol)
- Submit reputation feedback on Base Mainnet (via ERC-8004)
- Use the same agent identity across Ethereum and Base

The `ProofOfPayment` returned from `/settle` can be used to submit feedback on any
ERC-8004 supported network, enabling cross-chain reputation flows.

#### Files Changed

| File | Change |
|------|--------|
| `src/erc8004/mod.rs` | Added `BASE_MAINNET_CONTRACTS` with official addresses |
| `src/erc8004/mod.rs` | Updated `get_contracts()` to return Base contracts |
| `src/erc8004/mod.rs` | Added Base to `supported_networks()` and `supported_network_names()` |
| `static/index.html` | Updated ERC-8004 section with Base Mainnet support |
| `static/index.html` | Added BaseScan contract links |

#### Reference

- ERC-8004 Specification: https://eips.ethereum.org/EIPS/eip-8004
- Official Contracts: https://github.com/erc-8004/erc-8004-contracts
- BaseScan ReputationRegistry: https://basescan.org/address/0x8004BAa17C55a88189AE136b182e5fdA19dE9b63

---

## [1.24.0] - 2026-02-03

### Added - x402r PaymentOperator Escrow Scheme

This release adds the x402r escrow payment scheme, enabling advanced escrow flows
(authorize/charge/release/refund) via the PaymentOperator contract on Base Mainnet.
This is the first payment scheme beyond "exact" supported by the facilitator.

#### New Payment Scheme: `escrow`

- **`Scheme::Escrow`** enum variant added to payment schemes
- `/supported` endpoint now advertises escrow support on Base Mainnet (CAIP-2: `eip155:8453`)
- Gated by `ENABLE_PAYMENT_OPERATOR=true` environment variable
- Escrow contract info exposed in `/supported` response:
  ```json
  {
    "x402Version": 2,
    "scheme": "escrow",
    "network": "eip155:8453",
    "extra": {
      "escrow": {
        "escrowAddress": "0x320a3c35f131e5d2fb36af56345726b298936037",
        "operatorAddress": "0xa06958d93135bed7e43893897c0d9fa931ef051c",
        "tokenCollector": "0x32d6ac59bce8dfb3026f10bcadb8d00ab218f5b6"
      }
    }
  }
  ```

#### Base Mainnet Contract Addresses

| Contract | Address |
|----------|---------|
| PaymentOperator | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` |
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| PaymentOperatorFactory | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |

#### Security Fixes

- **Address validation**: Client-provided contract addresses (operatorAddress,
  tokenCollector, escrowAddress) are now validated against hardcoded known
  deployments before any on-chain transaction is submitted. This prevents gas
  drain attacks where an attacker could specify arbitrary target addresses.
- **`encode_collector_data` fix**: Changed from ABI-encoding `(bytes, bytes)` to
  raw signature bytes, matching what the `ERC3009PaymentCollector` contract expects.
  The old encoding would have caused on-chain reverts.

#### Files Changed

| File | Change |
|------|--------|
| `src/types.rs` | New `Scheme::Escrow`, `EscrowSupportedInfo` struct |
| `src/facilitator_local.rs` | Escrow scheme in `/supported` (gated by feature flag) |
| `src/payment_operator/operator.rs` | Address validation, raw signature encoding |
| `src/payment_operator/addresses.rs` | `PAYMENT_OPERATOR` address, `OperatorAddresses` update |
| `src/chain/*.rs` | `escrow: None` field on all chain providers |
| `static/index.html` | PaymentOperator section with contract links |
| `terraform/*/main.tf` | `ENABLE_PAYMENT_OPERATOR=true` |
| `.env.example` | Updated PaymentOperator docs |

#### Protocol Team Notes (Ali Abdoli, 2026-02-03)

- **$100 USDC deposit limit**: Enforced by PaymentOperator contract per deposit
- **`refundPostEscrow`**: NOT functional in production (requires `tokenCollector`
  contract not yet implemented by protocol team)
- **Recommended approach**: Use refund-in-escrow (keep funds locked until arbiter
  decides release or refund) instead of post-escrow refund
- **ERC-8004 reputation gating**: Future feature under consideration - could add
  condition contracts that check ERC-8004 scores before allowing authorize/charge

#### Related Changes (Other Repos)

This release was part of a coordinated update across 3 repositories:

1. **Chamba MCP Server** (`chamba` repo, commit `0ee2cf4`):
   - 8 new MCP tools for AI agents: `chamba_escrow_authorize`, `chamba_escrow_release`,
     `chamba_escrow_refund`, `chamba_escrow_charge`, `chamba_escrow_partial_release`,
     `chamba_escrow_dispute`, `chamba_escrow_status`, `chamba_escrow_recommend_strategy`
   - Agent guide: `mcp_server/docs/ESCROW_AGENT_GUIDE.md`
   - Integration layer: $100 limit, arbiter escrow pattern

2. **Python SDK** (`uvd-x402-sdk-python`, commit `835e9f6`):
   - `DEPOSIT_LIMIT_USDC = 100_000_000` constant
   - `refund_post_escrow()` marked NOT FUNCTIONAL

3. **TypeScript SDK** (`uvd-x402-sdk-typescript`, commit `10b6e89`):
   - `DEPOSIT_LIMIT_USDC = '100000000'` constant
   - `refundPostEscrow()` marked NOT FUNCTIONAL

---

## [1.19.1] - 2026-01-06

### Fixed - Aggregator ISO8601 Timestamp Parsing

Fixed a bug where the discovery aggregator failed to parse responses from Coinbase and other facilitators that return `lastUpdated` as an ISO8601 string instead of a Unix timestamp.

#### Changes

- **Flexible timestamp parsing**: `lastUpdated` field now accepts both:
  - Unix timestamp (u64): `1767737779`
  - ISO8601 string: `"2026-01-06T20:22:59.724Z"`

- **Added 11 new facilitator sources**:
  - PayAI: `https://facilitator.payai.network`
  - Thirdweb: `https://api.thirdweb.com/v1/payments/x402`
  - QuestFlow: `https://facilitator.questflow.ai`
  - AurraCloud: `https://x402-facilitator.aurracloud.com`
  - AnySpend: `https://mainnet.anyspend.com/x402`
  - OpenX402: `https://open.x402.host`
  - x402.rs: `https://facilitator.x402.rs`
  - Heurist: `https://facilitator.heurist.xyz`
  - Polymer: `https://api.polymer.zone/x402/v1`
  - Meridian: `https://api.mrdn.finance`
  - Virtuals: `https://acpx.virtuals.io`

- **`FacilitatorConfig::all()`**: New method returns all 12 known facilitators

---

## [1.19.0] - 2026-01-06

### Added - Meta-Bazaar Discovery Aggregation

This release implements Phase 1 of the unified Bazaar architecture, enabling the facilitator to aggregate discoverable resources from external facilitators (like Coinbase). This transforms the Ultravioleta facilitator into a "Meta-Bazaar" that indexes services from across the x402 ecosystem.

#### New Features

- **Discovery Source Tracking**: Resources now track their origin
  - `DiscoverySource` enum: `SelfRegistered`, `Settlement`, `Crawled`, `Aggregated`
  - `source_facilitator` field identifies origin facilitator (e.g., "coinbase")
  - `first_seen` timestamp for when resource was discovered
  - `settlement_count` for tracking payment activity

- **Discovery Aggregator**: Background task that fetches from external facilitators
  - Fetches from Coinbase CDP Bazaar (1,700+ services)
  - Converts v1 network names to CAIP-2 format
  - Runs periodically (default: every hour)
  - Configurable via `DISCOVERY_AGGREGATION_INTERVAL`

- **Enhanced Filtering**: Query resources by source
  - `GET /discovery/resources?source=aggregated` - Show only aggregated resources
  - `GET /discovery/resources?source_facilitator=coinbase` - Show Coinbase resources
  - Combines with existing filters (category, network, provider, tag)

- **Bulk Import API**: Efficient resource ingestion
  - `DiscoveryRegistry::bulk_import()` for batch upserts
  - Smart deduplication by URL
  - Only updates if newer `last_updated` timestamp

#### Environment Variables

```bash
# Enable/disable aggregation (default: true)
DISCOVERY_ENABLE_AGGREGATION=true

# Aggregation interval in seconds (default: 3600 = 1 hour)
DISCOVERY_AGGREGATION_INTERVAL=3600
```

#### Architecture

```
External Facilitators          Ultravioleta Facilitator
+------------------+          +-------------------------+
| Coinbase Bazaar  |--fetch-->| DiscoveryAggregator     |
| 1,700+ services  |          |   |                     |
+------------------+          |   v                     |
                              | Convert to v2 format    |
+------------------+          |   |                     |
| Other Facilitator|--fetch-->|   v                     |
+------------------+          | DiscoveryRegistry       |
                              | (source: Aggregated)    |
                              +-------------------------+
```

#### API Changes

- `DiscoveryResource` struct now includes:
  - `source: DiscoverySource` (default: `self_registered`)
  - `source_facilitator: Option<String>`
  - `first_seen: Option<u64>`
  - `settlement_count: Option<u32>`

- `DiscoveryFilters` struct now supports:
  - `source: Option<String>`
  - `source_facilitator: Option<String>`

### Added - Settlement Tracking (Phase 2)

This update implements Phase 2 of the unified Bazaar architecture: automatic settlement tracking. Resources can now be auto-registered in the Bazaar discovery registry when payments are settled.

#### How It Works

When a payment is settled via `POST /settle`:
1. Check if `discoverable=true` in `paymentRequirements.extra`
2. If true, auto-register the resource in the Bazaar (if new) or increment its settlement count (if existing)
3. Resources are tagged with `source: Settlement` to distinguish from self-registered or aggregated resources

#### Usage

Resource providers can opt-in to discovery by adding `discoverable: true` to their payment requirements:

```json
{
  "paymentRequirements": {
    "scheme": "exact",
    "network": "eip155:8453",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "amount": "100000",
    "payTo": "0x...",
    "resource": "https://api.example.com/premium-data",
    "description": "Premium market data API",
    "extra": {
      "discoverable": true
    }
  }
}
```

#### Benefits

- **Zero-effort discovery**: Resources are automatically indexed when payments succeed
- **Settlement metrics**: `settlement_count` tracks payment activity per resource
- **Trust signals**: Resources with high settlement counts demonstrate active usage
- **Backward compatible**: Existing integrations work unchanged (discoverable defaults to false)

#### Technical Details

- Settlement tracking runs asynchronously (non-blocking)
- Uses `DiscoveryRegistry::track_settlement()` for upsert logic
- Resources created via settlement are tagged with `source: Settlement`
- Settlement count is incremented for existing resources

### Added - Discovery Crawler (Phase 3)

This update implements Phase 3 of the unified Bazaar architecture: the well-known endpoint crawler. The crawler periodically fetches `/.well-known/x402` from configured seed URLs to discover x402-enabled resources.

#### How It Works

1. Configure seed URLs via `DISCOVERY_CRAWL_URLS` environment variable
2. Crawler fetches `/.well-known/x402` from each domain
3. Parses the JSON response containing resource definitions
4. Imports discovered resources with `source: Crawled`

#### Well-Known Format

Resource providers should serve `/.well-known/x402` with this format:

```json
{
  "x402Version": 2,
  "resources": [
    {
      "url": "https://api.example.com/premium",
      "type": "http",
      "description": "Premium API endpoint",
      "accepts": [
        {
          "scheme": "exact",
          "network": "eip155:8453",
          "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
          "amount": "100000",
          "payTo": "0x...",
          "maxTimeoutSeconds": 300
        }
      ],
      "metadata": {
        "category": "finance",
        "provider": "Example Corp",
        "tags": ["market-data", "real-time"]
      }
    }
  ]
}
```

#### Environment Variables

```bash
# Enable/disable crawler (default: false)
DISCOVERY_ENABLE_CRAWLER=true

# Crawl interval in seconds (default: 86400 = 24 hours)
DISCOVERY_CRAWL_INTERVAL=86400

# Comma-separated list of seed URLs to crawl
DISCOVERY_CRAWL_URLS=https://api.example.com,https://data.service.io
```

#### Technical Details

- Uses `reqwest` for HTTP requests with 30s timeout
- Crawl runs in background task (non-blocking)
- Resources tagged with `source: Crawled` and `source_facilitator: <domain>`
- 404 responses are silently ignored (domain doesn't support x402)
- Invalid URLs are skipped with warning log

#### Complete Bazaar Architecture

With all three phases complete, the Bazaar now has four resource sources:

| Source | Description | Trigger |
|--------|-------------|---------|
| `self_registered` | Direct POST to `/discovery/register` | Manual registration |
| `settlement` | Auto-registered on successful `/settle` | `discoverable: true` in payment requirements |
| `aggregated` | Fetched from external facilitators (Coinbase) | Background task (hourly) |
| `crawled` | Discovered from `/.well-known/x402` endpoints | Background task (daily) |

---

## [1.10.0] - 2025-12-19

### Added - Multi-Stablecoin Support

This release adds support for 6 stablecoins with EIP-3009 `transferWithAuthorization` capability across 14 EVM networks.

#### Supported Tokens

| Token | Networks | Decimals | Description |
|-------|----------|----------|-------------|
| **USDC** | All 14 networks | 6 | USD Coin by Circle (default) |
| **EURC** | Ethereum, Base, Avalanche | 6 | Euro Coin by Circle |
| **AUSD** | Ethereum, Polygon, Arbitrum, Avalanche | 6 | Agora USD (CREATE2 - same address all chains) |
| **PYUSD** | Ethereum | 6 | PayPal USD by Paxos |
| **GHO** | Ethereum, Arbitrum, Base | 18 | Aave stablecoin |
| **crvUSD** | Ethereum, Arbitrum | 18 | Curve Finance stablecoin |

#### New Features

- **TokenType Enum**: New enum in `src/types.rs` for token identification
  - Values: `usdc`, `eurc`, `ausd`, `pyusd`, `gho`, `crvusd`
  - Default: `usdc` for backward compatibility
  - Methods: `decimals()`, `symbol()`, `all()`

- **Token Deployment Registry**: Comprehensive token contract addresses
  - `get_token_deployment(network, token_type)` - Get deployment info
  - `is_token_supported(network, token_type)` - Check availability
  - `supported_tokens_for_network(network)` - List tokens per network
  - `supported_networks_for_token(token_type)` - List networks per token

- **Dynamic EIP-712 Validation**: Per-token domain separator calculation
  - Extracts token type from payment payload
  - Uses correct token name/version for typed data signing
  - Handles different decimal places (6 vs 18)

- **Enhanced `/supported` Endpoint**: Token information in response
  - New `tokens` field with token addresses and decimals per network
  - `SupportedTokenInfo` struct with token metadata

- **Frontend Token Badges**: Visual token support display
  - Token pills with per-token colors on network cards
  - JavaScript-based dynamic rendering
  - Shows which stablecoins each network supports

#### Contract Addresses

```
EURC:
  Ethereum: 0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c
  Base:     0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42
  Avalanche: 0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD

AUSD (CREATE2 - same on all chains):
  0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a

PYUSD:
  Ethereum: 0x6c3ea9036406852006290770BEdFcAbA0e23A0e8

GHO:
  Ethereum: 0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f
  Arbitrum: 0x7dfF72693f6A4149b17e7C6314655f6A9F7c8B33
  Base:     0x6Bb7a212910682DCFdbd5BCBb3e28FB4E8da10Ee

crvUSD:
  Ethereum: 0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E
  Arbitrum: 0x498Bf2B1e120FeD3ad3D42EA2165E9b73f99C1e5
```

#### Backward Compatibility

- **No breaking changes** - USDC remains the default token
- Existing clients work without modification
- `tokenType` is optional in payment payloads (defaults to `usdc`)
- Non-EVM chains (Solana, NEAR, Stellar) continue with USDC only

#### Test Coverage

- 39 new unit tests for multi-stablecoin functionality
- TokenType enum serialization/deserialization
- Token deployment lookups and validation
- Decimal handling (6 vs 18)
- Network/token mapping verification

---

## [1.8.0] - 2025-12-12

### Added - x402 Protocol v2 Support

This release adds full support for the x402 Protocol v2 specification, enabling CAIP-2 chain-agnostic network identifiers while maintaining complete backward compatibility with v1 clients.

#### New Features

- **CAIP-2 Network Identifiers**: Networks can now be specified using the [CAIP-2 standard](https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-2.md)
  - EVM chains: `eip155:{chainId}` (e.g., `eip155:8453` for Base)
  - Solana: `solana:{genesisHash}` (e.g., `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp`)
  - NEAR: `near:mainnet` / `near:testnet`
  - Stellar: `stellar:pubnet` / `stellar:testnet`

- **Dual Protocol Support on `/verify` and `/settle`**:
  - Auto-detects v1 vs v2 request format from request body
  - V1 requests use network strings: `"network": "base-mainnet"`
  - V2 requests use CAIP-2: `"network": "eip155:8453"`
  - Both formats processed identically after parsing

- **Enhanced `/supported` Endpoint**:
  - Returns both v1 and v2 entries for each network
  - V1 entry: `{ "x402Version": 1, "network": "base-mainnet", "scheme": "exact" }`
  - V2 entry: `{ "x402Version": 2, "network": "eip155:8453", "scheme": "exact" }`
  - Clients can filter by `x402Version` to find their preferred format

#### New Files

- `src/caip2.rs` - CAIP-2 parsing and validation
  - `Namespace` enum: `Eip155`, `Solana`, `Near`, `Stellar`, `Fogo`
  - `Caip2NetworkId` struct with parsing, display, and serde support
  - Validation rules per namespace (chain ID, genesis hash, network name)

- `src/types_v2.rs` - v2 protocol types
  - `ResourceInfo` - Separated resource metadata
  - `PaymentRequirementsV2` - Requirements with CAIP-2 network
  - `PaymentPayloadV2` - Payload with extensions support
  - `VerifyRequestEnvelope` / `SettleRequestEnvelope` - Dual v1/v2 request handling
  - Conversion traits between v1 and v2 types

#### Modified Files

- `src/network.rs` - Added `FromStr`, `to_caip2()`, `from_caip2()` methods
- `src/handlers.rs` - Updated verify/settle handlers for dual protocol support
- `src/facilitator_local.rs` - `/supported` returns both v1 and v2 entries
- `src/lib.rs` - Exported new modules

#### Backward Compatibility

- **No breaking changes** - All existing v1 clients continue to work unchanged
- V1 network strings (`base-mainnet`) still fully supported
- V1 response formats unchanged
- Existing integrations require no modifications

#### Example Requests

**V1 Request (unchanged):**
```json
{
  "x402Version": 1,
  "paymentPayload": {
    "network": "base-mainnet",
    ...
  }
}
```

**V2 Request (new):**
```json
{
  "x402Version": 2,
  "paymentPayload": {
    "network": "eip155:8453",
    ...
  }
}
```

---

## [1.7.9] - 2025-12-11

### Fixed
- Removed emojis from Rust log messages to prevent CloudWatch encoding issues

---

## [Unreleased] - 2025-10-28

### Updated - 2025-10-28 (Evening)
- **Network badges updated** to show all 4 supported networks:
  - Avalanche Fuji (testnet) + Avalanche C-Chain (mainnet)
  - Base Sepolia (testnet) + Base (mainnet)
- **Network descriptions updated** in both English and Spanish
  - English: "Supports Avalanche (Fuji testnet and C-Chain mainnet) and Base (Sepolia testnet and mainnet)."
  - Spanish: "Soporta Avalanche (testnet Fuji y mainnet C-Chain) y Base (testnet Sepolia y mainnet)."

### Added - Interactive Landing Page

#### New Features
- **Interactive landing page** at root endpoint (`/`)
  - Animated grid background with cyberpunk aesthetic
  - Bilingual support (English/Spanish) with instant switching
  - Gradient hero title with color-shifting animation
  - Prominent network badges for all supported networks (2 testnets + 2 mainnets)
  - Interactive stats cards (hover to scale)
  - Feature cards with glow effects on hover
  - Syntax-highlighted code example (JetBrains Mono font)
  - Animated endpoint list with slide effects
  - Scroll-based fade-in animations
  - Network-colored glows (Avalanche red, Base blue)

- **Logo support** at `/logo.png` endpoint
  - Embedded at compile time
  - Graceful fallback if logo not provided
  - Shows in header with pulse animation

#### Design System
- **Fonts**: Inter (UI) + JetBrains Mono (code)
- **Colors**:
  - Avalanche: `#e84142` (red)
  - Base: `#0052ff` (blue)
  - Accent: `#00d4ff` (cyan)
- **Animations**:
  - Moving grid background (20s loop)
  - Hero glow pulse (4s)
  - Gradient text shift (8s)
  - Fade-in on scroll
  - Hover transforms and glows

#### Files Modified
- `static/index.html` - Complete landing page (NEW)
- `static/logo.png` - Placeholder logo (NEW)
- `static/README.md` - Static assets documentation (NEW)
- `static/SETUP.md` - Setup guide (NEW)
- `src/handlers.rs` - Added `get_index()` and `get_logo()` handlers
- `src/main.rs` - Added routes for `/` and `/logo.png`
- `LANDING_PAGE.md` - Complete documentation (NEW)

#### Technical Details
- HTML/CSS/JS embedded at compile time via `include_str!()`
- Logo embedded via `include_bytes!()`
- Zero external dependencies (fonts via Google Fonts CDN only)
- Responsive design (mobile, tablet, desktop)
- Intersection Observer API for scroll animations
- Network badges with sweep animation on hover

### API Compatibility
- All existing API endpoints unchanged
- `/health` - Health check
- `/supported` - Payment schemes
- `/verify` - Payment verification
- `/settle` - On-chain settlement

### Networks Supported
- ✅ Avalanche Fuji (testnet)
- ✅ Avalanche C-Chain (mainnet)
- ✅ Base Sepolia (testnet)
- ✅ Base (mainnet)

### Browser Support
- Chrome/Edge 90+
- Firefox 88+
- Safari 14+
- Mobile browsers (iOS Safari, Chrome Mobile)

### Performance
- Initial page load: ~50ms
- Language switch: Instant (client-side)
- Zero API calls on page load
- Total page size: ~30KB (including fonts)

---

## Previous Releases

### [0.1.0] - Previous
- Initial x402 facilitator implementation
- EIP-3009 meta-transaction support
- Multi-network provider support
- Health and supported endpoints
- Verify and settle endpoints
