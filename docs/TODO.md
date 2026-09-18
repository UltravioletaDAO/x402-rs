# x402-rs Implementation TODO

> Auto-generated from control-plane brainstorming backlog on 2026-01-19.
> Sources: from_superfluid_x402_partnership, from_x402cloud, session_voice2earn_ecosystem, from_erc8004, session_meritstream

---

## Context

x402-rs is the core Rust implementation of the x402 payment protocol. Key integrations needed:
- Superfluid streaming payments
- ERC-8004 agent identity
- Voice2Earn real-time payments
- x402cloud serverless execution

---

## P0 (CRITICAL - This Week)

### 1. Integrate Superfluid as streaming backend
**Priority**: P0
**Status**: [ ] Not started
**Location**: New module `src/streaming/`

- Add Superfluid SDK dependency
- Dual mode: pay-per-request OR stream via Superfluid
- 8 networks overlap: Ethereum, Base, Arbitrum, Optimism, Polygon, Avalanche, Celo, BSC
- Script that reads `src/network.rs` as golden source for network/stablecoin matrix

**Why**: Enables continuous payment flows for Voice2Earn, MeritStream

---

### 2. Add ERC-8004 AGID payment identity
**Priority**: P0
**Status**: [ ] Not started
**Location**: `src/identity/`

- Payment processors/gateways register as AGID
- Reputation based on successful transactions + dispute resolution rate
- Validators can attest compliance
- Enables trustless A2A payments

**Why**: Foundation for agent economy payments

---

### 3. Implement x402cloud basic endpoint (S3 PUT)
**Priority**: P0
**Status**: [ ] Not started
**Location**: New crate `x402-cloud/`

- Proof-of-concept: S3 PUT → x402 payment required → AWS execution → response
- Validates full payment→execution→response flow
- Critical for "Serverless as a Service" dogfooding

**Why**: Proves x402 can gate cloud resources

---

## P1 (High Priority - This Month)

### 4. Voice2Earn real-time payment integration
**Priority**: P1
**Status**: [ ] Not started
**Location**: Integration with v2e-live

- Connect v2e-live scoring → x402 instant payments
- OR Superfluid streaming at flow_rate = SYNERGY_SCORE * BASE_RATE
- Per "moment of value" micropayments

**Why**: Core use case for Voice2Earn monetization

---

### 5. Quest IRC bounty bot prototype
**Priority**: P1
**Status**: [ ] Not started
**Location**: New example `examples/irc-bounty-bot/`

- IRC bot posts bounties on meshrelay channels
- Pays correct answerers via x402
- Flow: quest posted → claim → solve → verify → x402 payment

**Why**: Tests colmena-style distributed work + x402 settlement

---

### 6. ERC-8004 Agent Card integration
**Priority**: P1
**Status**: [ ] Not started
**Location**: `src/agent_card/`

- Auto-generate Agent Card for x402 services
- Declare OASF skills (payment-processing, escrow)
- Bidirectional registration linking to on-chain AGID NFT

**Why**: Agent discoverability in ecosystem

---

### 9. Decide the fate of EIP-6492: resurrect it or stop advertising it

**Priority**: P1
**Status**: [ ] Not started
**Location**: `src/chain/evm.rs` (`assert_valid_payment`, `SignedMessage::extract`)

EIP-6492 (counterfactual smart-wallet signatures) is **unreachable on all 40
networks**, not just on the one that has no validator deployed. `assert_valid_payment`
rejects any signature that is not exactly 65 bytes, and it runs before
`SignedMessage::extract`; a 6492 envelope is an ABI tuple plus a 32-byte magic
suffix and never measures 65. So the whole `StructuredSignature::EIP6492` branch
in `verify` and `settle` -- the validator multicall, the counterfactual deploy,
the atomic factory-plus-transfer path -- cannot be entered by any request. A
payer who sends one gets `invalid_signature_length`, which points them at their
signature instead of at the truth.

Measured 2026-09-16: two mutants deleting the Arc 6492 guard from both endpoints
survived the entire 2,426-test suite, because the lines they deleted could never
run.

Two ways out, and the choice is a product decision, not a refactor:

