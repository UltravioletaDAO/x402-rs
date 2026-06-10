# Algorand: settled payment's recipient and amount are never bound to the payment requirements (P1)

## Summary
On the Algorand settlement path the facilitator validates the client-signed ASA transfer's *structure* (group shape, fee-tx safety, ASA == USDC, validity window, group-id replay) but **never compares the transfer's `receiver` to `requirements.pay_to` nor its `amount` to `requirements.max_amount_required`**. `AlgorandProvider::verify` / `settle` (`src/chain/algorand.rs:862-953`) call `verify_payment_group(payload)` and never touch `request.payment_requirements` at all. As a result the facilitator returns `valid` / `success:true` for *any* USDC ASA transfer the client signed — including a 1-microUSDC self-transfer to an attacker address against a 10-USDC priced resource — so any merchant that trusts the facilitator response delivers goods unpaid (theft-of-goods / payment-confirmation forgery). Algorand is a live production mainnet built into the prod image, and `/settle` is unauthenticated.

## Root cause
`AlgorandProvider::verify` and `settle` consume only `request.payment_payload`, never `request.payment_requirements` (which *is* present on the request struct — `SettleRequest = VerifyRequest`, `src/types.rs:1444,1465`):

`src/chain/algorand.rs:862-885` (verify) — only network is checked, then straight to `verify_payment_group`:
```rust
async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
    let payload = &request.payment_payload;
    if payload.network != self.network() { /* NetworkMismatch */ }
    match &payload.payload {
        ExactPaymentPayload::Algorand(p) => {
            let verification = self
                .verify_payment_group(p)                // <-- requirements NEVER passed
                .await
                .map_err(FacilitatorLocalError::from)?;
            Ok(VerifyResponse::valid(verification.payer.into()))
        }
        _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
    }
}
```

`settle` (`src/chain/algorand.rs:887-953`) is identical in this respect: it calls `verify_payment_group(algorand_payload)` (line 904) and then `submit_group`, returning `SettleResponse { success: true, payer: verification.payer, ... }` (lines 941-949). `verification.recipient` and `verification.amount` are used **only for an `info!` log line** (lines 908-913, populated at `:822-824`).

`verify_payment_group` (`src/chain/algorand.rs:481-596`) extracts the four fields the spec is supposed to bind, then checks everything *except* recipient and amount:
```rust
// src/chain/algorand.rs:543-563
let (asset_id, amount, receiver, sender) = match &payment_signed.transaction.txn_type {
    TransactionType::AssetTransferTransaction(xfer) => (
        xfer.xfer,
        xfer.amount,
        xfer.receiver.clone(),
        xfer.sender.clone(),
    ),
    _ => { return Err(AlgorandError::InvalidAtomicGroup("Payment must be an asset transfer".to_string())); }
};

// Verify it's USDC
if asset_id != self.chain.usdc_asa_id {            // <-- ONLY asset gate
    return Err(AlgorandError::AsaIdMismatch { expected: self.chain.usdc_asa_id, actual: asset_id });
}
// ... validity-window check on last_valid round (576) ...
// NO comparison of `receiver` to pay_to, NO comparison of `amount` to max_amount_required,
// NO requirements.network / scheme check.
```

Why the control is absent: the function signature is `verify_payment_group(&self, payload: &ExactAlgorandPayload)` — `requirements` is simply not in scope. The on-chain `amount`/`receiver` are read from the *client-signed* ASA transfer, so they are entirely attacker-controlled, and nothing downstream re-checks them. The only invariant guarding against abuse is `validate_fee_transaction` (`:413-478`), which forces the fee tx to be FROM the facilitator and caps the fee at `MAX_ALGORAND_FEE_TX_MICROALGOS` (0.1 ALGO) — this protects the **treasury** but does nothing for the merchant's recipient/amount. There is also a *defined-but-unused* `lease` replay/bind hook: `verify_payment_group:524-540` only `tracing::debug!`/`warn!`s on the lease field (the GoPlausible spec says `lease == SHA256(paymentRequirements)`), and the `AlgorandError::MissingLease` / `LeaseMismatch` variants (`:120,123`) are **never returned**.

