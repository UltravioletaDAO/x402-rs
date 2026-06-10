# ERC-8004 Reputation Forgery + Cross-Customer Feedback Destruction via Unauthenticated `/feedback` and `/feedback/revoke` (P1)

> Audit: x402-rs facilitator security review, 2026-06-10. Finding ID `erc8004-forgery`. Verifier lens: control-hunt. Confirmed **P1**.

## Summary

The ERC-8004 reputation write endpoints `POST /feedback` and `POST /feedback/revoke` (and `POST /feedback/response`, `POST /register`) take every field from the request body and immediately sign + broadcast an on-chain transaction with the **facilitator's own EOA**, with **no proof that the caller controls the agentId/wallet or actually paid the agent**. Because the on-chain `IReputationRegistry.giveFeedback(...)` carries no `clientAddress` parameter, every relayed entry is attributed to the single facilitator address, so an unauthenticated attacker can (a) fabricate arbitrary positive/negative feedback (`value: i128`, may be negative) for **any** agent on all 20 ERC-8004 networks, and (b) revoke **any** feedback the facilitator ever relayed by guessing a small `feedbackIndex` — all gaslessly, at the facilitator's gas expense. The routes carry **no rate limit** (`handlers::routes()` is merged at `main.rs:361` without a `GovernorLayer`), so the entire facilitator-relayed reputation corpus can be rewritten or its gas treasury drained in bulk.

## Root cause

### 1. No caller authentication / proof-of-interaction on the EVM feedback path

`src/handlers.rs::post_feedback` (lines 2326–2640). The EVM branch builds the contract call straight from body fields and sends it with the facilitator's signer — there is no `recover`/`ecrecover`/signature/owner check anywhere in the handler:

```rust
// src/handlers.rs:2527-2541 (EVM branch of post_feedback)
let reputation_registry =
    IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

let feedback_hash = feedback.feedback_hash.unwrap_or_default();

let call = reputation_registry.giveFeedback(
    alloy::primitives::U256::from(agent_id_u64),
    feedback.value,            // attacker-controlled, i128 (may be negative)
    feedback.value_decimals,
    feedback.tag1.clone(),
    feedback.tag2.clone(),
    feedback.endpoint.clone(),
    feedback.feedback_uri.clone(),
    feedback_hash,
);
// ... call.send().await  (signs with facilitator EOA)
```

`provider.inner()` is the facilitator's `EthereumWallet`-backed provider (`src/chain/evm.rs:327` `fn inner(&self) -> &Self::Inner`; wallet/signer addresses built in `from_env.rs::make_evm_wallet`). So the on-chain `msg.sender` (and therefore the recorded `clientAddress` in `NewFeedback`) is **always the facilitator**, never the requesting client.

### 2. The proof field that would close this is defined but never read

`src/erc8004/types.rs:193-195`:

```rust
/// Proof of payment (required for authorized feedback)
#[serde(skip_serializing_if = "Option::is_none")]
pub proof: Option<ProofOfPayment>,
```

`ProofOfPayment` (types.rs:366-385) and `ProofOfPayment::compute_payment_hash` (types.rs:416-441) are fully implemented but `request.feedback.proof` is **never consumed** in `post_feedback`. The only `proof` reference in `handlers.rs` is the JSON doc example at line 2279.

### 3. The on-chain registry has no ACL to refute this

The `IReputationRegistry` ABI states the model in its own doc comment (`src/erc8004/abi.rs:159-161`): *"Anyone can submit feedback for any agent."* `giveFeedback(...)` (abi.rs:207-216) takes **no** `clientAddress` param, and `revokeFeedback(uint256 agentId, uint64 feedbackIndex)` (abi.rs:221) is scoped on-chain only to `msg.sender`'s own entries. The ReputationRegistry is an **external deployed contract** (no Solidity source in `contracts/` — confirmed by `find . -name "*.sol" | grep -i reputation` → none of ours), so there is no on-chain control that can distinguish a legitimate paid review from a fabricated one. The integrity gate must live in the facilitator.

### 4. Revoke path: facilitator is the recorded submitter for everything it relayed

`src/handlers.rs:2799-2805` (EVM branch of `post_revoke_feedback`):

