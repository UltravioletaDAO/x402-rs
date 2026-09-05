# Changelog

## [2.14.0] - 2026-09-04

### Changed

- **A settle that broadcast and never confirmed returns the transaction hash,
  not a uuid only we can resolve** (`1cf251e6`). That arm used to answer `400
  contract_call_failed (ref: <uuid>)` for a payment that may well be mined; with
  nothing to look up on chain, the only move left to the caller was the retry
  the arm exists to prevent -- and re-signing is a NEW, perfectly valid
  authorization for the same purchase, which the EIP-3009 nonce does not stop.
  It now answers `502` with
  `{"error":"settlement_unconfirmed","transaction","paymentId","retryable":false}`
  and deliberately no `Retry-After`. **The two 502s mean opposite things**:
  `upstream_rpc_unavailable` is retryable, `settlement_unconfirmed` is not, so
  an SDK that retries every 502 double-spends here. Built in five families (EVM,
  Solana, Stellar, XRPL, Algorand); NEAR and Sui cannot have it, because their
  single call yields a hash only on success.
- Stellar, XRPL and Algorand stop answering `success:false` with
  `transaction:null` on that path, which told the caller the payment had not
  happened when it may have.

## [2.13.0] - 2026-09-04

### Fixed

- **`invalidReason` went to the wire as `null`** (`6833f9ed`).
  `FacilitatorErrorReason` derived `Serialize` with `#[serde(untagged)]`, and an
  untagged unit variant is written with `serialize_unit` -- so all four fixed
  reasons serialised as `null` and every `#[serde(rename = "...")]` on them was
  present and inert. `VerifyResponse`'s own `Deserialize` refuses that body, so
  the facilitator was emitting a response its own parser -- and every client
  built on this crate -- could not read. Serialisation now delegates to
  `Display`, so the wire token and the log line cannot drift.
- **Four rejections stopped collapsing into `invalid_scheme`.**
  `ReceiverMismatch`, `InvalidSignature`, `InvalidTiming` and
  `InsufficientValue` shared one match arm. They need four different fixes on
  the client, so they now answer `receiver_mismatch`, `invalid_signature`,
  `invalid_timing` and `insufficient_value`, and `invalid_scheme` goes back to
  meaning an actual scheme mismatch. The extra tokens ride in `FreeForm` rather
  than as new enum variants, so no downstream exhaustive `match` breaks.

### Added

- **The settle response carries the payment's canonical id.** `paymentId` --
  `keccak256(caip2 || txHash)` -- was already load-bearing (DX402 stores
  evidence under it) but nothing ever handed it to the payer, so a caller who
  had just settled could not name the payment without reimplementing the keccak
  from the spec. Derived at serialisation time from `network` and `transaction`,
  so it cannot drift from the hash printed beside it, and absent when there is
  no transaction.

## [2.12.0] - 2026-09-04

### Changed

- **`durable-evidence` takes the shape every merged x402 extension has.** The
  declaration moves to the top level of the 402 as `{ info, schema }` with
  `acceptIndexes` naming the offers (`DurableEvidenceInfo`); the evidence object
  is placed under `extensions["durable-evidence"]` of the forwarded settlement
  response, the slot the core specification reserves, with the
  `X-Durable-Evidence` header kept as a convenience. `PaymentRequiredResponse`
  gains `extensions`. The v0.2 per-offer form under `extra.extensions` is still
  emitted and read as a fallback for one version. Verified against the
  registry convention on 2026-09-03; without this the first review comment is
  "please restructure". Nothing cryptographic, no gate rule and no receipt
  field changes — every anchor already verified stays valid.
- `X402Payments::select_from_challenge` / `build_payment_header_in` honour the
  top-level declaration; `select_payment_requirements` keeps its signature.
- Spec **v0.3** in registry format: `docs/plans/dx402/12-SPEC-v0.3-foundation.md`.

## [2.11.0] - 2026-09-04

### Added

- **The buyer chooses durable evidence from `accepts`.** A seller lists the same
  resource twice — plain, and with `durable-evidence` at its own price
  (`X402Middleware::with_durable_offer`) — and the client picks
  (`X402Payments::prefer_durable_evidence`, off by default, order-independent).
  The seller hook honours the paid offer (`OfferDecision`): the durable offer's
  `mode`/`retention` win; paying the plain one yields `{"skipped":"not_selected"}`
  rather than silence; a route without offers behaves exactly as before. No
  change to the x402 core: the declaration rides in
  `extra.extensions["durable-evidence"]`.
- `SkipReason::Unknown` (`#[serde(other)]`): a skip reason from a newer
  facilitator no longer drops the whole notice, pointer included.
- Spec v0.2 (`docs/plans/dx402/08-SPEC-v0.2.md`) describing only what is
  shipped, and `09-ESTADO-Y-CAMINO-A-UPSTREAM.md` tracking the path to the
  upstream proposal with evidence per row.

### Fixed

- **What was paid is decided by the buyer, not by listing order.** The layer
  matched a payload to `accepts` by scheme + network and took the first; with
  two same-network offers that anchored `not_selected` for a buyer who paid the
  higher price. It now matches on the signed `authorization.to` + `value` when
  more than one offer survives.