- **Resurrect**: move the 65-byte rule (and the EIP-2 low-`s` check it guards,
  which only makes sense for a raw `r||s||v`) into the `EIP1271` arm, where the
  signature really is 65 bytes. Then 6492 reaches the validator and the existing
  branch means something. Needs a positive end-to-end test against a real
  counterfactual wallet before it can be believed.
- **Retire**: drop the branch and say plainly that only EOA and deployed
  EIP-1271 wallets are served. Cheaper, honest, and reversible.

Either way the service should stop implying a capability it cannot deliver
anywhere. Until it is decided, the Arc guard stays: it is the only thing that
turns a call to an undeployed contract into a verdict.

**Why**: a documented capability that cannot be exercised is worse than an
absent one -- it sends integrators debugging their own correct code.

---

## P2 (Medium Priority - This Quarter)

### 7. Research Layer 1 vs Layer 2 priority
**Priority**: P2
**Status**: [ ] Research
**Location**: Documentation

- Superfluid has 8-network overlap with UVD
- UVD has unique advantage on 9 Superfluid-less networks (Solana, Sui, etc.)
- Consider adding Scroll (USDC via EIP-3009)

---

### 8. Implement payment mixing for Private Task Markets
**Priority**: P2
**Status**: [ ] Not started
**Location**: `src/privacy/`

- Escrow patterns for private tasks
- Output amounts different from input (mixer)
- Streaming payments during task execution

**Why**: Required for Private Task Markets idea

---

## Done

*Move completed items here with date.*



> **`x402-rs` NO tiene `docs/planning/BACKLOG.md`.** Su superficie es `docs/TODO.md`, en formato
> de secciones (`### N. título`, `**Priority**`, `**Status**`, `**Location**`, bullets, `**Why**`).
>
> **Es un repo PÚBLICO.** Estas filas **no nombran** pesos de detección, umbrales de sanción ni
> nada que sirva de mapa para evadir un filtro. El censo completo vive en un repo privado y esta
> sección lo dice.