Diverging precedent in the same codebase confirms this is an omission, not by design:
- **NEAR** (`src/chain/near.rs:610-675`) binds `requirements.network`, `payload.scheme == requirements.scheme`, `requirements.asset`, `receiver_id == pay_to` (via `validate_delegate_actions`, the "B3" invariant), and `amount == max_amount_required.to_string()` (`:669`).
- **Stellar** (`src/chain/stellar.rs`) binds `args[1] == pay_to` and `args[2] == max_amount_required` (the "B4" checks).
- **Algorand alone** omits all of these.

Compliance screening does not save this path either: `perform_compliance_screening` is an explicit no-op for Algorand (`src/facilitator_local.rs:501-508`, "allow … TODO").

## Exploit
Production config: `ENABLE_*` payment features on, `/settle` open/no-auth, Algorand mainnet (`Network::Algorand`, USDC ASA `31566704`) in the prod image (`Dockerfile:15` builds `--features …,algorand`).

1. Attacker browses a protected resource priced at, e.g., **10 USDC** on `network: "algorand"`; the server returns `PaymentRequirements { pay_to = <merchant base32 addr>, max_amount_required = 10_000000, asset = USDC, scheme = exact, network = algorand }`.
2. Attacker builds an Algorand atomic group `[unsigned_fee_tx(sender = facilitator), signed_asa_transfer]` where the **ASA transfer is a self-transfer of 1 microUSDC** (`xfer = 31566704`, `amount = 1`, `receiver = attacker_addr`, `sender = attacker_addr`). The attacker signs only the ASA transfer; sets a matching group id; fee tx is FROM the facilitator with a sane fee (so `validate_fee_transaction` passes).
3. Attacker `POST /settle` with `paymentRequirements` echoing the merchant's `pay_to`/`max_amount_required`, and `ExactAlgorandPayload { payment_index: 1, payment_group: [feeTxB64, signedAsaTransferB64] }`.
4. `verify_payment_group` passes: group ≥ 2 txs ✓, fee-tx safety ✓, ASA == USDC `31566704` ✓ (it *is* USDC, just 1 micro-unit), validity window ✓, group-id replay ✓. It **never** compares `receiver`(attacker) to `pay_to`(merchant) or `amount`(1) to `max_amount_required`(10_000000).
5. `submit_group` co-signs the ~0.001 ALGO fee tx, broadcasts the group, waits for confirmation, and `settle` returns `success: true`, `payer = attacker`, `transaction = <real tx id>`.
6. The merchant — trusting the facilitator's `success:true` — releases the 10-USDC resource. The attacker paid $0.000001 to a wallet they control. Repeat with a fresh group id each time.

Blast radius: every merchant using the Algorand path; unauthenticated; no treasury drain (fee capped at 0.1 ALGO) — loss falls on merchants.

## Fix
Bind the signed transfer to the payment requirements inside `verify_payment_group`, mirroring NEAR (`near.rs:610-675`) and Stellar. Touch only `src/chain/algorand.rs`.

### Step 1 — thread `requirements` into `verify_payment_group`
**Function:** `verify_payment_group` — change its signature (`src/chain/algorand.rs:481-484`).

Before:
```rust
async fn verify_payment_group(
    &self,
    payload: &ExactAlgorandPayload,
) -> Result<VerifyGroupResult, AlgorandError> {
```
After:
```rust
async fn verify_payment_group(
    &self,
    payload: &ExactAlgorandPayload,
    requirements: &crate::types::PaymentRequirements,
    payment_scheme: crate::types::Scheme,
) -> Result<VerifyGroupResult, AlgorandError> {
```
(Add `PaymentRequirements` and `Scheme` to the existing `use crate::types::{…}` import at `src/chain/algorand.rs:37-41` — `Scheme` and `MixedAddress` are already imported; add `PaymentRequirements`.)

### Step 2 — add the binding checks after field extraction
**Location:** immediately after the existing USDC `asset_id` gate (`src/chain/algorand.rs:558-563`), before the validity-window block (`:565`). Insert:

```rust
// ---- BIND TO PAYMENT REQUIREMENTS (mirrors near.rs:610-675 / stellar B4) ----

// (a) network must match the network this provider serves
if requirements.network != self.chain.network {
    return Err(AlgorandError::InvalidAtomicGroup(format!(
        "requirements.network {} does not match provider network {}",
        requirements.network, self.chain.network
    )));
}

// (b) scheme must be exact and match requirements
if payment_scheme != Scheme::Exact || requirements.scheme != Scheme::Exact {
    return Err(AlgorandError::InvalidAtomicGroup(format!(
        "unsupported scheme: payload={payment_scheme:?}, requirements={:?}",
        requirements.scheme
    )));
}

// (c) recipient: signed transfer receiver MUST equal requirements.pay_to
let expected_pay_to = match &requirements.pay_to {
    MixedAddress::Algorand(addr) => addr.clone(),
    other => {
        return Err(AlgorandError::InvalidAtomicGroup(format!(
            "pay_to must be an Algorand address, got {other:?}"
        )));
    }
};
let actual_receiver = receiver.to_string();
if actual_receiver != expected_pay_to {
    tracing::warn!(
        expected = %expected_pay_to,
        actual = %actual_receiver,
        "Algorand payment receiver does not match pay_to -- rejecting"
    );
    return Err(AlgorandError::InvalidAtomicGroup(format!(
        "payment receiver {actual_receiver} does not match required pay_to {expected_pay_to}"
    )));
}

// (d) amount: signed transfer amount MUST equal requirements.max_amount_required.
// On-chain `amount` is u64; max_amount_required is TokenAmount(U256). Compare in U256.
let expected_amount = requirements.max_amount_required.0; // alloy U256
let actual_amount = alloy::primitives::U256::from(amount); // u64 -> U256, no overflow
if actual_amount != expected_amount {
    tracing::warn!(
        expected = %expected_amount,
        actual = %actual_amount,
        "Algorand payment amount does not match max_amount_required -- rejecting"
    );
    return Err(AlgorandError::InvalidAtomicGroup(format!(
        "payment amount {actual_amount} does not match required {expected_amount}"
    )));
}
```

> Notes:
> - `MixedAddress` and `Scheme` are already in scope; `requirements.max_amount_required.0` is `U256` (`src/types.rs:701`). If `alloy::primitives::U256` is not already imported in this file, fully-qualify it as written above (or add `use alloy::primitives::U256;` and use `U256::from(amount)`).
> - `receiver.to_string()` yields the canonical 58-char base32 string (same format `pay_to` deserializes into via `MixedAddress::Algorand`, `src/types.rs:1184-1186`), so the string compare is correct. Both are case-sensitive base32 with a checksum, so an exact `==` is safe.
> - Using `AlgorandError::InvalidAtomicGroup` keeps the change minimal (it maps to `FacilitatorLocalError::Other` via the existing `From`, `:133-137`). Optionally add dedicated variants `ReceiverMismatch { expected, actual }` and `AmountMismatch { expected, actual }` to `AlgorandError` (`:73-131`) for clearer error reasons — not required for correctness.

### Step 3 — pass requirements at both call sites
**`verify`** (`src/chain/algorand.rs:877-880`):
```rust
let verification = self
    .verify_payment_group(p, &request.payment_requirements, request.payment_payload.scheme)
    .await
    .map_err(FacilitatorLocalError::from)?;
```
**`settle`** (`src/chain/algorand.rs:903-906`):
```rust
let verification = self
    .verify_payment_group(algorand_payload, &request.payment_requirements, request.payment_payload.scheme)
    .await
    .map_err(FacilitatorLocalError::from)?;
```
(`request.payment_payload.scheme` is `Scheme` on `PaymentPayload`, `src/types.rs:665`.)