- **`settle_before_execution` routes now run the DX402 hook** (red team #9):
  they charged for evidence and emitted nothing, not even a skip. The hook runs
  after the handler, on 2xx/3xx responses only — an error response is not
  evidence — and the delivered bytes are untouched either way. The default
  branch is unchanged: it never settled, and so never anchored, on 4xx/5xx.
- **The escrow rail could be bypassed by sealing to the TokenStore** (red team,
  2026-09-04; present since v1.78.0). The escrow resolution only ran when the
  declared payer disagreed with the transfer's `from`, so any co-payee of a
  release could take a stranger's slot as `verified`. The rail is now classified
  from the receipt's logs (`classify_rail`) and resolved unconditionally;
  ambiguity is a property of the receipt, not of the branch taken.

## [2.10.0] - 2026-09-03

### Fixed

- **The buyer of an x402r escrow release is never the ERC-20 `from`.** A release
  moves tokens out of the operator's TokenStore, so the anchor gate's
  "evidence must be sealed to whoever paid" rejected every escrow-mediated
  payment — 23 of 23 live Execution Market releases sampled, 690 of 699 anchors
  in production. Anchors on that rail now carry `escrowRelease`; the facilitator
  asks the escrow to `getHash` the authorization and requires a
  `paymentInfoHash` that same transaction captured. Verified against optimism
  `0x5a2822cc…`. New verdicts: `dx402_escrow_release_missing`, `_invalid`,
  `_ambiguous` (batched settlements collide on `paymentId` and are refused).
- The seller hook in `x402-axum` sends `escrowRelease: None` — it sits where the
  buyer paid directly.

## [2.9.0] - 2026-09-02

### Added

- **The four x402 calls are served as MCP tools at `POST /mcp`** (`445f2a15`).
  `x402_supported`, `x402_accepts`, `x402_verify` and `x402_settle` -- exactly
  four -- over MCP Streamable HTTP (rmcp, stateless), plus the card at
  `/.well-known/mcp/server-card.json`. A tool call is dispatched THROUGH the
  REST router (`ServiceExt::oneshot`), never by calling the handler functions:
  `POST /settle` is wrapped in `settle_writer_gate`, which serialises the nonce
  of the single EOA that spends gas, and calling `post_settle` directly would
  let two ECS tasks sign at once. `/mcp` shares the `verify_settle_config`
  governor `Arc` -- the same `Arc`, not a second config with the same numbers --
  so an MCP settle and an HTTP settle from one IP draw on one bucket.

### Fixed

- **The nonce resync that existed to heal the EVM nonce was what broke it**
  (`b4170d76`).
- The p99 latency alarm measured two populations as one; the nonce alarm
  pointed at the wrong culprit (`e0128023`, `9c3870c6`).

## [2.7.0] - 2026-09-01

### Fixed

- **Removed a size cap that could not be right at any value, and was refusing
  bodies that used to work.** v2.3.0 added `MAX_SEALED_BLOB_BYTES = 48_000` on
  the decoded blob so an oversized anchor would get a DX402 error naming the
  limit rather than the body-limit middleware's bare `413`.

  It cannot work. `RequestBodyLimitLayer` is the outermost layer on the router,
  so a request over 64 KiB never reaches the handler at all. That leaves the cap
  two options and no third: below what the request limit allows, and it refuses
  bodies that previously anchored — 48,000 rejected everything from there up to
  48,810, which the Python SDK's own test had been asserting worked; or above
  it, and it is unreachable code the middleware always beats.

  The constant's own doc comment already said the middleware cuts first. The
  check was added anyway. `dx402_sealed_too_large` is gone with it, and a test
  now pins the range that must keep anchoring.

  A bare `413` for an oversized anchor is not improvable from inside the
  handler. Saying so is better than a cap that narrows what works.

## [2.6.0] - 2026-09-01

### Added

- **An alarm for the DX402 storage failure that is silent by design.** When
  Pinata refuses a write, `FallbackEvidenceStore` writes to S3 and the payment
  succeeds -- correct behaviour, since DX402 must never fail a payment, and
  exactly why nobody would notice. The anchor returns 201, the buyer gets their
  bytes, and the only trace is one `warn!` line.

  Three problems arrive through that one door: the Pinata JWT expiring (the
  current one ends **2026-12-19**, after which every anchor falls back
  permanently), quota exhaustion, and any Pinata outage.
  `facilitator-production-dx402-store-fallback` fires on one fallback in five
  minutes -- whatever caused it is still true for the next anchor -- and
  publishes to the facilitator's own SNS topic.

  The metric filter matches a substring rather than log positions: the line is
  emitted by `tracing` with structured fields whose order is not a contract, and
  a positional filter that quietly stops matching is how an alarm becomes
  decoration.

### Fixed

- **Documented what Pinata's dashboard counter does not count.** Measured
  against the live account: the dashboard reads `Files 3/500` and `Storage
  3.90 KB / 1 GB`, while DX402 had written **481 private files totalling
  1.40 MiB**. The counter reports public IPFS pins; DX402 writes to the v3
  *private* files API, and the two are separate quotas.

  This document made the mistake in the other direction first, reading the
  private count against the public 500-pin cap and raising a "19 files of
  margin" alarm that was never real. Both halves were true on their own; the
  comparison was not. No private-file quota is displayed anywhere and no
  endpoint reachable with the scoped JWT reports one, so that ceiling is
  **unknown rather than unlimited** -- said plainly instead of guessed.

## [2.3.0] - 2026-09-01

### Fixed

- **The signed receipt could name an object that never existed.** DX402
  predicted the evidence pointer with `pointer_for()` *before* uploading, signed
  an EIP-712 receipt over the prediction, recorded it, and then discarded the
  pointer `put()` actually returned. `FallbackEvidenceStore::pointer_for` spells
  the contract out -- *"if the write then falls back, `put` returns the
  fallback's pointer and the caller records that one"* -- and its only caller
  did not.

  Production runs the `ipfs` backend, which is Pinata with S3 behind it, so one
  Pinata hiccup -- a 10s timeout, an expired JWT, any 5xx -- left the bytes
  safely in S3 while the record and the signed receipt both named an IPFS object
  that never existed. Reading it fails silently by design: the fallback store
  treats the primary's `NotFound` as a verdict and never retries, and even if it
  did, the S3 pointer parser rejects an `ipfs+` pointer as foreign. The anchor
  returned 201, the receipt carried our signature, and the evidence was
  unreachable forever with nothing anywhere to say so.

  The v1.82.0 anti-hijack ordering is untouched -- claim the slot, and only then
  write bytes. The correction sits strictly below it, fenced by a claim token
  whose condition is *narrower* than the authority ladder: it matches only the
  row this call wrote, so a claim superseded mid-upload is refused rather than
  overwriting the winner. The token is a top-level DynamoDB attribute, because a
  condition expression cannot read inside the serialized `record` -- the same
  reason the ladder flags are hoisted. The correction is a full `PutItem`, not
  an update: the task role grants `PutItem`, `GetItem`, `DescribeTable` and
  `Scan` and nothing else, so a design built on `UpdateItem` would have deployed
  green and answered `AccessDenied` forever, silently.

  Re-signing happens only when the pointer changes: `pointer` is the third field
  of the EIP-712 struct, while `backend` is not in the type hash at all.

  This also closes a latent one that needs no Pinata failure: `cid_v1_raw` is
  valid only for content that fits one block, so a sealed body over 256 KiB
  produced a predicted CID that disagreed with the real one. We no longer trust
  the prediction.

- **`backend` was free text nobody checked.** A request could ask for `arweave`
  -- which has no implementation, is absent from `Cargo.lock`, and has never
  held a byte -- and the record plus every later read of `/dx402/evidence` would
  claim it. Not a signed lie, since `backend` is not in the type hash, but a
  persisted one, which is worse for anybody reading the index to find their
  evidence. Now refused with `dx402_backend_unavailable`, and the backend
  recorded is the one that *took the bytes*, not the one declared.

- **A chunked response had no ceiling at all.** `buffer_body` skipped only
  bodies that *announce* their size; a chunked one announces nothing, sailed
  past the guard, and `collect()` then bought however many bytes the handler
  chose to send. For a streaming handler -- exactly the large-body case --
  `max_body_bytes` was not a memory bound, and `EvidenceBudget`, which exists to
  prevent that OOM, was charging a number the body had no obligation to honour.
  Now read frame by frame and stopped at the limit, with everything already
  buffered handed back *ahead of* the untouched remainder.
  `http_body_util::Limited` looks made for this and is not: it reports the
  overflow as a stream error, and the error arm has nothing left to deliver --
  which would answer a paid request with an empty body, the one outcome this
  path exists to prevent.

### Added

- **`POST /dx402/repair/{paymentId}`** -- admin-gated audit of one anchor, with
  `?write=true` to correct a pointer that names nothing. Its own
  `DX402_ADMIN_TOKEN`, deliberately not shared with the bazaar or ERC-8004
  tokens: this one re-signs a facilitator attestation. **404 when no token is
  configured**, so the route is indistinguishable from absent.

  `write` defaults to false and reports `repairable`. Auditing is safe and
  rewriting a signed attestation is not, so the dangerous half has to be asked
  for by name -- otherwise the safe-looking call would be the dangerous one. And
  `lost` is never papered over: a record pointing at a real absence is telling
  the truth.

- **`scripts/dx402-audit-anchors.py`** -- scans the registry and classifies
  every anchor. Nobody currently knows how many of the existing ones carry a
  pointer that resolves to nothing; that number is the deliverable. A transport
  failure is its own verdict and is never folded into `lost`: "we could not
  check" must not be recorded as "the evidence is gone", which is precisely the
  mistake INC-2026-07-21 was, one subsystem over.

- **`dx402_sealed_too_large`** names the real ceiling (48,000 bytes) instead of
  leaving the body-limit middleware's bare `413`, which names no field and never
  mentions DX402. It covers the band a seller lands in when merely over the
  line; far above, the middleware still cuts first, because it is the outermost
  layer on the router.

## [2.0.1] - 2026-08-31

### Fixed - el timeout del reenvio abortaba antes que el holder

El arreglo de 2.0.0 presupuso una espera de recibo de 60s y le sumo 30s de margen.
Ese numero no aparece en ninguna parte del camino real: la espera se elige POR RED
(`evm_receipt_timeout` y su gemela en `chain::evm`) — Ethereum 900s, Base 90s, el
resto 30s — y `TX_RECEIPT_TIMEOUT_SECS` no esta definida en la task definition, asi
que rigen los defaults.

Efecto en las dos tasks que reenvian: un settle o un `/register` sincrono en Ethereum
abortaba el salto a los 90s con `forward_failed` mientras el holder seguia esperando
hasta 900s y la transaccion aterrizaba igual. En Base el margen prometido era
directamente negativo (90 contra 90, mas el tiempo de firma). Es el desenlace que el
propio commit de 2.0.0 define como peor que rechazar: un fallo reportado sobre un pago
que se ejecuta. Ethereum y Base llevan trafico real.

El salto ahora presupone la espera mas larga que puede tocarle (900s + 30s) porque no
sabe que red transporta — las rutas ERC-8004 nunca parsean una. Cuando
`TX_RECEIPT_TIMEOUT_SECS` esta puesta, reemplaza el default de todas las redes, asi que
el salto usa ese valor mas el margen y no el peor caso.

Dos tests nuevos, y el primero falla contra el codigo de 2.0.0: fija el timeout del salto
contra los MISMOS numeros por red que usa el camino de recibo, de modo que subir la espera
de Ethereum rompe el test en vez de reintroducir en silencio un salto que se rinde primero.

Encontrado en revision adversarial del propio 2.0.0, no por un reporte de usuario.


## [2.0.0] - 2026-08-31

### Fixed - P0: two out of every three EVM writes were being refused

Since 2026-08-29 14:28Z the facilitator refused most EVM writes, and the cause
was a correct guard meeting a changed assumption.

Exactly one process may sign EVM transactions, because the nonce for the shared
signer is allocated in memory (`PendingNonceManager`). A DynamoDB lease elects
that process, and non-holders answered `503`. That was right while "more than
one task" meant "for about a minute per rolling deploy".

On 2026-08-29 `min_capacity` went 1 -> 2 and the ALB request-count alarm took the
service to 3 in the same minute. From then on the ALB spread writes evenly over
three tasks of which exactly one could serve them: **two out of every three EVM
writes were rejected, permanently**. Measured over the six hours before the fix:
582 rejections on the settle path, 132 on the ERC-8004 write routes, and zero
lease handovers -- the lease never moved, the other two tasks simply never wrote.

Callers could not diagnose it from outside. They had a valid signature, a funded
signer, a passing `eth_call` simulation, and a 502; a retry had a one-in-three
chance, so it read as an intermittent facilitator fault. It surfaced as
"facilitator lease time-out", `SETTLEMENT_FAILED` before approve, `lock_failed`
on Arbitrum/Ethereum/Base, and `em_rate_agent` 503s.

**A non-holder now forwards the write to the holder instead of refusing it.**
The invariant is untouched -- one process still allocates every nonce -- but
every task serves 100% of the traffic the ALB hands it, so adding tasks adds
capacity instead of subtracting availability.

- The lease record carries the holder's routable address. A lost election
  returns it in the SAME response via
  `ReturnValuesOnConditionCheckFailure::AllOld`: no extra read, no second
  table, no service discovery.
- Forwarding is capped at ONE hop (`x-facilitator-forwarded-for-writer`). A task
  that receives a forwarded request while not holding the lease answers rather
  than forwarding again, so a stale address cannot bounce a settle between tasks.
- `/settle` uses a separate gate that forwards **only EVM** payments. Solana,
  Stellar, NEAR, Algorand, Sui and XRPL touch neither the EVM signer nor its
  nonce; forwarding them would funnel six chain families through one task and
  trade a correctness bug for a capacity one. A body that cannot be parsed is
  treated as EVM, because the holder can serve every family while a non-holder
  cannot serve EVM.
- The forward timeout clears `TX_RECEIPT_TIMEOUT_SECS` by 30s. Cutting the hop
  while the holder is still mining would report failure for a payment that then
  lands -- the one outcome worse than refusing.
- Every failure path (address unknown, holder unreachable, body too large,
  forwarding disabled) falls back to the previous `503` + `Retry-After`, so the
  change can never be worse than what it replaces.
- `GET /settle` and `/verify` are reads and stay unlayered on every task.

### Infrastructure

`aws_security_group.ecs_tasks` gains self-ingress **and self-egress** on 8080,
scoped with `self = true` so it opens nothing to the wider VPC. Both halves are
required: egress on this SG is deliberately not `0.0.0.0/0`, so without the
egress rule the forwarding connection is dropped on the way out and the caller
sees the same 503 the forwarding exists to remove.

### Configuration

| Variable | Default | Notes |
|---|---|---|
| `ENABLE_WRITER_FORWARD` | `true` | `false` keeps the lease but restores refusal |
| `WRITER_LEASE_ENDPOINT` | *(unset)* | Pin this task's advertised address by hand; otherwise read from ECS task metadata |


## [2.1.0] - 2026-08-31

### Fixed

- **SECURITY: the DX402 authority ladder had two rungs against the table, not
  three.** `POST /dx402/anchor` ranks claims -- 2 = the chain confirms the
  payee, 1 = the claimant committed to an identity, 0 = anonymous -- and each
  rung may only take a slot from a lower one. That is the v1.82.0 anti-hijack
  rule. Rung 1 was not enforced: DynamoDB hoisted `payment_id`, `record`,
  `expires_at` and `verified` but never `signed`, while the rung-1 condition
  asks `attribute_not_exists(signed)`. Against an attribute nobody writes that
  is unconditionally true, so the clause was a tautology and any self-signed
  claim could take the evidence slot from any other.

  It costs nothing to mount: `paymentId` is `keccak256(caip2 || txHash)` over
  public chain data, and a rung-1 claim only requires signing over an address
  the claimant types into its own request. `put_object` then overwrites the
  real seller's ciphertext -- unconditional, versioning disabled. Worst
  affected are the sellers who can never reach rung 2: `proof_rpc_unavailable`,
  and the whole Solana path via `proof_unverifiable_chain`.

  The tests were green throughout because `the_ladder_only_climbs` exercises
  `MemoryEvidenceRegistry`, which enforces the rule in Rust and always got it
  right. Production is DynamoDB. The new tests evaluate the CONDITION the way
  DynamoDB would -- including the asymmetry that caused this, where a
  comparison against a missing attribute is false and existence is the only
  thing you can ask about it -- across the full 3x3 rung matrix, plus flagless
  legacy rows and empty slots. One more is structural and catches the next
  occurrence: every flag a condition names must be a flag the writer hoists.

- **The envelope reserved 64 bytes for a 115-byte header, and doubled.**
  `SealedEnvelope::to_bytes` under-reserved by 51 bytes on the smallest
  possible envelope, so every seal ever performed overflowed its reservation
  and `RawVec` doubled the entire ciphertext to absorb it. Invisible because it
  was correct, just needlessly large. Reserving the real header dropped a
  capture's measured peak from **5.0x the body to 4.0x** -- 32 MiB saved per
  capture at the ceiling -- which is what the four copies one can actually see
  said all along. Measured in debug and release, flat from 1 MiB to the 32 MiB
  ceiling.

## [2.3.0-unreleased-note] (shipped as part of 2.3.0 — the 32 MiB default is live)

### Changed
- **DX402 evidence body limit raised from 1 MiB to 32 MiB, and made configurable.**
  At 1 MiB, `durable-evidence` was durable storage for small responses: an 18 MB
  API response (the case that prompted this) got `{"skipped":"too_large"}` and no
  evidence at all. The seller now sets it with `DX402_MAX_BODY_BYTES`, default
  33554432.

  **Why 32 MiB.** The 18 MB incident is the only measured case DX402 ever
  refused, so anything under it fails the reason the limit was made configurable
  — a couple of MB was considered and rejected on exactly that ground. Above it,
  the deciding fact is that `DurableConfig::default()` ships in *other people's
  processes*: it is what a seller gets for not thinking about it, on a host whose
  memory we do not size. The smallest number that clears the known case with room
  beats the largest one our own infrastructure could absorb. Raising it is one
  variable; lowering it after integrators have built on a bigger promise is a
  regression, and that asymmetry settles the direction of error.

  It is **not** a storage ceiling and not our storage: in pointer mode — what the
  `x402-axum` hook does — the object lands in the seller's own sink. The
  facilitator's bucket only receives the inline `sealed` path, capped at ~47 KB
  by the 64 KiB request limit. `GET /dx402/blob` cannot serve a large object
  either: `key_from_pointer` rejects foreign pointers, so it only ever reads our
  own bucket.

  The limit was never a storage bound — S3 takes 5 GiB in a single `PUT`. It is a
  **memory** bound: sealing holds the plaintext, then the ciphertext, then the
  sink's copy, so one capture costs several times the body. Which is why raising
  the number on its own would have been a downgrade, not a feature:

- **`DX402_MAX_INFLIGHT_BYTES` (default 167772160) bounds the memory all
  concurrent captures may hold.** Nothing bounded concurrency before, because at
  1 MiB nothing needed to. With a raised body limit and no bound, a burst of
  large responses in parallel is an OOM — and an OOM drops responses that were
  already settled and paid for, which is precisely the outcome DX402 exists to
  prevent. The budget turns that burst into an ordered skip.

- **The amplification factor is measured now, not assumed.** The budget charges
  each capture a multiple of the body size, and that multiple started life as an
  estimate of 4 -- plaintext, ciphertext, the serialised envelope, the sink's
  copy. `crates/x402-axum/tests/memory_amplification.rs` runs a whole capture
  under a counting allocator and measured **5.0x**, so the budget was
  under-charging by a quarter and would have admitted bursts it could not pay
  for: the OOM it exists to prevent.

  Chasing the fifth body found a real defect one layer down.
  `SealedEnvelope::to_bytes` reserved 64 bytes for a header that is **115**
  (`src/dx402/envelope.rs`), so every seal ever performed overflowed its
  reservation by 51 bytes and `RawVec` doubled the entire ciphertext to absorb
  it. Invisible because it was correct -- just needlessly large. Reserving the
  real header dropped the measurement to a flat **4.0x**, which is what the four
  copies one can actually see said all along, and saves **32 MiB per capture**
  at the ceiling.

  So the factor settles at 5 (measured peak plus one body of slack) and the
  in-flight default at 160 MiB, keeping the invariant that exactly one
  worst-case capture fits. Measured in debug and release, from 1 MiB up to the
  32 MiB ceiling itself, and flat across all of it -- the earlier extrapolation
  from a 4 MiB sample was sound, but no longer has to be taken on faith. The
  test fails in both directions, so the number cannot quietly rot again.

  **It is not memory the process takes, only memory it refuses to exceed.**
  Reservations are sized per body, so a seller returning 4 KB of JSON never holds
  more than a few KB however high the ceiling sits. That is why the budget is the
  generous half of the pair and the body limit is the conservative one.

  It is denominated in bytes of real memory and charges each capture roughly four
  times the body. **That factor is an estimate, not a profile** — the handoff
  asked for the measurement and it is still missing; the budget is the knob that
  stays honest regardless, because it is expressed in the memory the process
  actually has. A body that announces its length reserves that length; a
  streaming body with no `size_hint` has to reserve the worst case. Reservations
  are **refused, never queued**: buffering happens before the buyer's response
  goes out, so waiting for memory would delay a delivery that has already been
  settled. Delivery wins; evidence gives way.

  A body already over the limit reserves **nothing** — it is skipped without
  being buffered, and charging it would evict captures that could have succeeded.

### Added
- **`SkipReason::Busy`** — wire value `busy`. A full budget is not a store
  failure and reporting it as `anchor_failed` would send the next investigation
  at the store. Nothing is broken; the deployment simply declined to buffer one
  more large body. Both SDKs already handle it: they parse `skipped` as an open
  string (`String(payload.skipped)` / `str(payload["skipped"])`), not a closed
  enum, so a new variant surfaces rather than failing the payload.
- **`EvidenceStats` on the hook** (`hook.stats()`): counts anchored captures and
  skips by reason. Skips were silent — a header nobody tallies — so there was no
  way to tell "no response was too large" from "every response was too large".
- **`DurableConfig::from_env()`** with a 16 KiB floor and a clamp of the body
  limit to what the budget can afford. `DX402_MAX_BODY_BYTES=0` cannot silently
  skip everything, an unparseable value falls back to the default and logs, and a
  body limit the budget cannot cover reports an honest `too_large` instead of
  `busy` forever.

Unchanged and still load-bearing: **an oversized or unbudgeted body is delivered
in full.** Settlement happens before the hook and the nonce is spent, so a
dropped body is paid-for goods that can never be re-fetched.

**Not** phase 1. Streaming — chunked encryption, incremental hashing, S3
multipart, and the `tee` of the body — is still open, along with the decision it
forces: the `contentHash` cannot ride in a header the streaming case has already
sent. See `docs/plans/dx402/04-STREAMING-EVIDENCE-HANDOFF.md`.

## [1.92.0] - 2026-08-21

### Added
- **ERC-8004 on Scroll** (chain 534352) -- 21st network, 12th mainnet. Scroll already
  settled x402 payments; the canonical registries were simply never wired up.
  Verified before wiring, not assumed:
  - `eth_getCode` on two independent RPCs (`rpc.scroll.io`, `scroll.drpc.org`), both
    reporting chainId `0x82750`.
  - The ERC-1967 implementation behind each proxy is the *same address* as on Base,
    Arbitrum and Avalanche (`0x7274e874...` identity, `0x16e0fa7f...` reputation,
    `0xdb31f5d9...` validation), so the ABI we already ship applies unchanged.
  - `name()` on the Identity Registry returns `AgentIdentity`.
  - `totalSupply()` reverts on Scroll -- and equally on Base, so this is the existing
    registry behaviour and not a Scroll-specific gap.

### Fixed
- **Mainnet ValidationRegistry was declared `None` on every chain.** The canonical
  mainnet ValidationRegistry `0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58` was deployed
  after the identity/reputation pair and never picked up here. Verified live on
  Ethereum, Base, Polygon, Arbitrum, Optimism, Celo, BSC, Monad, Avalanche and Scroll;
  SKALE Base has no code at that address and correctly stays `None`.
- **Landing page no longer frames ERC-8004 as an Avalanche-specific deployment.** The
  Avalanche card carried an "ERC-8004 Beta" badge and a button both pointing at
  `ava-labs/8004-boilerplate`, implying Avalanche ran its own registry. It never did:
  the facilitator has always used the canonical CREATE2 addresses. Those links are
  gone, replaced by the canonical deployment list, and the landing now publishes all
  six registry addresses (identity/reputation/validation x mainnet/testnet) with a
  note that they are identical on every EVM chain.

### Contract addresses (golden source: `erc-8004/erc-8004-contracts`)

| Registry | Mainnet | Testnet |
|----------|---------|---------|
| Identity | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004A818BFB912233c491871b3d84c89A494BD9e` |
| Reputation | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| Validation | `0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58` | `0x8004Cb1BF31DAf7788923b405b754f57acEB4272` |

### Operator note
- The mainnet EVM wallet holds **0.0051 ETH on Scroll**. Enough for registrations and
  feedback at Scroll gas prices, but it is the thinnest balance of any 8004 mainnet --
  top it up before pointing real agent traffic at it.

## [1.91.0] - 2026-08-20

### `paymentRequirements` is the v1 spelling of `accepts`, and nothing here knew it

Found by KarmaKadabra on the Python SDK: their buyer matches both keys in
production, ours matched one. A seller answering `{"paymentRequirements": [...]}`
therefore read as "no terms here" — the exact false negative these readers exist
to prevent, one key away.

Two places carried the gap:

- **`PaymentRequiredResponse.accepts`** now has a serde alias. Without it the
  challenge simply failed to deserialize, so `x402-reqwest` could not pay a v1
  seller at all.
- **The Bazaar payTo-hijack check** read only `accepts`, so a v1 seller was one
  more resource where the security check saw nothing and read as "nothing
  drifted".

Contributed upstream to the Python SDK as 0.61.0; TypeScript 2.66.0 brings the
third implementation level.

## [1.90.0] - 2026-08-20

### The first real anchor to Pinata wrote evidence that could not be read back

Two defects, both caught by the e2e on the very first production anchor, and
both mine from the version before.

**The blob route was doubled.** `DX402_STORE_PUBLIC_BASE` already ends in
`/dx402/blob`, exactly as the S3 store assumes, and the Pinata pointer appended
it again: `…/dx402/blob/dx402/blob/0x…`. That pointer 404s, and it is the pointer
the receipt is SIGNED over.

**A private pointer had no way to be resolved.** It names the PAYMENT, while
Pinata addresses by CONTENT, and the store has no registry to translate between
them — `get` answered "resolve the CID from the registry first", which nothing
did. Evidence was being written and could not be read.

The CID now rides in the pointer's fragment
(`ipfs+https://…/dx402/blob/0x…#bafkrei…`). The pointer becomes self-sufficient:
`get` mints a signed URL from it alone, with no lookup and no dependence on
Pinata's list filtering — which does not filter by `keyvalues` the way its docs
suggest, measured. It also means the content address of the evidence is part of
what the facilitator attested, since the pointer is inside the signed receipt.

A test pins that the reserved pointer and the written one agree: `pointer_for`
reserves the registry slot before the upload, so a disagreement there would sign
a receipt over a pointer that never resolves.

## [1.89.0] - 2026-08-20

### Pinata turned ON, because the thing that was blocking it now exists

`dx402_storage_backend` defaults to `ipfs`. It stayed on `s3` for a day and the
reason was not caution: **Pinata expires nothing on its own.** S3 deletes
objects with a bucket lifecycle rule; Pinata has no equivalent. Enabling it
without a sweeper would have meant evidence that never expires while every
receipt we SIGN says `retentionUntil`. A promise with no mechanism.

`spawn_retention_sweeper` is that mechanism. Hourly, and deliberately timid:

- An object whose `retentionUntil` cannot be read is **counted and left alone**.
  Deleting evidence because we failed to parse its deadline would be the worst
  possible failure of the one component whose job is honouring deadlines.
- The public network is **never** swept. Unpinning does not remove bytes from
  IPFS, so a sweep there would report a deletion that did not happen.
- A sweep that cannot run leaves everything in place and retries next tick.

Objects are found by `keyvalues.retentionUntil`, written at upload. That is not
a shortcut around storing a reference: the upload happens **after** the registry
write — the ordering v1.84.0 established so a caller that loses the anti-replay
cannot overwrite evidence it does not own — so Pinata's object id does not exist
yet when the record is written. Reading the deadline off the object needs no
second write and works on objects already uploaded.

`EvidenceStore::put` now returns `StoredObject { pointer, reference }` so a
backend that needs a handle to delete later can hand one back. S3 ignores it.

## [1.88.0] - 2026-08-20

### The x402 challenge lives in a header, and two of our readers only looked at the body

Reported by an external prober measuring our own catalog's walls. Verified
against production the same day: of 40 live Bazaar resources, **36 of 36 that
answer 402 carry the challenge in the `PAYMENT-REQUIRED` header, and none in the
body**. Both transports are legal x402; sellers picked the one we did not read.

Worse than an empty body: sellers like Tenjin use the 402 body for a **free
preview of the paid content**. So the body is valid JSON that simply has no
`accepts` — a parse that succeeds and finds nothing.

**`x402-reqwest` could not pay any of them.** The buyer middleware did
`res.json::<PaymentRequiredResponse>()` and nothing else, so every header-transport
seller was unpayable. It now reads the header first and falls back to the body,
and an unparseable header falls through rather than refusing a seller whose body
is fine.

**The Bazaar payTo-hijack check had never fired.** `pay_to_from_402` parsed the
body only, and its caller guarded with `if !live.is_empty()` — so on every
header-transport resource the security check saw nothing and read as "nothing
drifted". A check that did not run looked exactly like one that passed.

The fix separates those two states: `LiveTerms { pay_to, readable }`. `readable`
is set only when the value actually looks like an x402 challenge, so a free
preview no longer counts as having been read, and a resource whose terms we
cannot parse now says so in the logs instead of passing silently.

Test vectors are real base64 challenges captured from production, and the "body"
vector is Tenjin's actual article preview.

### Not affected

The DX402 seller post-hook never inspects a 402 at all — it runs in
`settle_after_execution`, after the payment has already settled, on a response
that is not an error. And liveness classification (`402 => alive`) is by status
code, so no listing was ever buried by this.

## [1.87.0] - 2026-08-19

### Pinata (IPFS) as a storage backend, with S3 as the automatic fallback

`DX402_STORE_BACKEND=ipfs` now works. Pinata sits in front of S3 rather than
replacing it: an outage costs latency, never the evidence. The fallback is
deliberately one-directional -- S3 is the more conservative store (private,
deletable, retention enforced by a bucket rule), so falling back can never turn
a revocable promise into an irrevocable one.

**The record says where the bytes actually landed**, not what was configured. A
record claiming `ipfs` for an object sitting in S3 sends every later read to the
wrong place, and perfectly intact evidence looks lost.

Three things about Pinata's API differ from its documentation, and each one
changed the code (all measured against the live API, not read):

- A `name` containing `/` is truncated at the slash, so the S3-style key layout
  does not survive. The reliable index is `keyvalues.paymentId`.
- Uploads deduplicate by content, and a duplicate returns the FIRST record
  *including its network* -- an upload requested as `public` came back
  `"network":"private"`. The store records what came back, not what it asked for.
- Private reads need a signed URL against the account's OWN gateway; the generic
  `gateway.pinata.cloud` answers 403.

The CID is computed locally (CIDv1/raw/sha2-256) so the registry slot is still
reserved before a byte is uploaded -- the ordering that v1.84.0 established after
a refused duplicate was found to destroy the evidence it lost to. Its test vector
comes from a real upload, because the first one written here was invented and did
not match.

### `/dx402/stats` advertises what this deployment can actually offer

New `backends[]`, derived from configuration rather than declared:

```json
{"id":"ipfs-public","retention":"permanent","revocable":false,"public":true,
 "enabled":false,"disabledReason":"irreversible; awaiting buyer opt-in"}
```

`revocable` and `public` are not decoration: they are the difference between the
products. `revocable:false` means the `retentionUntil` the facilitator SIGNS
cannot be honoured -- unpinning removes our copy, not the network's. So
`ipfs-public` stays off by default even with a working credential: it is the
BUYER's ciphertext that becomes permanent, and the buyer cannot consent yet.

A backend without its credential is listed but disabled, with a reason. Hiding it
would read as "not a thing" rather than "not here".

### `/supported` no longer advertises an extension whose routes 404

It keyed off `ENABLE_DX402` alone, but the routes are registered only if the
service was built -- which it is not when the bucket is missing. So the flag
could be on, every `/dx402/*` route 404, and `/supported` still announce
`durable-evidence`. The comment on that code says it exists to prevent exactly
that. Both paths now read one predicate, `Dx402Config::is_serviceable`, so they
cannot drift.

Cosmetic until something keys off the signal -- which the landing page is about
to do.

## [1.86.0] - 2026-08-19

`POST /dx402/anchor` now accepts a CAIP-2 network id (`eip155:8453`) as well as
the v1 name (`base`). Every other route takes either — that is what the v2
format is for — so a client speaking CAIP-2 everywhere else got a bare `422`
here with no field named. Worse than an unknown network: the caller has no
reason to suspect the one field it spells exactly as it does on `/verify`.

## [1.85.0] - 2026-08-19

### The proof did not have to be a proof of THIS payment

`verify_anchor` read the payment facts off the chain and then certified a
`paymentId` that had nothing to do with them. A real payment proves a real
payment; it does not prove *which* payment the claim is about.

So the fix shipped one version earlier was defeated by anyone willing to spend
one wei: send yourself a token, obtain a perfectly valid `proofOfPayment` where
payer and payee are both you, and present it against a stranger's `paymentId`.
Every remaining check passes — the payer matches what you sealed to (yourself),
and your signature is over the payee the chain reports (yourself) — so the claim
reaches the FINAL rung and locks the real seller out permanently.

`paymentId` is a pure function of `(network, txHash)`, so binding it is one
comparison. It runs before any RPC call, because a definite rejection must never
be masked as `RpcUnavailable`, the verdict that never blocks. New verdict:
`dx402_payment_id_not_bound`, enforceable.

### Evidence work can no longer hold a paid response hostage

Both HTTP clients in the seller hook were `reqwest::Client::new()` — no timeout.
A facilitator or sink that accepts the connection and then stalls blocked the
buyer's already-settled response for as long as it stalled. Now 10s total, 3s
connect. A slow anchor costs the receipt, never the delivery.

## [1.84.0] - 2026-08-19

Two criticals from an adversarial audit of the v1.82.0/v1.83.0 anchor fixes.
Both were introduced by those fixes, and both were found by attacking them
rather than by re-reading them.

### Finality was self-asserted

`verified` — the flag that makes an anchor record unsupersedable — was decided by
checking `sellerSignature` against `req.payee`, **a field the HTTP caller
supplies**. So proving "I control the address I typed into my own request" was
enough to own a stranger's evidence permanently. `paymentId` is
`keccak256(caip2 || txHash)` over public data, so any observer of a settlement
can compute it, front-run the seller, and — worse than the v1.79 hijack v1.82.0
was written to fix — *supersede* the real seller's record.

The fix separates two questions that were collapsed into one flag, and ranks
them:

| rung | means | may supersede |
|---|---|---|
| 2 `verified` | the **chain** says this address is the payee | anything below |
| 1 `signed` | the claimant committed to an identity it controls | rung 0 only |
| 0 | anonymous claim | an empty slot only |

Equal rungs never overwrite each other — the anti-replay still holds, first
writer keeps the slot. `verified` now comes only from the gate, which checks the
signature against `facts.payee` read off-chain.

**This changes what `verified` reports.** An anchor with a valid
`sellerSignature` but no `proofOfPayment` now answers `verified: false`,
`signed: true`, `notVerifiedReason: "proof_missing"` — where before it answered
`verified: true`. Nothing breaks: the anchor still lands, still holds the slot,
still decrypts. It simply stops claiming an authorship nobody checked. Send
`proofOfPayment` to reach rung 2.

The DynamoDB condition was also wrong in a way the in-memory tests could not
see: it tested `verified = :f`, and every item written before the attribute
existed **lacks** it, where a comparison against a missing attribute is false.
The rule refused to supersede exactly the legacy records that most needed it.
Now guarded with `attribute_not_exists` on each flag.

### An oversized paid response was delivered empty

When the body exceeded `max_body_bytes`, the hook returned the original response
parts with `Body::empty()`. With `settle_after_execution` the payment has
**already settled** and the authorization nonce is spent, so the buyer was
charged and received a 200 with zero bytes, unrecoverably.

`buffer_body` now returns a `BufferedBody` that always carries something
deliverable. It also asks `size_hint()` first: a body that announces it is over
the limit is passed through untouched instead of being collected into memory to
be measured — which made a multi-gigabyte download an OOM of the 2 GB task
rather than a skip.

Skipping evidence must never change the bytes the buyer receives.

### Also

A refused duplicate could destroy the evidence it lost to: the `sealed` blob was
uploaded to the paymentId-keyed S3 object **before** the anti-replay decided,
and the bucket has versioning disabled. An anonymous caller could POST an
already-anchored paymentId with garbage, irreversibly overwrite the real
ciphertext, and receive a tidy 409 as if nothing had happened. The slot is now
reserved before any bytes are written.

## [1.83.0] - 2026-08-19

### The 409 that pointed at the wrong thing

An anchor whose `sellerSignature` did not verify was answered
`dx402_already_anchored` (409). True — a record did hold the payment — but not
the cause, and the cause it *implied* was credible: the reader goes to audit
retries, races and repeated heartbeats, finds candidates because those always
exist, and never suspects the shape of a digest they do not know has shapes.
KarmaKadabra isolated it with three anchors to one paymentId and reported it
2026-08-19.

The verdict already existed one layer up — `service.rs` computes `verified` and
logs `anchor is PROVISIONAL` — and was then discarded before answering. Now:

- `dx402_signature_not_verified` (**422**) when a signature was supplied and did
  not verify, instead of the misleading 409.
- `verified` is returned on the `201` from `POST /dx402/anchor` and on
  `GET /dx402/evidence/{paymentId}`. It was recorded but exposed nowhere, which
  is the quieter half of the same bug: with no prior record the anchor SUCCEEDS,
  so a seller signing the wrong digest form got a 201 that looked perfect while
  its anchor stayed provisional forever. It should not take a collision — which
  may never come — to find that out. A consumer treating a provisional record as
  proof of *who* produced the artifact is trusting a claim anyone could make.

A rejected signature still does not cost the anchor when nothing else holds the
slot. Refusing to write would make a seller-side signing bug cost the **buyer**
its copy, and DX402 degrades rather than withholds.

### The anchor size limit, measured and enforced early

`POST /dx402/anchor` accepts 64 KiB of request body (`MAX_REQUEST_BODY_BYTES`, an
anti-OOM bound on every route). Inline `sealed` blobs travel base64, so the
ceiling is ~47 KB of plaintext — KarmaKadabra measured 47 KB fits, 48 KB does
not. Both SDKs now check the **serialised** request, not the plaintext, and skip
with `too_large` before touching the network; measuring the plaintext lets
through bodies the facilitator rejects, and the failure lands long after sealing
is done. The Rust `x402-axum` hook is unaffected — it uploads via its sink and
sends a pointer.

Also in the SDKs: HTTP failures now carry the facilitator's own `error` and
`status` out of `anchor_evidence()` instead of flattening to `anchor_failed`,
which would have reproduced the 409 problem one layer down. The TypeScript
base64 conversion no longer spreads the blob into function arguments, where a
large body overflowed the call stack and surfaced as an unrelated failure.

## [1.82.0] - 2026-08-18

### Security — the anchor could be hijacked

Reported by KarmaKadabra and reproduced against production: **anyone could claim
the evidence slot of any payment**, and the legitimate seller was then locked out
permanently with a 409.

The two halves of the defence were in different phases. The `paymentId` claim was
**unconditional and permanent**; the proof that you are the seller was gated
behind `DX402_REQUIRE_PROOF`, which is off by default. So the part that protects
was the part that did not run — and the anti-replay added in v1.78.0 made it
worse, because before it the real seller could at least overwrite the garbage.

**Fix: a claim nobody proved is provisional.** An anchor carrying a valid payee
signature is `verified` and final. One without is provisional: it still blocks a
duplicate, but a verified anchor for the same payment supersedes it. The seller
can never be locked out by someone who cannot prove anything.

The signature check is deliberately **not** behind `DX402_REQUIRE_PROOF`. That
flag phases in the *on-chain* half, which needs an RPC and cannot run on every
family. A signature needs neither, so it is enforced from day one — and it has to
be, because the claim it guards is permanent.

### Added — ed25519 seller signatures

A Solana payee is an ed25519 address and cannot produce an EIP-712 signature at
all, so requiring one would have left Solana permanently unable to prove
authorship — exactly the hole the check exists to close. `verify_authorization_for`
now dispatches on the payee's own curve: secp256k1 recovery for EVM, raw ed25519
verification for Solana and Stellar, over the same canonical digest.

This is what makes the fix real on Solana **today**, without waiting for the
on-chain gate there: it needs no RPC, so it works while `unverifiable_chain` is
still non-blocking. The approach was KarmaKadabra's suggestion.

Addresses that cannot yield a verifying key (NEAR account ids, Sui hashes) report
"not proven" rather than being quietly accepted.


## [1.81.0] - 2026-08-18

### Fixed — Solana settlements silently dropped

Reported by KarmaKadabra: `/settle` on Solana returned
`contract_call_failed` with "confirmation timed out", naming a signature that
never appeared on chain. Measured in production over 24h: **10 timeouts against
2 successful Solana settlements** — intermittent, not dead, and the RPC (a
QuickNode premium endpoint) was healthy throughout.

Three changes, none of which guess at a root cause — they make the real one
visible and stop reporting a wrong outcome:

- **Preflight is now ON by default.** `skip_preflight: true` made the RPC
  validate nothing — not the blockhash, not the signatures — and return a
  signature regardless. A transaction that could never land looked exactly like
  one that would: a signature, thirty seconds of silence, then a timeout. With
  preflight the same failure comes back immediately saying what it was.
  `SOLANA_SKIP_PREFLIGHT=true` restores the old behaviour.
- **Confirmation window 30s → 90s.** A Solana blockhash stays valid for ~150
  slots (~60–90s), so giving up at 30 abandoned transactions that could still
  land — and told the caller the payment failed while it was in flight.
- **One final status read before declaring failure.** "TX may have been
  submitted" is the worst answer available: the money may have moved while the
  seller is told it did not. The timeout path now re-checks and distinguishes
  *did not settle, retry* from *status unreadable, check on chain first*.
- `max_retries` 5 → 20 for leader re-forwarding under congestion.


## [1.80.0] - 2026-08-18

### Fixed

A duplicate anchor answered `dx402_store_unavailable` with `"retryable": true`
instead of `dx402_already_anchored` (409). The anti-replay itself worked — the
second anchor was refused — but the verdict told the caller to retry something
that can never succeed, and this codebase's own rule is that a retryable failure
must not be persisted as a permanent answer. A seller would have retried forever.

Cause: the duplicate was detected by matching `"ConditionalCheckFailed"` against
the *Display text* of the AWS SDK error, which does not reliably contain the
exception name. Now matched on the typed error
(`is_conditional_check_failed_exception`). Found by the end-to-end check against
production, not by a unit test — the in-memory registry returns the right variant
either way, so only the real DynamoDB path exercised the string match.


## [1.79.0] - 2026-08-17

### Added — bidirectional evidence (envelope format v2)

Evidence sealed to the payer alone protects one side of a two-sided exchange.
The seller could not open the evidence for a payment it served, so it had no way
to answer a false "that is not what you sent" — while paying to anchor it.

The envelope now carries **several recipients**: `payer`, optionally `seller`,
optionally a designated `auditor`. The body is encrypted **once**; only the
content key is wrapped per recipient, so adding the seller costs about sixty
bytes rather than a second copy of the payload.

`seal_to()` takes the recipient list; `seal()` is unchanged and still means
"payer only". `open()` tries every slot, because a holder does not necessarily
know which one is theirs.

**A single-payer envelope is still emitted as v1, byte-for-byte.** Every reader
already deployed keeps working, and a v2 blob becomes a positive signal that
somebody besides the payer can open it. Both SDKs read v1 and v2; the roles are
readable from the blob without decrypting (`sealed_roles` / `sealedRoles`),
because a buyer has to be able to see who else holds a key — discovering that
afterwards would destroy the property this design sells.

Verified across three implementations in both directions, including a v2
envelope sealed by Rust and opened by Python and TypeScript **from both the
buyer and the seller slot**.


## [1.78.0] - 2026-08-17

### Security — the anchor gate

`POST /dx402/anchor` shipped in v1.77.0 checking nothing: anyone could park
bytes in the evidence store without paying, and — worse — obtain a receipt
**signed by this facilitator** for a payment that never happened.

The gate verifies, per anchor:

- the payment is real, succeeded, and is in the block it claims (reused wholesale
  from the ERC-8004 proof module, not reimplemented);
- **the payer is the address the evidence was sealed to** — without this, someone
  could seal evidence to their own key and hang it off a stranger's payment;
- **the anchor is signed by the payee** (EIP-712 over paymentId + contentHash +
  pointer). Comparing a declared payee instead would leave a race where an
  observer anchors garbage first and anti-replay locks out the real seller.

Plus **anti-replay**: one payment anchors once, enforced by a conditional
`PutItem`. A second anchor previously overwrote the first, and the receipt would
still verify because we had signed the replacement.

**Phase 1 by default.** `DX402_REQUIRE_PROOF=false` verifies and reports without
rejecting. Two verdicts never block in either phase: `rpc_unavailable` (no
verdict reached — our outage must not be recorded as someone's anchor being
fraudulent) and `unverifiable_chain` (non-EVM families have no receipt to read;
enforcing a check that never ran would silently disable DX402 there).

`DX402_ANCHOR_MAX_AGE_SECS` defaults to 900s, against ERC-8004's 7 days: an
anchor happens inside the same handler as the settle, and a window wider than the
operation only widens the attack surface.

### Changed

- `verify_proof_of_payment` split: the payment-level half is now
  `verify_payment_facts`, shared by both gates. A second copy of those checks
  would drift, and a drifted payment check does not fail loudly — it quietly
  accepts a payment that never happened.
- `ProviderMap` gains a blanket impl for `Arc<T>`, so DX402 reads receipts
  through the connections the facilitator already opened instead of a second set.
- `x402-axum`'s post-hook now forwards the settle proof and signs the anchor
  (`with_anchor_signer`). Best-effort: a missing key costs the receipt, never the
  response.

### Fixed

- Restored check ordering in the ERC-8004 gate. Extracting the shared half moved
  the rater checks after the RPC calls, so an unreachable RPC masked a definite
  "wrong rater" as `RpcUnavailable` — the one verdict that does *not* block a
  write. Two existing tests caught it.


## [1.77.0] - 2026-08-17

### Fixed — a seller with no storage could not produce evidence

`POST /dx402/anchor` recorded metadata but never wrote the ciphertext anywhere,
and the resource server has no credentials for the facilitator's private bucket.
The result was a dangling design: `GET /dx402/blob/{paymentId}` could serve
nothing, and any seller wanting evidence had to stand up their own **publicly
readable** object store — the exact failure mode this design set out to avoid.

`anchor` now accepts the sealed envelope inline as `sealed` (base64) and stores
it, issuing the pointer itself. `pointer` becomes optional: a seller that already
has durable storage keeps using it; one that does not sends the bytes and is
done. One HTTP call, no bucket, no credentials.

Storing ciphertext does not make the facilitator a custodian — in `direct` mode
it cannot read what it stores.

**Size ceiling:** the inline path is bounded by `MAX_REQUEST_BODY_BYTES`
(64 KiB default), and base64 inflates by a third, so roughly 48 KiB of
ciphertext. Larger bodies need the seller-hosted pointer path.

### Added

- Seller-side sealing in both SDKs (Python 0.48.0, TypeScript 2.55.0). It
  previously existed only in Rust, so any non-Rust resource server could read
  evidence but not produce it.
- `tests/dx402_cross_seal.rs`: Rust opens envelopes sealed by the Python **and**
  TypeScript SDKs, on both curves, from committed fixtures. The envelope format
  is now verified by three independent implementations in both directions.


## [1.76.0] - 2026-08-17

### Added — DX402 infrastructure and a pointer buyers can resolve without AWS

- `terraform/environments/production/dx402.tf`: private S3 bucket for sealed
  ciphertext, DynamoDB index, and the two task-role policies. Provisioning and
  switching on are **separate**: those resources cost ~$0 idle and are created
  regardless, while `var.enable_dx402` (default `false`) only controls the
  container's environment.
- `GET /dx402/blob/{paymentId}` serves the ciphertext from the private bucket.
  Pointers now address the **payment** rather than the S3 key layout, so a
  pointer a buyer holds a year from now keeps resolving through a re-layout.
- `scripts/dx402-bootstrap-secret.sh` creates the receipt-signing key. It signs
  attestations only — no funds, no gas — and refuses to overwrite an existing
  secret, which would silently invalidate the address every issued receipt
  verifies against.
- Runbook: `docs/plans/dx402/03-DEPLOY-RUNBOOK.md`.

### Security

- `key_from_pointer` rejects any pointer containing a path separator. The
  `paymentId` is interpolated into an S3 key, so `../../etc/passwd` and `a/b` are
  refused outright rather than sanitised. Tests cover traversal, a foreign host,
  the EC2 metadata endpoint, and a lookalike domain suffix.


## [1.75.0] - 2026-08-14

### Added — DX402 `durable-evidence` extension

x402 settles payment on-chain permanently but delivers the purchased resource
exactly once, in the body of a `200 OK`, and keeps nothing. A buyer who did not
capture it at that instant cannot recover it, and neither party can later prove
*what* was delivered — only *that* payment happened.

DX402 seals a copy of the response body to the payer's own public key, recovered
from the payment signature itself, and anchors it. Durable, private, and coupled
to the payment with no registration and no extra round trip.

- `src/dx402/` — facilitator as notary and index: EIP-712 `EvidenceReceipt`
  signing, DynamoDB/S3 backends, `POST /dx402/anchor`,
  `GET /dx402/{evidence,receipt}/{paymentId}`, `GET /dx402/stats`,
  `POST /dx402/recover` (honest 501 — `direct` mode needs no recovery endpoint).
- `crates/x402-axum/src/durable.rs` — the seller post-hook, wired into the
  `settle_after_execution` branch of `layer.rs`. `.with_durable_evidence(hook)`.
- `crates/x402-reqwest/src/durable.rs` — buyer-side fetch, decrypt and
  `contentHash` verification.
- Landing page section with a live anchor counter (EN + ES), OpenAPI entries,
  `docs/DX402.md`, spec and research under `docs/plans/dx402/`, and handoffs for
  KarmaCadabra, execution.market, MeshRelay and describe.net.

Key coverage: EVM and XRPL recover the key from the signature; Solana, NEAR,
Stellar and Algorand need only the address; Sui reads it from the signature
envelope.

**Off by default.** `ENABLE_DX402` is unset in production, so nothing on the
payment path changes. Missing configuration disables the feature and logs why
rather than falling back to a store that only looks durable.

### Security

- Reject small-order ed25519 public keys in ECDH (RFC 7748 §6.1, constant time).
  `ed25519-dalek` accepts non-canonical and small-order encodings in
  `VerifyingKey::from_bytes`; unchecked, an attacker able to influence the
  recorded payer key could collapse the shared secret to a constant and derive
  the content-key wrapping key. Tested against libsodium's 7-value blacklist.

### Changed

- `chain::evm::find_known_eip712_metadata` is now `pub`, so DX402 reuses the
  facilitator's own EIP-712 domain resolution instead of carrying a second copy
  that would drift and silently recover the wrong public key.


## [1.74.0] - 2026-08-14

### Security — reputation authorship and the proof-of-payment gate

The ERC-8004 Reputation Registry records `msg.sender` as the author of every
feedback, and the facilitator was the one signing: **87,2% of the feedback on
Base — 1384 entries across 28 agents — is attributed to our wallet instead of
the person who wrote it.** And `POST /feedback/revoke` authenticated nobody, so
an anonymous POST made us sign the destruction of exactly that reputation.
Measured against production before the fix: the unauthenticated call returned
500, meaning it had already reached the on-chain signing path.

- **`POST /feedback/revoke` is admin-only and fail-closed.** New credential
  `ERC8004_ADMIN_TOKEN`, deliberately not the bazaar's: one hides a catalog
  listing, the other erases third-party reputation irreversibly. With no token
  configured the route answers 404 and is indistinguishable from absent.
- **`src/erc8004/proof.rs` — the proof-of-payment gate.** Verifies on-chain that
  the transaction exists and succeeded, sits in the block claimed, carries an
  ERC-20 `Transfer` of exactly `amount` in `token` from `payer` to `payee`, that
  the payer is the new `rater` field, that the payee is an address the Identity
  Registry ties to the agent, that the block timestamp is inside the freshness
  window, that `paymentHash` recomputes, and that the (payment, agent) pair has
  not already been spent. Two-phase rollout: `ERC8004_REQUIRE_PROOF=false`
  verifies and reports without rejecting, so the blast radius is measured before
  it is enforced.
- **Real authorship on Solana** — `POST /feedback/solana/prepare` + `/submit`.
  The rater signs as `client`, the facilitator stays fee payer. `submit` refuses
  any transaction that is not byte-for-byte the one it built; otherwise the
  fee-payer keypair would be a public signing oracle.
- **Real authorship on EVM via EIP-7702** — `POST /feedback/evm/prepare` +
  `/submit`. The rater delegates their EOA to Execution Market's
  `FeedbackDelegate`; the transaction goes **to the rater's own address**, so the
  registry observes the rater while we pay the gas. Served only where a delegate
  is deployed and verified on-chain — today `base-sepolia`.
- `scripts/verify_feedback_anchor.py` and `scripts/spike_eip7702_stipend.sh`.

### Measured, not assumed

- `getAgentWallet` returns zero for almost every real agent on Base, so `ownerOf`
  is load-bearing and both are accepted — the wallet alone would have rejected
  nearly every genuine payment.
- `readFeedback` does not expose `feedbackHash` at all (29 selectors of the
  deployed implementation enumerated); it exists only in the `NewFeedback` event.
- Execution Market's payments carry **two** `Transfer` events, a fee and the net
  to the agent, so a proof must declare the net the payee actually receives.
- EIP-7702: the cold account-access charge for loading the delegate's code is
  billed to the caller, not taken out of the callee's 2300-gas stipend, so
  `transfer()` into a delegated wallet still works. Closes finding H2, which
  Execution Market had honestly marked NOT CONCLUSIVE rather than claim.
- A `.call()` to an address with no code returns **success**, so a delegate
  pinned to an undeployed registry would report ratings that never happened.
  Guarded.

### Fixed

- `dynamodb:DeleteItem` was missing from the task policy, so the anti-replay
  claim could never be released after a write that did not land.
- `alb_idle_timeout` had been silently reverted 600 → 180 on every deploy for
  months: `terraform.tfvars` is gitignored, CI applies with the defaults in
  `variables.tf`, and the targeted apply pulls the ALB in as a dependency.

## [1.64.0] - 2026-07-30

### Fix — USDC is 18 decimals on BSC, and two places assumed 6

`/api/stats` now returns `decimals` per row, resolved against the actual
deployment via the new `network::decimals_for_asset`.

Decimals are a property of a **deployment**, not of a token. USDC is 6 nearly
everywhere and **18 on BSC** — the deployment table had it right, with a comment
saying so. Two things did not:

- **`/stats` hardcoded 6.** On BSC it would have displayed a volume 10^12 too
  large — a metrics page wrong by twelve orders of magnitude while looking
  entirely plausible. Invisible today only because the only recorded volume is on
  Base, which is 6.
- **`TokenType::decimals()` asserted "All supported stablecoins use 6 decimals"**
  in its doc comment, contradicting the data three files away. It is only called
  from tests, so nothing shipped wrong — but a typed constant reads as
  authoritative, which makes a wrong one worse than none. The doc now says it is
  a default that is wrong on BSC and points at the deployment-aware resolver.

Serving `decimals` from the API is the actual fix: every consumer joining
against `/supported` and scaling by hand is one more place to make the same
mistake. `null` means the asset is unregistered, and the honest render is the
atomic value — never a guessed scale.

Found while answering a KarmaCadabra integration question, before they built a
dashboard on the same assumption.


## [1.63.0] - 2026-07-30

### Feature — publishing failed operations, behind a switch

`X402_EVENTS_PUBLISH_FAILURES` (default `false`). When on, operations that
*error* — RPC down, bad signature, contract revert — reach both the stream and
the store instead of vanishing.

Until now `ok:false` could only mean "resolved and came back negative", never
"blew up", so a 100% success rate meant *"no failures were recorded"*. That is a
much weaker claim than it looks, and it was the difference between a rail that
looks healthy and one that is.

Failures carry an `error` field holding a **bounded category**
(`contract_revert`, `invalid_signature`, `insufficient_funds`, …) and never the
error text. That is not tidiness: `ContractCall` wraps the transport error
verbatim, which on a bad day is an RPC URL with the API key inside it —
`src/redact.rs` exists because exactly that leaked once. Classification keys on
the Debug **variant name**, not the message, so a reworded error does not
silently degrade every category to `other`; an unrecognised variant becomes
`other` rather than echoing itself.

No payer is published on the error path. A bad signature recovers to a
meaningless address, and broadcasting it would name an innocent party.

`error` deliberately survives `DETAIL=minimal`: it holds no counterparty data,
and stripping it would leave minimal mode unable to answer the one question it
is still good for.

### Docs — the endpoints added in 1.60–1.62 were never documented

`README.md`, `static/index.html` and `CLAUDE.md` did not mention `/events`,
`/events/live`, `/stats`, `/api/stats` or `/transactions`. Only `openapi.rs` had
been updated, so the pages existed for anyone who already knew they existed.

All three now carry them, with the caveats attached rather than buried: neither
the stream nor the store is a ledger, the stream is lossy so absence proves
nothing, and while failure publishing is off a 100% success rate means "none
were recorded". The landing page links **Stats** and **En vivo** from the header
and footer.


## [1.62.0] - 2026-07-30

### Feature — `/events/live` and `/stats`, served by the facilitator

Two pages, baked into the binary like the landing page.

`GET /events/live` is the live traffic viewer. It exists as a route rather than
a file you open because Chrome and Brave treat `file://` as an opaque origin and
block its cross-origin requests no matter what CORS headers the server sends — a
viewer opened by double-click could never reach the stream. Served from the
facilitator it is same-origin, and the page derives its stream URL from
`location.origin` so it works unchanged on localhost, staging and production.

`GET /stats` is the metrics page, deliberately its own page rather than another
section of the landing page: that one is already a monolith, and someone reading
throughput is asking a different question than someone evaluating the service.

The page states what it measures **and what it does not**, because a dashboard
that omits its own limits is how a number gets believed: counting starts when
the store was enabled so earlier operations are unknown rather than zero;
errored operations are never recorded so a 100% success rate means "no failures
were recorded"; and the record is best-effort and written after the payment, so
the chain stays authoritative.

Volume is scaled with BigInt rather than `Number` — atomic amounts are
u256-shaped and lose precision above 2^53, which would understate exactly the
totals the page exists to show.


## [1.61.0] - 2026-07-30

### Feature — historical transaction store and `/api/stats`

`GET /events` is a live hint and lossy by construction: an event nobody was
connected for does not exist anywhere. This adds the other half — a durable,
queryable index, so "how much have we settled on Polygon this month" stops being
a question answered by grepping CloudWatch.

- `GET /transactions?limit&network` — recent operations, newest first, capped at
  200 because an unbounded limit from an unauthenticated caller turns a page
  load into a bill.
- `GET /api/stats` — totals per network and asset, read from pre-aggregated
  counters.

**This is an INDEX, not a ledger,** and both endpoints say so in their own
payloads. The write is fire-and-forget *after* settlement resolves and runs in a
spawned task, so it adds no latency and a DynamoDB outage loses records rather
than blocking payments. The chain remains the source of truth.

**Cost was measured, not assumed.** ~1,600 operations/day = ~48k writes/month
≈ $0.06. The real exposure is the read side: scanning the table on every stats
load is ~$0.011 a time, which is $330/month at a thousand views a day. So
aggregates live in their own single partition and the stats page issues one
bounded Query whose cost does not grow with history. The IAM policy grants no
`Scan` at all, which makes that structural rather than a convention.

Everything goes through a `TransactionStore` trait with a no-op implementation,
so an unconfigured deployment records nothing and settles exactly as before, and
moving off DynamoDB later is one new implementation touching no handler.

Counters use DynamoDB's atomic `ADD`, so concurrent settles cannot lose an
increment the way a read-modify-write would. Records carry a TTL
(`TRANSACTIONS_TTL_DAYS`, default 90, `0` disables); aggregate items deliberately
do not, so lifetime totals outlive the rows that produced them.


## [1.60.0] - 2026-07-30

### Feature — events now say WHAT was bought, not just how much

`resource`, `payTo`, `description` and `scheme` are added to every traffic event.
The facilitator already had all four at publish time — they were simply not
emitted.

The gap was visible the moment anyone watched the stream: two 1-USDC settles on
Base look identical, and the amount alone never says which endpoint was paid for
or which seller received it. That is the first question anyone asks of a payment
feed and it could not be answered.

**This is a deliberate step up in exposure**, taken with the tradeoff on the
table: the stream is public and unauthenticated, so it now shows that a given
wallet bought a given thing from a given seller, not merely that a payment
happened. Reversible without a deploy — `X402_EVENTS_DETAIL=minimal` drops the
new fields along with the old ones, and `redacted()` was extended so the privacy
dial keeps meaning what it says.

Serialisation is now explicitly camelCase (`payTo`). Every other field is a
single word so nothing else changes on the wire, but shipping `pay_to` beside
the camelCase of the rest of the protocol would have forced consumers to
special-case us. Pinned by a test that asserts the serialised JSON rather than
the Rust field names, and that absent fields are *omitted* rather than sent as
null.

Both SDKs carry the fields (PyPI 0.32.0, npm 2.45.0), and the events viewer
shows endpoint and seller as columns.

## [1.59.7] - 2026-07-29

### Fix — `/supported` advertised `upto` on five networks where it cannot settle

The proxy address is identical on every chain because it is deployed with
CREATE2. That was read as "therefore it works everywhere", so `/supported`
offered `upto` on every EVM network carrying `exact`. Deterministic is not the
same as deployed: the deployment has to be replayed per chain, and on five of
ours it never was.

Measured with `eth_getCode`, two independent RPCs per network:

- **Deployed, 3142 bytes:** base, optimism, arbitrum, polygon, bsc, ethereum,
  hyperevm, monad, base-sepolia, avalanche-fuji, arbitrum-sepolia.
- **No code:** avalanche, celo, scroll, unichain, optimism-sepolia.

Advertising an unsettleable scheme is worse than not offering it. The client
discovers the problem only at settle time — after it has signed a Permit2
authorization, and a signed authorization does not un-sign itself.

Networks are resolved through the `Network` enum rather than matched as CAIP-2
strings, so a renamed id fails to compile instead of silently dropping a chain
from the advertisement.

**Why a static list and not a startup probe.** A probe looks more honest and is
less reliable. Polygon returned NO CODE from `polygon-rpc.com` and from Ankr, and
the correct 3142 bytes from PublicNode — same address, three answers. A probe
would have dropped a working network depending on which node answered. The settle
path keeps its own `assert_proxy_deployed` guard, so a stale entry degrades to a
clear rejection rather than a transfer into an empty address. Polygon has its own
regression test saying exactly this, so the next person to see a "no code"
reading gets a second opinion before deleting the entry.

Also corrected in `src/openapi.rs`, which carried the same false claim in prose:
it now names the eleven networks and says plainly that `upto` is not available
wherever `exact` is.

Reported by Execution Market. Verified here independently — including that
Avalanche mainnet lacks the proxy while Fuji has it, so testnet and mainnet say
nothing about each other.

## [1.59.6] - 2026-07-29

### Fix — ERC-8004 writes bypassed the EVM writer lease

The writer lease exists because a rolling deploy leaves two ECS tasks running for
about a minute, each with its own in-memory nonce cache, and they race for the
same nonce on the shared EOA. `chain/evm.rs` has gated the settle path on it
since it shipped.

The ERC-8004 write handlers never passed through that gate. They reach the chain
through their own `contract.call().send()` sites — around ten of them across
`/register`, `/feedback`, `/feedback/revoke` and `/feedback/response` — spending
gas from the *same* signer. So during a deploy an ERC-8004 write on the old task
and a settle on the new one collide on a nonce: precisely the failure the lease
was built to prevent, entering through a door nobody had closed.

Reported by Execution Market, who found it while auditing this repo before
raising a request. They named one call site; there were ten.

The gate is now applied to `erc8004_write_routes()` itself rather than to the
call sites, so a write route added later is covered the moment it is registered
and there is no per-site guard to forget. A non-writer is shed with **503 +
`Retry-After`**, not 500 — during a deploy this state is expected and transient,
and the caller should retry rather than treat the request as malformed.

### Test — the concurrency criterion is finally executed

"20 concurrent settles, zero `nonce too low`" had been carried as the success
criterion of the nonce work and verified by nobody. The existing tests each
allocate a single nonce, which cannot observe the failure they guard against.

A duplicate nonce *is* `nonce too low` — two transactions signed with the same
number, the second rejected. So the property asserted is not "no error" but "no
repeats": 20 concurrent allocations must yield 20 distinct **and contiguous**
nonces, contiguity mattering because a gap strands every later settle behind it
until the chain-trust window expires. A second test runs the same race across
three signers to pin that per-address state stays independent.

Both drive the real `PendingNonceManager::get_next_nonce` against a provider
pointed at a closed port: with cached state the allocation must not touch the
network, so a refactor that reintroduces an RPC round-trip fails loudly instead
of silently taxing every settle.

Also added: the writer-lease gate's own behaviour tests, serialised against the
process-global flag so they pass under a plain `cargo test` and not only under
CI's `--test-threads=1`.

## [1.59.5] - 2026-07-28

### Feature — `GET /events`, a live traffic stream (Server-Sent Events)

One SSE message per `verify` / `settle`, so an observer can render facilitator traffic
without scraping CloudWatch. Built for KarmaCadabra's observatory but deliberately
generic: `src/events.rs` knows nothing about any client — filtering is an address
allowlist fed by env.

**The money path never blocks on the stream.** The bus is a lossy `tokio::sync::broadcast`
channel: `publish()` returns `()`, never propagates an error, and runs only after the
operation already resolved. A subscriber that falls behind loses events and stays
connected rather than applying back-pressure. Zero subscribers is the normal case, not an
error.

Config (all optional, safe defaults, an unparseable value falls back rather than failing):

| Variable | Default | Notes |
|---|---|---|
| `X402_EVENTS_ENABLED` | `true` | `false` → `/events` 404s and nothing is published |
| `X402_EVENTS_SCOPE` | `all` | `allowlist` = only payers in the list, and it fails **closed** |
| `X402_EVENTS_ALLOWLIST` | *(empty)* | comma-separated addresses, case-insensitive |
| `X402_EVENTS_DETAIL` | `full` | `minimal` = only `{ts, kind, network, ok}` |
| `X402_EVENTS_BUFFER` | `256` | broadcast channel capacity |
| `X402_EVENTS_MAX_SUBSCRIBERS` | `64` | at the cap `/events` returns 503 + `Retry-After` |

Operators should know what the default means: with `scope=all` + `detail=full` the stream
is public and carries the payer, tx hash and amount of **every** client of this
facilitator. Both dials narrow it without a code change.

### Fix — three defects in the first pass, caught in review before deploy

- **The event's `network` was not a network name.** It was built with `format!("{:?}")`,
  which prints the enum *variant*, so `SkaleBase` went out as `skalebase` and
  `BaseSepolia` as `basesepolia` — names that match nothing in `/supported`. A consumer
  that maps by slug drops those events silently, which looks like a stream that almost
  works. Now uses `Display`, the canonical slug.
- **The rate limit and subscriber cap the plan called non-negotiable were missing.**
  `/events` is public, unauthenticated and long-lived, on the same task that settles
  payments — `publish()` cannot be slowed by one observer, but unbounded observers could
  starve the process without ever touching the money path. Now behind the same
  `SmartIpKeyExtractor` governor as the rest (1 token/2s, burst 10) with bounded
  admission.
- **`verify` never published**, though the design lists `verify`/`settle` and consumers
  already listen for it. A verify publishes without a `tx`: nothing settled yet, and
  inventing a hash would make the stream lie.

`/events` is documented in `src/openapi.rs`, so it shows up in `/docs`.

## [1.59.4] - 2026-07-28

### Fix — four dead block explorers reached the public landing page

`config/supported_tokens.json` is the JSON source of truth other projects sync
their explorer links from, and four of its entries no longer resolved. Two of
them were also hardcoded as clickable links on the landing page, so a real
settlement looked fabricated when the operator tried to open its receipt.

| Network | Was | Now |
|---|---|---|
| monad (143) | `monad.socialscan.io` — 429 always | `monadscan.com` |
| skale-base (1187947933) | `skale-base.explorer.skalenodes.com` — no DNS | `skale-base-explorer.skalenodes.com` |
| hyperevm (999) | `purrsec.com` — 404 | `hyperevmscan.io` |
| skale-base-sepolia | `base-sepolia-testnet.skalenodes.com` — 404 | `base-sepolia-testnet-explorer.skalenodes.com` |

Every domain was checked with a browser user-agent, and the mainnets with a real
address. `monadscan.com` and `skale-base-explorer` are also the chainlist
canonical explorers for those chain IDs, and `hyperevmscan.io` was already in
`static/index.html` — the JSON had drifted from the HTML in the same repo.

Left alone deliberately: `testnet.purrsec.com` 404s but `testnet.hyperevmscan.io`
does not resolve, so there is no verified improvement to make. Avalanche and BSC
return 403 to curl, which is Cloudflare's anti-bot check, not a dead explorer —
they open fine in a browser.

`static/index.html` is baked into the binary via `include_str!`, so the landing
page kept serving the dead links until this rebuild. Reported from KarmaCadabra,
fixed in `ff3c4123`.

## [1.59.3] - 2026-07-27

### Fix — an ignored query parameter is not a search

`GET /discovery/resources` accepted any query parameter and applied only the ones
it knew. `?q=logs` filtered server-side and returned 3 of 13,590; `?search=logs`
was accepted, ignored, and returned the full unfiltered page. From the caller's
side those two responses are indistinguishable from a filter that matched
everything, so a consumer read the ignored parameter as a working search and
spent months filtering 100 arbitrary rows locally and calling that the result.

- Any parameter outside `limit`, `offset`, `category`, `network`, `provider`,
  `tag`, `source`, `sourceFacilitator`, `health`, `tier`, `q` is now a 400 whose
  body lists the supported set, so the fix is in the error.
- When exactly one parameter is rejected and its intent is obvious, the response
  carries a hint: `search`/`query`/`text`/`keyword`/`filter` point at `q`,
  `page` at `offset`, `status` at `health`, `curation` at `tier`. The names are
  bounded in count and length before they are echoed back — they come from an
  unauthenticated caller.
- OpenAPI documents the rejection, the 400 example, and that timestamps are
  epoch **seconds** as JSON numbers.

Reported by KarmaCadabra. The same report turned up two SDK bugs, both fixed and
published: `uvd-x402-sdk` **0.27.0** (PyPI) — `firstSeen` is an epoch int but the
Python model declared it `str`, so every `list_resources()` call raised
`ValidationError`, and `health` / `curation` were dropped by the model entirely.
`uvd-x402-sdk` **2.42.0** (npm) — `BazaarClient` targeted
`bazaar.ultravioletadao.xyz`, a host that does not resolve and never has, with an
invented schema; it now speaks the real `/discovery/*` contract.

The registry itself was never at fault: 21,254 registered, 13,620 visible, 7,634
quarantined, 1,879 alive, and 10 of 10 hand-probed URLs live.

## [1.59.2] - 2026-07-27

### Fix — upstream feed failures no longer read like our own 5xx

When an upstream bazaar feed is down, the aggregator logged
`ERROR ... Failed to fetch from facilitator ... error=Facilitator error: HTTP 500`.
Three problems: ERROR severity for a routine condition (several configured
sources are permanently broken and it affects no response we serve), no
indication the failure was outbound, and the upstream's own status quoted inline
so the line reads as though the facilitator returned a 500. That combination cost
real time chasing a 500 incident that never happened — the actual symptom that
day was HTTP 429 from a mis-sized rate limit.

- Aggregator and crawler now log upstream failures at WARN with
  `direction=outbound`, `upstream_url` and `upstream_error`. Only our own
  responses carry `status=NNN` (emitted by `telemetry`), so the two can no longer
  be confused by a grep. Internal failures (e.g. importing into the registry)
  remain ERROR.
- `CLAUDE.md` gains a "confirm before chasing" runbook: check ALB
  `HTTPCode_Target_5XX_Count` first, then count our own `status=NNN` in logs, and
  note that a few `503`s around a deploy are ECS task replacement.

## [1.59.1] - 2026-07-26

### Fix — reject an unsettleable UPTO witness before spending RPC

`resolve_settling_signer` ran after the Permit2 allowance and balance checks, so a payload
naming a facilitator address we do not hold still cost two on-chain round-trips before being
rejected — and the allowance error masked the real reason. Verified against production during
the first end-to-end mainnet settlement: a deliberately wrong `witness.facilitator` came back
as "Insufficient Permit2 allowance" instead of the mismatch. It now resolves immediately after
the off-chain validations, before any RPC.

Closes out the UPTO path: settlement proven end-to-end on Base mainnet
(tx `0x52e93c878bc3e0337e1741f30646d0671c92e02cf9a858f2f869eca62fdab573`, 0.01 USDC charged
against 0.03 authorised, sender the pinned facilitator EOA), and replay correctly rejected by
Permit2 with `InvalidNonce()`.

## [1.59.0] - 2026-07-26

### Fix — reverted transactions were reported to merchants as successful

`send_transaction` never checked `receipt.status()`; only the EIP-3009 path did. A reverted
`release` or `refundInEscrow` therefore came back as `success: true` with a real transaction
hash — roughly 297 caller-pinned calls a month across 9 mainnets were exposed. Same defect
class as FAC-1, fixed for ERC-8004 in v1.49.0 and still open everywhere else. The check now
lives inside the send path rather than at each call site.

### Fix — signer pinning for every write that binds `msg.sender`

Added `send_transaction_from`, which errors rather than silently falling back when asked for
a signer the wallet does not hold. Three families are now pinned:

**UPTO.** Proven on-chain against the deployed proxy: it enforces
`msg.sender == witness.facilitator` unconditionally, and `Address::ZERO` is *not* a wildcard
— it reverts `UnauthorizedFacilitator` (`0x0f6fae87`) exactly like a mismatch. Since
`facilitator` sits inside the EIP-712 witness typehash, the payer commits to one address at
signing time and it can never be rotated without breaking the signature. `witness.facilitator`
is now required and must equal the pinned signer, validated before any RPC call, and both
simulation and broadcast use it. Previously it was parsed, defaulted to zero, and never
checked — so a client that omitted it got a guaranteed revert, and verify could approve a
payment that settle could not complete.

**x402r PaymentOperator.** All 8 reachable legacy mainnets pin the canonical EOA in a
`StaticAddressCondition` on both `release` and `refundInEscrow`; the condition bytecode is
byte-identical across chains, and a differential `eth_call` on Optimism confirms a
non-canonical sender gets `ConditionNotMet`. All three operator writes are pinned.

**ERC-8004** stays on the default signer, as before: feedback is keyed by
`(agentId, msg.sender)`, so a rotated writer would fragment the facilitator's reputation
identity.

### Fix — a reverting payload could stall settlement (regression from 1.58.0)

Alloy fills gas and nonce concurrently (`try_join!` in `JoinFill::prepare`), so
`NonceFiller` consumes a nonce before a failing gas estimate can return. The high-water mark
added in 1.58.0 then held that gap for up to 120s — and `/settle` is unauthenticated, so a
stream of invalid payloads could stall legitimate settles. Gas is now estimated *before* the
nonce is reserved, so a revert costs nothing, and failures that provably never reached the
mempool hand their nonce back via `release_nonce`. The explicit gas limit also saves the
filler's own estimate round-trip on the happy path.

### Add — single-writer lease for EVM submission

ECS runs two tasks on every rolling deploy (`minimumHealthyPercent=100` /
`maximumPercent=200`), each with a private nonce cache, about 32 times a month. Measured
exposure is entirely deploy-driven — the service has never autoscaled.

A conditional `PutItem` on the existing `facilitator-nonces` table elects one writer: 15s
TTL, 5s renewal, explicit release on shutdown for immediate handover. Non-holders keep
serving reads and refuse only EVM writes. **Fail-open** — if DynamoDB is unreachable the
process assumes the writer role and logs loudly, degrading to the previous behaviour rather
than refusing payments. Kill-switch: `ENABLE_WRITER_LEASE=false`.

No terraform or IAM change: the table, its TTL, the VPC gateway endpoint and
`dynamodb:PutItem` all already existed.

Rejected alternatives, with reasons, in
`docs/plans/upto-blockers-and-single-writer-2026-07-26.md`: forcing serial deploys costs
~130s of hard downtime each; moving nonces to DynamoDB turns a race into a permanent gap;
sharding by task index is impossible because ECS exposes no ordinal.

## [1.58.0] - 2026-07-26

### Fix — the EVM nonce lane under concurrent settles

Response to Execution Market's report of `nonce too low` / `replacement
transaction underpriced` under concurrent settles (their INC-2026-07-06, and the
handoff in `docs/HANDOFF-2026-07-24-signer-pool-concurrencia.md`). The handoff
asked for a signer pool; the pool already exists and works
(`next_signer_address`, comma-separated `EVM_PRIVATE_KEY_MAINNET`), so the work
went to the defects that actually break concurrency. Full analysis in
`docs/plans/em-concurrency-response-2026-07-24.md`.

**A resync could rewind under an in-flight transaction.** `reset_nonce` wiped the
cached nonce entirely, so the next allocation refetched the chain's pending
count. When an RPC load balancer routed that refetch to a node that had not yet
seen our transactions, the refetched value sat *below* nonces already handed out
— and reusing one tries to replace an in-flight transaction rather than queue
behind it, which is exactly `replacement transaction underpriced`. The manager
now keeps a per-address high-water mark that a resync can never rewind below.
The mark is released after `NONCE_TRUST_CHAIN_AFTER` (120s) so that a genuinely
dropped transaction does not wedge the signer behind a nonce gap forever.

**Ethereum L1 bypassed the nonce manager entirely.** The L1 branch set the nonce
by hand from a *latest*-block transaction count. Setting a nonce makes alloy skip
`NonceFiller` completely, so two concurrent L1 settles deterministically received
the same nonce; the retry re-derived the same value and failed again. Worse, the
lookup was `unwrap_or(0)` — a rate-limited probe stamped nonce 0 into the
transaction, a guaranteed `nonce too low` for any funded signer. The override is
gone; the manager handles L1 like every other chain.

**A failed probe suppressed the retry.** The "did the transaction actually mine?"
guard compared two transaction counts, both `unwrap_or(0)`. Under rate limiting
the pre-send probe collapsed to 0, making `post > pre` trivially true, so the
guard concluded the transaction had mined and skipped the retry — under exactly
the conditions where the retry matters. Both probes are now fallible and the
guard only fires when both answered; when either fails we decline to retry rather
than guess.

**Rate limits now retry instead of killing the settle.** There was no retry or
backoff at the transport layer at all. Added alloy's `RetryBackoffLayer`, which
recognises HTTP 429 and the provider rate-limit codes (`-32005` Infura, `-32016`
Alchemy, `-32012`/`-32007` QuickNode) and honours any backoff hint. EM measured
258 of these on a shared 50 req/s Base budget, each one killing a settle or
refund. The client-side throttle is off by default; set `RPC_MAX_CU_PER_SECOND`
to stay under a known provider budget (~20 CU per request, so 1000 CU/s is
roughly 50 req/s).

**Recoverable collisions were reported as hard failures.** `is_nonce_error`
required the literal word "nonce", so geth's bare `already known`, the short
`replacement underpriced`, and `nonce too high` all fell through untreated.
Retries raised from 1 to 2, with exponential jittered backoff — a fixed delay
made settles that had just collided wake together and collide again.

### Fix — ERC-8004 owner resolution (`-32003 out of gas`)

`GET /identity/{network}/owner/{address}` had been failing on Base since
2026-07-05. `resolve_first_token_by_owner` built a single Multicall3 batch with
one `ownerOf` call per token — about 58,400 on Base — which exceeds the node's
600M gas cap. Measured limits: the production RPC fails on gas, and a public Base
node caps the response body at ~16,383 calls (~2.5 MB). The scan is now split
into batches of 2,000 (~6M gas, ~320 KB) with early exit at the first match, and
a hard ceiling on batches per scan.

**RPC failures were read as "token does not exist".** The probe and binary search
treated any error as a missing token, so a rate limit silently truncated the scan
range. `is_execution_revert` now only accepts a recognised contract revert;
anything else is inconclusive. The resolver returns three distinct outcomes, and
`POST /register` **fails closed** on the inconclusive one instead of minting —
previously each such failure minted a duplicate identity NFT, growing the very
registry that broke the scan.

The inconclusive case also returns `503 + retryable: true` rather than `404`.
Callers persist "not registered" from a `404` and stop asking, which is how a
transient RPC failure turned into a permanently null agent ID downstream.

Successful resolutions are cached for 5 minutes, keyed by network (the ERC-8004
registries share one deterministic address across chains).

## [1.57.1] - 2026-07-25

### Fix — the Bazaar read limit was throttling legitimate pagination (429s)

The read-route rate limit added in 1.56.0 was sized wrong: 30 req/min per IP.
Paging a 21k-item catalog is ~212 requests at the 100/page cap, so any consumer
walking the bazaar exhausted the budget in seconds and then got 429s. Production
confirmed it — every 429 observed (502 of them in six hours) was a paginating
client on `/discovery/resources`, and the ALB's 4xx rate stepped up right after
1.56.0 rolled out.

Raised to 1 token per 200ms with a burst of 120 (~300 req/min sustained), which
covers a full-catalog walk in about 45 seconds while still cutting off a
hammering loop. These are in-memory reads (~100ms even with a `q=` scan), so the
tighter budget was never justified.

Note for the record: the facilitator was not returning 500s during this window.
Target 5xx totalled 3 responses in 12 hours (503s consistent with ECS task
replacement during the day's deploys) and no `status=500` was logged at all.

## [1.57.0] - 2026-07-24

### Bazaar — documentation, landing section, and a faster initial sweep

- **README**: the curated Bazaar was entirely undocumented. Added a section
  covering the ingestion filter, the 402 liveness probe (including the MCP
  handshake and the quarantine/recovery hysteresis), curated tiers, on-chain
  ERC-8004 verification, and the prober's SSRF hardening plus the payTo-drift
  alarm. The API table now lists `/discovery/stats`, `/bazaar` and `/docs`, and
  documents the real filters on `/discovery/resources`.
- **Landing page**: new Bazaar section (EN/ES) with live counters — listed
  endpoints, verified-alive, first-party + VIP, aggregated facilitators — read
  from `/discovery/stats`, plus a call to action into `/bazaar`. Counters fall
  back to dashes rather than a stale number if the request fails.
- **Prober throughput**: default `DISCOVERY_HEALTH_CONCURRENCY` raised from 15 to
  40. Most of the sweep's wall-clock is probes sitting on the 12s timeout, so at
  15 the initial pass over the ~21k catalog was tracking to ~19 hours. Politeness
  toward any individual target is unchanged — that is enforced separately by the
  per-host cap of 3 probes per tick.

## [1.56.1] - 2026-07-24

### Fix — admin routes no longer reveal themselves via a 400 on a malformed body

With `BAZAAR_ADMIN_TOKEN` unset the admin routes are supposed to be
indistinguishable from routes that do not exist (404). They were not: axum's
`Json` / required-`Query` extractors run *before* the handler, so a malformed
body (or a missing `url` param) returned 400 and leaked the route's existence.
The bodies are now taken as raw bytes and the query param as `Option`, and both
are parsed only after authentication succeeds.

## [1.56.0] - 2026-07-24

### Bazaar — search, stats, admin curation, MCP probing, payTo-drift alarm, rate limits

Closes every deferred item from the curated-bazaar plan.

- **Server-side search**: `GET /discovery/resources?q=` searches the whole
  catalog (url / description / provider / category / tags, case-insensitive)
  instead of only the loaded page. Capped at 128 characters (400 beyond that),
  since the scan is O(catalog) on a public route. The `/bazaar` UI now uses it,
  so result counts are accurate.
- **`GET /discovery/stats`** (new): catalog aggregates — `total`, `visible`, and
  breakdowns by source, sourceFacilitator, network, tier and health — served
  from a 60-second in-process cache. The UI's metrics band is now one request
  instead of four count-probes.
- **Admin curation API** (gated by `BAZAAR_ADMIN_TOKEN`; every route answers 404
  when it is unset, so the surface does not exist unless configured):
  `DELETE /discovery/resources?url=`, `POST /discovery/admin/suppress`,
  `POST /discovery/admin/release`. Bearer token compared in constant time, the
  credential is never logged, URLs are normalized before lookup, and the routes
  sit behind the strict ~5 req/min governor. Suppression hides a resource from
  every listing and from stats without deleting it.
- **MCP handshake probing**: `type: mcp` resources are probed with a JSON-RPC
  `initialize` POST instead of a GET. Previously our own first-party MCP
  endpoints (Execution Market, 402Milly) could never report `alive`, since MCP
  servers do not answer a bare GET with a 402.
- **payTo-drift alarm (F4)**: when a live 402 advertises a recipient the listing
  never declared, the resource is quarantined immediately (bypassing the failure
  hysteresis) and a `paytoswap` warning is logged with both the expected and the
  observed recipients. This is a hijack signal, not a liveness signal.
- **Read-route rate limiting (F6)**: `/discovery/resources` and
  `/discovery/stats` were ungoverned; they now carry a ~30 req/min per-IP limit,
  so an unauthenticated loop can no longer contend on the catalog read lock and
  slow the aggregator and health prober.
- **OpenAPI rewritten for the Bazaar**: the section documented a `type` param
  that never existed and a v1-shaped response. It now documents the real wire
  format (including `health` and `curation.verification`), all 11 query params,
  the tier ordering, and every new endpoint — list, stats, UI, attestation
  evidence, register and the three admin routes.

Also: `docs/handoffs/2026-07-24-execution-market-bazaar-listing.md` — handoff for
the Execution Market team covering why `POST /api/v1/tasks` cannot be listed as a
fixed-price payable, the verified ERC-8128 signing recipe, and the three
decisions needed from them (plus adjacent MeshRelay / 402Milly fixes).

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