```rust
let reputation_registry =
    IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

let call = reputation_registry.revokeFeedback(
    alloy::primitives::U256::from(agent_id_u64),
    request.feedback_index,
);
```

The doc comment at handlers.rs:2644-2645 claims *"Only the original submitter can revoke their feedback"* — but the original submitter is always the facilitator. The on-chain `msg.sender == submitter` gate is satisfied for **every** feedback entry the facilitator ever relayed, so any caller who supplies `(agentId, feedbackIndex)` for a facilitator-relayed entry can wipe it.

### 5. No rate limit on these gas-spending writes (gas-treasury drain amplifier)

`src/main.rs:351-361`: only `verify_settle_routes` (line 353) and `discovery_register` (line 357) get a `GovernorLayer`. `handlers::routes()` — which holds `/feedback`, `/feedback/revoke`, `/feedback/response`, `/register` — is merged at `main.rs:361` with **no governor**:

```rust
let http_endpoints = Router::new()
    .merge(verify_settle)                                  // rate-limited
    .merge(handlers::routes().with_state(axum_state))      // NOT rate-limited  <-- /feedback, /register
    .merge(discovery_register)                             // rate-limited
    ...
```

Every forged `giveFeedback`/`revokeFeedback`/`register` is a real on-chain tx the facilitator EOA **pays gas for**. With no per-IP limit, an attacker turns the reputation graph into an unbounded gas-treasury drain.

## Exploit (production config: ENABLE flags irrelevant — routes are wired unconditionally, prod runs them)

Fabrication / defamation:

```bash
# Flood negative feedback to defame a competitor agent on Base mainnet.
for i in $(seq 1 1000); do
  curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback \
    -H 'content-type: application/json' \
    -d '{"x402Version":1,"network":"base",
         "feedback":{"agentId":<VICTIM_AGENT_ID>,"value":-100,"valueDecimals":0,"tag1":"scam"}}'
done
# Each call: facilitator signs giveFeedback() with its EOA, pays gas, records
# defamatory feedback attributed to the facilitator's clientAddress.
```

Destruction / censorship of a rival's positive reviews:

```bash
# Enumerate feedbackIndex 1..N and revoke each facilitator-relayed entry.
for i in $(seq 1 50); do
  curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/revoke \
    -H 'content-type: application/json' \
    -d "{\"x402Version\":1,\"network\":\"base\",\"agentId\":<AGENT_ID>,\"feedbackIndex\":$i}"
done
# Every revoke succeeds because msg.sender (facilitator) was the recorded submitter.
```

No authentication, no rate limit, no proof. `getSummary` consumers (the merchants/agents the facilitator vouches for) cannot tell legitimate paid reviews from fabricated ones, and legitimate reviews can be erased.

## Fix

Goal: **require cryptographic proof that the caller controls the paying wallet AND actually paid the target agent before the facilitator signs any reputation/identity write, and bound the gas exposure with a rate limit.** Two layers: (A) immediate gas/blast-radius cap (ship first, low risk), (B) proof-of-interaction gate (the real fix).

### Layer A — add a rate limit to the ERC-8004 write routes (`src/main.rs`, ~lines 342–362)

Carve the gas-spending ERC-8004 writes into their own router behind a strict governor, mirroring the existing `discovery_register` pattern. Add a route group helper in `src/handlers.rs` (new `erc8004_write_routes()` analogous to `discovery_register_routes()` at handlers.rs:189) containing `/register`, `/feedback`, `/feedback/revoke`, `/feedback/response`, and move those `.route(...)` lines out of `routes()` (handlers.rs:118-124).

**Before** (`main.rs:359-362`):
```rust
let http_endpoints = Router::new()
    .merge(verify_settle)
    .merge(handlers::routes().with_state(axum_state))
    .merge(discovery_register)
```

**After**:
```rust
// Reuse the strict 5 req/min config used for discovery_register, OR define a
// dedicated erc8004_write_config with the same builder (per_second(12), burst_size(5)).
let erc8004_writes = handlers::erc8004_write_routes()
    .with_state(axum_state.clone())
    .layer(GovernorLayer::new(discovery_register_config.clone()));

let http_endpoints = Router::new()
    .merge(verify_settle)
    .merge(handlers::routes().with_state(axum_state)) // now only reads + meta
    .merge(erc8004_writes)
    .merge(discovery_register)
```