### Step 4 (optional, recommended) — cross-check requirements.asset against the ASA id
The hard ASA gate already requires `asset_id == self.chain.usdc_asa_id` (`:558`). To honor per-asset requirements (and not silently accept a USDC transfer when the merchant required a different ASA), additionally parse `requirements.asset` to a numeric ASA id and require equality. Because Algorand ASA ids arrive on the wire as a numeric string, they deserialize as `MixedAddress::Offchain(_)` (the numeric string is not a 58-char base32 address, `src/types.rs:1184-1203`), so match it leniently:
```rust
// (e) optional: if requirements.asset carries a numeric ASA id, it must match
if let MixedAddress::Offchain(asset_str) = &requirements.asset {
    if let Ok(req_asa) = asset_str.parse::<u64>() {
        if req_asa != asset_id {
            return Err(AlgorandError::AsaIdMismatch { expected: req_asa, actual: asset_id });
        }
    }
}
```
This is defense-in-depth; the `usdc_asa_id` gate is the primary control.

### Step 5 (optional hardening) — enforce the lease bind (close the spec replay+bind gap)
The GoPlausible x402-avm spec mandates `lease == SHA256(canonical paymentRequirements)`. The code already inspects the lease but only logs (`:524-540`) and never returns `AlgorandError::MissingLease` (`:120`). To enforce, replace the `match &payment_signed.transaction.lease { … warn … }` block with a hard check:
```rust
let expected_lease = sha2::Sha256::digest(canonical_requirements_bytes(requirements)); // 32 bytes
match &payment_signed.transaction.lease {
    Some(lease) if lease.0 == expected_lease.as_slice() => { /* ok */ }
    Some(lease) => {
        return Err(AlgorandError::LeaseMismatch {
            expected: BASE64.encode(expected_lease),
            actual: BASE64.encode(&lease.0),
        });
    }
    None => return Err(AlgorandError::MissingLease),
}
```
This requires agreeing a canonical byte serialization of `PaymentRequirements` with the client SDK; ship it behind a flag or after SDK rollout to avoid breaking existing clients. **Do not block the P1 fix on this** — Steps 1-3 close the exploit; the lease bind is additional replay/binding hardening.

### Why this closes the hole
After Steps 1-3 the facilitator rejects any group whose signed ASA transfer does not pay `requirements.max_amount_required` to `requirements.pay_to` on the correct network with the `exact` scheme. The exploit's 1-microUSDC self-transfer fails check (c) (receiver ≠ pay_to) AND check (d) (amount ≠ max_amount_required), so `verify` returns invalid and `settle` errors before `submit_group` — no broadcast, no false `success:true`. The recipient/amount are now read from the signed tx and *compared to the merchant's requirements*, exactly as on EVM (`assert_valid_payment`) and NEAR/Stellar.

## Test plan
Add Rust unit tests in `src/chain/algorand.rs` (the file already has a `#[cfg(test)] mod tests` for NEAR-style; if none exists in algorand.rs, add one). Build helpers that construct an `ExactAlgorandPayload` (base64 msgpack `[unsigned_fee_tx(sender=facilitator), signed_asa_transfer]`) and a `PaymentRequirements`. Where on-chain `status()` would be needed, factor the pure validation (group decode + the new binding checks) into a synchronous helper so tests don't hit algod, or gate the network call behind the binding checks (move checks (a)-(e) above the `self.algod.status()` call at `:566`).

- `algorand_wrong_recipient_is_rejected` — receiver = attacker, pay_to = merchant, amount correct → expect `Err` whose message contains `pay_to` / "receiver". (Mirrors `near.rs::wrong_receiver_id_is_rejected`.)
- `algorand_under_amount_is_rejected` — receiver correct, amount = 1, max_amount_required = 10_000000 → expect `Err` containing "amount". (Mirrors `near.rs::wrong_amount_is_rejected`.)
- `algorand_over_amount_is_rejected` — amount = 20_000000 vs required 10_000000 → expect `Err` (strict equality, not `>=`).
- `algorand_exact_match_passes` — receiver == pay_to AND amount == max_amount_required AND ASA == USDC → binding checks return Ok (stub/skip the algod round-trip).
- `algorand_wrong_network_requirements_rejected` — requirements.network = `Base` while provider is `Algorand` → expect `Err` from check (a).
- `algorand_non_algorand_pay_to_rejected` — requirements.pay_to = `MixedAddress::Evm(_)` → expect `Err` from check (c).
- `algorand_wrong_asa_in_requirements_rejected` (if Step 4 added) — requirements.asset = `"10458941"` (testnet ASA) on mainnet provider → expect `AsaIdMismatch`.
- `algorand_lease_mismatch_rejected` / `algorand_missing_lease_rejected` (only if Step 5 shipped).