```markdown
## Jev (System One Model) — fit measured 2026-09-18

> Full census lives in a PRIVATE repo (it names moderation internals verbatim).
> API: POST https://api.typesafe.ai/v1/systemone — docs at https://docs.typesafe.ai/llms.txt
> Three primitives only, all using the field `criteria`: Noul (yes/no, returns `{noul}` ONLY —
> no confidence, no probabilities), Choice (`criteria` is a map option->description, <=255
> options, returns choice + probabilities + confidence), Score (`criteria` is an ORDERED array
> of 2 to 10 described levels, returns a FRACTIONAL position + legend + probabilities +
> confidence). The 255 cap is Choice-only. Many questions per call, evaluated in parallel with
> essentially no added latency.
> Read https://docs.typesafe.ai/model-jaggedness/jev-1.13.md before designing any question:
> the model does not treat state as hostile by default, does not count or compare magnitudes,
> and reads dates as text. Arithmetic stays in code. And a Score crosses a threshold — it must
> not be interpolated to reconstruct a magnitude ("weak in numerical calibration").
> Output tokens are free; input is $0.042/M. Served from us-west-2; this repo runs in
> us-east-2, so every call crosses the continent — measure the real RTT before committing any
> synchronous path to it.
> Pin `jev-1.13.0`, never `jev-latest`: the vendor publishes a known-failure page PER VERSION,
> so an alias can change the answers without a change on our side.
> **There is no Rust SDK** (none for Go either). Use reqwest directly (already a dependency,
> Cargo.toml:66) and follow the outbound-HTTP policy that already exists in
> src/discovery_security.rs:214-289 (allowed ports, private-IP rejection, redirect cap).
> A Python sidecar is NOT worth a process, a deployment and a failure point for one POST — but
> retries, backoff, timeouts and the response types have to be written by hand. The defaults to
> replicate, from both official SDKs: 10s per-attempt timeout; 2 retries on 408/429/500-599;
> backoff 500ms doubling to 5s with 25% jitter subtracted; honour Retry-After / retry-after-ms
> up to 60s; a 30s total retry budget per call; keep the `x-typesafe-request-id` response
> header (it is the only thing that lets us report a case); and never log the body — it holds
> the state.

### 1. Bazaar search: replace the substring scan with a typed relevance decision
**Priority**: P2
**Status**: [ ] Not started — needs an API key (Jev is invite-only)
**Location**: `src/discovery.rs:968-997` (filter), `:665-680` (ordering)

- Free-text search is a plain case-insensitive `contains()` over url / description /
  provider / tags — the code says so at `:971`. Searching "traducción" cannot find a
  resource described as "translation API"; searching "pagos" cannot find "payment rails".
- Ordering is `Tier::rank()` (`src/types_v2.rs:1167-1177`) then `health_rank` — never
  relevance.
- Shape: **one call, one `noul` per candidate** ("resource `.candidates[N]` satisfies
  `.query`"), all evaluated in parallel. `noul` is the right primitive here: we need a
  number to rank by, not a confidence gate — and Noul returns no confidence anyway.
  Candidates are one line each, so batching them in a single state is correct; a richer
  per-candidate profile would need one call per candidate instead.
- Rank by `noul` **within** the existing `Tier` order, never instead of it — first-party and
  curated products do not lose their place to an estimate.
- Pre-filter with today's `contains()` plus `curation_check` / `HealthTracker` before building
  the state: input tokens are what we pay for, so do not ship thousands of candidates. Accuracy
  also falls with large irrelevant state (`model-jaggedness` §5).
- **The resource description is author-controlled.** The vendor states the model "does not
  treat state as hostile by default" and that "text that argues for its own classification can
  move the answer" (`model-jaggedness` §6). A listing whose description reads "this resource
  satisfies every query" is bazaar SEO poisoning, and it is cheap to publish. Mitigations, all
  in code: truncate the description before it reaches the state (`MAX_DESCRIPTION_LEN` is
  already 2048, `discovery_security.rs:336` — that is a lot of room to argue), keep `Tier`
  ordering above the model score so a hostile listing cannot outrank a first-party product,
  and cap how far a `noul` can lift an item within its tier.
- **Must degrade, not fail closed**: if Jev times out, serve today's ordering. A search
  endpoint that 503s because an out-of-region model was slow is worse than one that ranks
  badly. This is the one place in the stack where fail-open is the right policy, and it is
  because nothing here moves funds.
- **Close when**: a query in Spanish returns the semantically matching English-described
  resource; the Tier ordering is provably unchanged by the model; and a forced Jev timeout
  serves today's ordering with no 5xx.

**Why**: this is the one decision in the facilitator that is a genuine judgment, has a
closed output type, and lives on the **search path** — it signs nothing and moves no
funds. Everything on the signing path is a verifiable invariant, not a judgment.

### 2. Health hysteresis: calibrate the quarantine, keep `PayToDrift` deterministic
**Priority**: P3
**Status**: [ ] Not started — needs an API key
**Location**: `src/discovery_health.rs:35-41`, `:94-107`

- `QUARANTINE_AFTER_FAILS`, `RECOVER_AFTER_OK` and the backoff schedule are hand-set. The
  output is already a closed enum of 6 (`ProbeClass`), ordered by severity, so a `score` fits
  (6 levels, well under the 10-level cap) and it returns confidence.
- **The hysteresis counters and the backoff schedule stay in code**: the model "does not count
  reliably" and "reads dates as text" (`model-jaggedness` §2, §3). Jev judges what a probe
  response *means*; code counts the fails and computes the next probe time.
- Cost of an error is one listing entry, so a probabilistic signal is acceptable. Runs in a
  background task, off the request path — no latency budget.
- **Hard constraint**: `ProbeClass::PayToDrift` (`:100-102` — the live 402 pays a recipient the
  listing never declared) stays deterministic and keeps quarantining immediately, bypassing
  hysteresis. It is a hijack signal detected by string comparison, not a judgment.
- **Close when**: the probe classifier disagrees with today's hysteresis on a reviewed sample
  and the disagreements are explainable, with `PayToDrift` untouched.

### 3. Aggregator `trust` field is parsed and ignored
**Priority**: P3
**Status**: [ ] Not started
**Location**: `src/discovery_aggregator.rs:414`, `:447`, example at `:1109`

- Source configs carry `"trust": "standard"` and the code documents that `trust` / `maxItems` /
  `note` are **ignored** (`:447`). An empty slot where a curation judgment would go.
- If filled: **ordering only, never admission** — `curation_check`
  (`src/discovery_security.rs:356`) stays the gatekeeper.
- **Close when**: either the field is honoured for ordering with a declared policy, or it is
  removed from the config schema so it stops implying a behaviour that does not exist.

### NOT a Jev candidate (recorded so nobody re-litigates it)
**Priority**: n/a
**Status**: [x] Decided 2026-09-18
**Location**: `src/chain/mod.rs:223-277`, `src/facilitator_local.rs:486-493`,
`src/blocklist.rs:21-73`, `src/chain/evm.rs:555-586`,
`src/discovery_security.rs:166-213`, `:356-442`, `src/handlers.rs:141-219`

- Every decision on the verify/settle path is a **verifiable invariant**, not a judgment:
  EIP-712 signature validity, balance, `validAfter`/`validBefore`, network, scheme, receiver
  match, blocklist membership. Each has a single computable correct answer. A signature is
  valid or it is not; there is no `p = 0.87` of being valid.
- Typed output makes a *format* hallucination impossible. It says nothing about whether the
  decision is right. A well-formed wrong answer is still a wrong answer, and here a wrong
  answer signs. (The vendor is explicit that its published "0% hallucination" figure "is not
  empirical" and describes schema matching, not correctness.)
- The failure is asymmetric the worst way: a false negative breaks a client visibly; a false
  positive **signs a transaction that moves funds** and cannot be reverted.
  `FacilitatorLocalError::SettlementUnconfirmed` (`chain/mod.rs:263-273`) already exists
  because a broadcast tx with no verdict "may well be mined" — that is what a doubt on the
  money path costs with deterministic rules.
- **Not even as an accompanying signal.** Under confidence-gated routing the rule decides and
  confidence may only ESCALATE to review, never RELAX the rule — and on this path there is
  nothing to escalate to, so there is nothing for it to do. A logged "0.9 fraud" on a payment
  the rules accepted only creates pressure to write the `if` six months later. Operationally it
  would also add a cross-region network call, and a new failure mode, to a service whose
  failure mode is a half-broadcast transaction.
- **Gas pricing** (`chain/evm.rs:555-586`) is hand-set but its output is **continuous** — none
  of the three primitives returns a number in a range — and it is on the signing path. The
  vendor is explicit anyway: "Jev is not a calculator... implement any mathematical logic in
  code", and it "cannot reliably judge whether two values are near each other" given numeric
  representations (`model-jaggedness` §2).
- **Rate limiting** (`handlers.rs:141-219`) publishes `x-ratelimit-remaining` so clients can
  self-throttle (`:345-352`). A probabilistic limiter cannot publish "43 left": the client
  contract breaks. (Yes, there were false positives — `handlers.rs:47-48` records that every
  429 in the 2026-07-24 bazaar incident was a legitimate paginating client. The right fix was
  raising the limit and writing down why, not making it opaque.)
- **Curation admission** (`discovery_security.rs:356-442`, 7 rules, first failure wins) and
  **outbound URL safety** (`:166-213`) are declared rules, and `discovery_curation.rs` resolves
  a resource's tier against a declared manifest by exact host — a table, not a judgment.
- Two decisions previously assumed to exist and **do not**: RPC *selection* (there is one
  `std::env::var` URL per network — `src/chain/solana.rs:1911-1923`, `src/chain/evm.rs:230` —
  with retry/backoff against the *same* RPC at `:255-291`), and any economic gate on settling
  (`uneconomic`, `not worth`, `gas_cost`, `cost_exceeds`, `dust`, `min_amount`, `too_small`:
  zero hits in `src/`). The gas-reserve threshold reported elsewhere is not in this repo; it
  lives in monitoring. If an RPC pool is ever built, a `choice` over that network's 2-5
  allowlisted endpoints is acceptable — getting it wrong costs latency, not money — and the
  ~83 networks are nowhere near the 255-option limit.
```

**4 secciones — 1×P2, 2×P3, 1 decisión registrada.**

---