Why it closes (part of) the hole: caps the gas-treasury drain and bulk-rewrite rate even before the proof gate ships. This is necessary but **not sufficient** — it does not stop forgery, only slows it. Ship Layer B too.

### Layer B — require a verified proof-of-interaction in `post_feedback` (`src/handlers.rs:2376` onward, before the provider dispatch at 2390)

Insert a proof-validation block immediately after the `is_erc8004_supported` check (handlers.rs:2374) and before `let provider_map = facilitator.provider_map();` (handlers.rs:2390). The block must:

1. **Require** `request.feedback.proof` to be present (reject 400 if `None`).
2. Verify the proof binds the caller to a real, finalized payment **to this agent**:
   - Fetch the settlement tx on-chain via the network provider for `proof.network` (EVM: `provider.inner().get_transaction_receipt(proof.transaction_hash)`); reject if missing, unconfirmed, or reverted (`receipt.status() == false`).
   - Confirm `proof.network == request.network` (feedback network must match the payment network).
   - Recompute `ProofOfPayment::compute_payment_hash(&proof.transaction_hash, proof.block_number, &proof.payer, &proof.payee, &proof.amount)` and reject on mismatch with `proof.payment_hash` (note: `compute_payment_hash` is currently private at types.rs:416 — change to `pub` or add a public `verify_payment_hash(&self) -> bool` method on `ProofOfPayment`).
   - Resolve the agent's on-chain owner via `IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone()).ownerOf(agent_id_u256)` (the registry/ABI already used at handlers.rs:3754-3762, abi.rs:126) and **require `proof.payee` to equal that owner** (or a wallet declared in the agent's identity metadata). This is the "you may only review an agent you actually paid" binding.
3. **Bind the request to the paying client**: require an EIP-712 signature from `proof.payer` over `(agentId, value, valueDecimals, tag1, feedbackIndex)` (add a `signature: Bytes` field to `FeedbackParams` / a new `FeedbackProofEnvelope`), recover the signer, and reject unless `recovered == proof.payer`. This is the **proof the caller controls the wallet** — without it, anyone replaying a public proof could still post.
4. **Prevent flooding per payment**: key a one-feedback-per-settlement record on `proof.payment_hash` in the existing nonce/idempotency store (`nonce_store.rs` / `idempotency_store.rs`) and reject a second feedback with the same `payment_hash`.

Sketch (EVM; mirror for Solana with ed25519 over the same tuple, fetching the tx via `rpc_client().get_transaction(...)` and checking `meta.err.is_none()`):

```rust
// src/handlers.rs, inserted ~handlers.rs:2375 (after is_erc8004_supported, before provider dispatch)
let proof = match request.feedback.proof.as_ref() {
    Some(p) => p,
    None => return bad_request_feedback(network, "proof of payment is required to submit feedback"),
};
if proof.network != network {
    return bad_request_feedback(network, "proof.network must match feedback network");
}
if !proof.verify_payment_hash() {  // new pub method wrapping compute_payment_hash
    return bad_request_feedback(network, "proof.paymentHash does not match proof contents");
}
// EIP-712 binding: recover signer over (agentId, value, valueDecimals, tag1, feedbackIndex)
let recovered = recover_feedback_signer(&request)?;          // new helper
if recovered != proof.payer.as_evm()? {
    return bad_request_feedback(network, "feedback signature does not match proof.payer");
}
// On-chain checks happen inside the EVM branch where `provider` exists:
//   - receipt = provider.inner().get_transaction_receipt(tx).await; reject if None/!status
//   - owner = identity_registry.ownerOf(agent_id_u256).call().await; reject if owner != proof.payee
//   - idempotency: reject if payment_hash already used
```