Also add/extend an integration test under `tests/` that POSTs a wrong-recipient and an under-amount Algorand `/settle` body and asserts a non-success response, alongside the existing non-EVM binding tests.

## Rollback notes
- The change is confined to `src/chain/algorand.rs` (signature of one private method + two call sites + an inserted validation block, plus one import). No schema/wire changes, no DB/state changes, no Terraform/infra changes.
- To roll back: revert `verify_payment_group`'s signature to `(&self, payload: &ExactAlgorandPayload)`, drop the inserted checks, and restore the two call sites to `verify_payment_group(p)` / `verify_payment_group(algorand_payload)`.
- **Client-compat risk:** legitimate Algorand clients must already sign a transfer that pays the *required* amount to the *required* recipient. If any existing client was relying on the loose behavior (e.g., sending a different amount/recipient), it will now be rejected — that is the intended correctness fix, but coordinate with the Algorand SDK path before deploy. Step 5 (lease) is the only sub-change with real client-rollout coupling; keep it out of the hotfix unless the SDK already sets `lease = SHA256(requirements)`.

## Verification
Pre-fix (current prod) — reproduce the forgery against a testnet/staging facilitator:
1. `POST /settle` with `paymentRequirements.payTo = <merchant>`, `maxAmountRequired = "10000000"`, and an `ExactAlgorandPayload` whose signed ASA transfer is `amount = 1` to `receiver = <attacker>`.
   Expected (vulnerable): `{"success":true,"payer":"<attacker>","transaction":"<id>", ...}` — the under-paid/wrong-recipient transfer is confirmed.
2. Same against `POST /verify` — expected (vulnerable): `{"isValid":true, "payer":"<attacker>"}`.

Post-fix:
1. The under-amount / wrong-recipient `POST /settle` returns `success:false` (or HTTP error) with an error reason referencing receiver/amount mismatch; **no group is broadcast** (confirm no new tx for that group id on `https://allo.info`).
2. `POST /verify` on the same body returns `isValid:false`.
3. A *correct* payload — signed ASA transfer of exactly `maxAmountRequired` USDC to `payTo` — still returns `success:true` and the tx confirms on-chain (regression: don't break the happy path).
4. `cargo test -p x402-rs --features algorand chain::algorand` passes the new tests; `cargo clippy --features algorand` is clean.

Curl skeleton (staging):
```bash
curl -sS -X POST "$FACILITATOR/settle" -H 'content-type: application/json' \
  -d @algorand_underpay.json | jq '{success, payer, transaction, errorReason}'
# expect post-fix: {"success": false, "errorReason": ...}, no transaction
```

## Residual risk / related findings
- **Compliance still bypassed on Algorand:** even after this fix, `perform_compliance_screening` is a no-op for Algorand (`src/facilitator_local.rs:501-508`). A sanctioned payer/recipient can still settle a *correctly-bound* payment. Tracked by the separate "non-EVM no-op screening" / "blocklist-enforcement-coverage" findings — fix together for full coverage.
- **Replay across restart:** Algorand replay defense is group-id tracking in the nonce store; if `NONCE_STORE_TABLE_NAME` is unset the store is in-memory and replay reopens on restart (prod sets it). Enforcing the lease bind (Step 5) adds a second, requirements-bound replay defense.
- **Sibling P1 findings (same class — settled tx not bound to requirements):** Solana settlement-account path (recipient never bound to `pay_to`), Solana standard settle (confirmed-but-failed reported as success), Sui (`validate_ptb` never checks coin is USDC), Stellar (`from` forced to facilitator). Apply the same "bind every settled field to the signed payload AND to requirements" discipline across all non-EVM providers.