Apply the **same gate** to `post_revoke_feedback` (handlers.rs:2657-2865): additionally require an EIP-712 signature from the **original submitter** over `(agentId, feedbackIndex)` and track facilitator-relayed submissions keyed to the verified submitter, so a caller can only revoke entries they originated. Without a per-submitter record there is no way to bind revoke to the right party — at minimum, refuse to revoke unless the request carries a proof matching the original `payment_hash` recorded at feedback time.

For `post_register` (handlers.rs:4118+): require a signature from `request.recipient` (the would-be owner) over the registration payload, or gate behind the same `ENABLE_ERC8004_WRITES` flag (Layer C).

### Layer C — feature flag (defense-in-depth, ship immediately)

Add an `ENABLE_ERC8004_WRITES` env gate (default **off** in production until Layer B lands), checked at the top of `post_feedback`/`post_revoke_feedback`/`post_append_response`/`post_register`, returning `403` when disabled. This makes the unauthenticated-forgery surface zero by default while the proof gate is implemented and reviewed.

Why the combined fix closes the hole: after Layer B, the facilitator only signs a `giveFeedback`/`revokeFeedback` when the caller has cryptographically proven (i) control of `proof.payer` (EIP-712 signature recovery) and (ii) a real finalized payment to that specific agent's owner (on-chain receipt + `ownerOf` + recomputed `payment_hash`). Fabrication against arbitrary agents and revocation of others' reviews both become impossible; one-per-`payment_hash` stops flooding; the rate limit + flag cap residual gas exposure.

## Test plan

Rust unit/integration tests (extend `src/erc8004/types.rs::tests` at line 580 and add a handler-level module):

- `src/erc8004/types.rs::tests::test_proof_payment_hash_roundtrip` — build a `ProofOfPayment::new(...)`, assert `verify_payment_hash()` is true, then mutate `amount`/`payer` and assert it becomes false. (Locks the recompute-and-compare logic.)
- `src/erc8004/types.rs::tests::test_feedback_request_requires_proof` — deserialize a `/feedback` body with `proof: None`; assert the new validation helper rejects it.
- New `src/handlers.rs` (or `tests/`) module `erc8004_feedback_auth_tests`:
  - `feedback_rejected_without_proof` — POST with no proof → 400.
  - `feedback_rejected_when_payment_hash_mismatch` — proof with tampered `amount` → 400.
  - `feedback_rejected_when_signature_not_payer` — valid proof but signature from a different key → 400.
  - `feedback_rejected_when_payee_not_agent_owner` — proof where `payee != ownerOf(agentId)` (mock `IIdentityRegistry`) → 400.
  - `feedback_rejected_on_duplicate_payment_hash` — two POSTs with same `payment_hash` → second 409/400.
  - `revoke_rejected_without_submitter_signature` — POST `/feedback/revoke` without a matching submitter signature → 400.
  - `feedback_accepted_with_valid_proof_and_signature` — happy path, signer == payer, payee == owner, receipt confirmed → handler reaches the `giveFeedback` build step (use a mock provider so no real tx is sent).
- `rate_limit_present_on_erc8004_writes` — assert `erc8004_write_routes()` is layered with a `GovernorLayer` in the assembled router (or an integration test that the 6th rapid `/feedback` from one IP returns 429).
- Extend `tests/integration/test_erc8004_feedback.py` with a negative case: a `/feedback` POST without `proof` returns 400, and one with a forged proof (wrong `paymentHash`) returns 400.

No echidna change required — the vulnerable control is off-chain (the facilitator relayer), not the (external) registry contract.

## Rollback notes

- All changes are additive (new validation block, new route group, new env flag, new test module) plus a visibility change (`compute_payment_hash` → `pub`/new `verify_payment_hash`) and a struct field add (`signature` on the feedback envelope).
- To roll back behavior without reverting code: set `ENABLE_ERC8004_WRITES=false` to disable the write endpoints entirely (fail-closed), or remove the `GovernorLayer` from `erc8004_write_routes`.
- If the proof gate breaks a legitimate first-party integration (photo2melee / ExecutionMarket): the safest interim is `ENABLE_ERC8004_WRITES=false` (writes off) rather than reverting to the unauthenticated path — never re-expose the open `giveFeedback`/`revokeFeedback` relayer.
- The route-grouping move (`/feedback*`, `/register` out of `routes()` into `erc8004_write_routes()`) is the only change with merge-conflict potential against the large `handlers::routes()` block; keep it a clean cut/paste of the existing `.route(...)` lines (handlers.rs:118-124) to ease review.

## Verification

Before fix (current `main`, against a test/staging facilitator with a funded ERC-8004 network):

```bash
# Forged feedback succeeds with NO proof and NO signature:
curl -s -X POST $FAC/feedback -H 'content-type: application/json' \
  -d '{"x402Version":1,"network":"base-sepolia","feedback":{"agentId":1,"value":-100,"valueDecimals":0,"tag1":"scam"}}'
# EXPECTED (before): {"success":true,"transaction":"0x..."}  <-- facilitator signed + paid gas

# Bulk revoke succeeds:
curl -s -X POST $FAC/feedback/revoke -H 'content-type: application/json' \
  -d '{"x402Version":1,"network":"base-sepolia","agentId":1,"feedbackIndex":1}'
# EXPECTED (before): {"success":true,"transaction":"0x..."}
```

After fix:

```bash
# 1) No proof -> rejected:
curl -s -o /dev/null -w '%{http_code}\n' -X POST $FAC/feedback \
  -H 'content-type: application/json' \
  -d '{"x402Version":1,"network":"base-sepolia","feedback":{"agentId":1,"value":-100,"valueDecimals":0,"tag1":"scam"}}'
# EXPECTED (after): 400  (body: "proof of payment is required to submit feedback")

# 2) Forged proof (paymentHash mismatch) -> rejected:
#    (construct a proof object with a wrong paymentHash)
# EXPECTED (after): 400  ("proof.paymentHash does not match proof contents")

# 3) Valid proof but signature from a non-payer key -> rejected:
# EXPECTED (after): 400  ("feedback signature does not match proof.payer")

# 4) Rate limit present: 6 rapid POSTs from one IP:
for i in $(seq 1 6); do curl -s -o /dev/null -w '%{http_code} ' -X POST $FAC/feedback \
  -H 'content-type: application/json' -d '{}'; done; echo
# EXPECTED (after): ... 429   (governor kicks in)

# 5) Feature flag off entirely:
#   ENABLE_ERC8004_WRITES=false  =>  every /feedback, /feedback/revoke, /register -> 403
```

Build/lint locally (do not auto-deploy per project policy): `just clippy-all && cargo test -p x402-rs erc8004`. Hand off to the user for `./scripts/fast-build.sh <version> --push` and ECS deploy.

## Residual risk / related findings

- **Revoke binding is structurally limited**: because the external `giveFeedback` records `clientAddress = facilitator` (no per-client field on-chain), perfect attribution requires either an off-chain per-submitter store (added in Layer B step 4) or migrating to a registry variant/wrapper that records the verified payer. Until such a wrapper exists, revoke authorization relies on the facilitator's own submission record — if that store is lost (e.g., in-memory fallback when `IDEMPOTENCY_TABLE_NAME` unset, per recon §6), revoke authorization degrades. Ensure the production DynamoDB store backs the submission record.
- **Solana path** (`post_feedback` Solana branch, handlers.rs:2393-2487; revoke 2709-2779) has the **same** missing-proof flaw and must receive the equivalent ed25519-over-tuple + on-chain-tx (`meta.err.is_none()`) gate. Solana revoke already requires a `sealHash` (handlers.rs:2733) but still no caller proof.
- **`POST /feedback/response`** (handlers.rs:2884) and **`POST /register`** (handlers.rs:4118) share the unauthenticated-relayer shape; both need the flag (Layer C) at minimum and a recipient/responder signature for full closure.
- **Related audit findings** (same root pattern — facilitator signs untrusted body fields without authorization): `erc8004-forgery` (this doc) is the reputation analogue of the escrow-authorization gaps (`payment-operator-escrow-authz`, `blocklist-enforcement-coverage`) and the missing rate limit on gas-spending routes (recon §9, hotspot #7). The `main.rs:361` no-governor fact also amplifies those escrow/register gas-drain vectors; Layer A's route-grouping should be coordinated with the escrow-path fix so all gas-spending writes share a rate-limited router.
