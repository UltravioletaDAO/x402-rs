# Solana settlement-account path forges payment success: referenced on-chain tx is never bound to `pay_to`, and the no-sweep branches return `success:true` without moving funds to the merchant (P1)

> Security audit 2026-06-10 — finding 03. Verifier-confirmed P1 (`control-hunt` lens). Component: `src/chain/solana.rs` `verify_settlement_account` / `settle_settlement_account` / `sweep_settlement_account`; supporting: `src/facilitator_local.rs` compliance no-op; new env gate.

## Summary

The Solana "settlement account" (Crossmint custodial) settle path accepts an arbitrary, publicly-observable on-chain USDC transfer signature and reports `success:true` to the merchant **without ever binding that transfer to the merchant's `pay_to` address**. `verify_settlement_account` only proves that *some* ATA of the required mint was credited at least the required amount; it never checks that the recipient is `pay_to`. Two branches in `settle_settlement_account`/`sweep_settlement_account` then return `success:true` (with a real-but-unrelated transaction hash) while **transferring zero USDC to the merchant**: (a) when `settleSecretKey` is `None`, and (b) when the settlement ATA balance is `0`. A merchant trusting the `/settle` verdict ships goods for free. Reachable in production: the Docker image builds `--features solana,...` and production provisions a Solana keypair + RPC, so the dispatch routes any `SolanaSettlementAccount` payload straight into this path; there is **no `ENABLE_SETTLEMENT_ACCOUNT` gate** and compliance screening is an explicit no-op for this payload type.

## Root cause

### 1. `verify_settlement_account` never checks `owner == pay_to`

`src/chain/solana.rs:1457-1540`. The credit is summed across **every** post-token-balance whose `mint == requirements.asset`, with no constraint on the credited account's `owner`. The dev comment admits this and (incorrectly) defers the binding to the sweep:

```rust
// src/chain/solana.rs:1457-1462 (verbatim)
// B1 NOTE: The Crossmint settlement-account model deposits USDC into a settlement ATA
// (owned by the settlement keypair), NOT directly into pay_to's ATA.  A strict owner==pay_to
// check would reject all legitimate Crossmint payments.  Instead, verification confirms
// that SOME ATA of the correct mint received at least required_amount.  The binding to
// pay_to is enforced in sweep_settlement_account, which hardcodes the transfer destination
// to pay_to_ata and caps the amount at required_amount.
```

```rust
// src/chain/solana.rs:1476-1500 — credit loop: ONLY filters by mint, never owner
for post_bal in post_balances {
    if post_bal.mint != asset_str { continue; }
    let post_amount: u64 = post_bal.ui_token_amount.amount.parse().unwrap_or(0);
    let pre_amount: u64 = /* matching pre-balance */ ;
    let diff = post_amount.saturating_sub(pre_amount);
    if diff > 0 {
        total_credit += diff;           // <-- credited regardless of who owns the ATA
        // owner = ?post_bal.owner is only DEBUG-LOGGED, never compared to pay_to
    }
    // ...
}
```

```rust
// src/chain/solana.rs:1535-1539 — the ONLY amount gate; no pay_to gate
if total_credit < required_amount {
    return Err(FacilitatorLocalError::DecodingError(format!(
        "settlement account transfer amount {} < required {}",
        total_credit, required_amount
    )));
}
```

Note the `owner` field is *already available* on each `post_token_balances` entry (it is read at `:1497` and `:1504` as `post_bal.owner`, an `OptionSerializer<String>` from the JsonParsed encoding), so the owner==pay_to check needs **no extra RPC call**.

### 2. The promised binding (the sweep) is skipped — two `success:true` branches that move no funds

The comment says binding "is enforced in `sweep_settlement_account`", but the sweep does not always run.

**Branch (a) — `settleSecretKey == None`** (`src/chain/solana.rs:1601-1626`):

```rust
// Step 2: If settleSecretKey is provided, sweep funds from settlement account to payTo.
if let Some(ref secret_key_str) = payload.settle_secret_key {
    return self.sweep_settlement_account(secret_key_str, payload, requirements, &verification).await;
}

// No secret key: funds already at payTo, return original tx signature.   <-- FALSE ASSUMPTION
// ...
Ok(SettleResponse {
    success: true,                                            // <-- forged success
    error_reason: None,
    payer: verification.payer.clone().into(),
    transaction: Some(TransactionHash::Solana(*verification.tx_signature.as_array())), // unrelated tx
    network: self.network(),
    proof_of_payment: None,
    extensions: None,
})
```

The comment "funds already at payTo" is never verified — `verification` only proved *some* ATA was credited, not `pay_to`'s ATA. So with `settleSecretKey: null` the facilitator returns success with no sweep and no binding.

**Branch (b) — empty settlement ATA** (`src/chain/solana.rs:1717-1734`), reached when a (possibly attacker-supplied) `settleSecretKey` decodes to a keypair whose ATA holds `0`:

```rust
if on_chain_balance == 0 {
    // No balance to sweep - funds went directly to payTo   <-- ALSO UNVERIFIED
    return Ok(SettleResponse {
        success: true,                                       // <-- forged success
        error_reason: None,
        payer: verification.payer.clone().into(),
        transaction: Some(TransactionHash::Solana(*verification.tx_signature.as_array())),
        network: self.network(),
        proof_of_payment: None,
        extensions: None,
    });
}
```

### 3. No control elsewhere blocks this

- **Replay/nonce** keys on `solana-settle#{network}#{tx_signature}` (`src/chain/solana.rs:76-78`), so a third party's transfer is first-use and passes `check_and_mark_used` (`:1582`).
- **`is_supported_asset`** (`src/chain/solana.rs:1955`, `src/network.rs`) matches only the *mint*, never `pay_to`; passes for USDC.
- **Compliance** is an explicit no-op for this payload type (`src/facilitator_local.rs:517-524`): returns `Ok(())` with a `// will verify on-chain transaction` comment — but the on-chain verify never screens anyone.
- **No `ENABLE_SETTLEMENT_ACCOUNT` gate** exists anywhere; the dispatch at `src/chain/solana.rs:1964-1969` routes any `ExactPaymentPayload::SolanaSettlementAccount` straight into `settle_settlement_account`.

## Exploit (production config)

Production builds `--features solana,...` (`Dockerfile:15`) and provisions a Solana mainnet keypair + RPC, so the Solana provider initializes and the settlement-account path is live and unauthenticated.

1. Attacker observes **any** confirmed Solana mainnet USDC transfer of `>=` the resource price on a block explorer — call its signature `SIG`. It can be a totally unrelated payment between two strangers.
2. Attacker `POST /settle` with:
   - `payment_requirements`: `{ network: "solana", scheme: "exact", asset: <USDC mint>, payTo: <merchant>, maxAmountRequired: <price> }`
   - `payment_payload.payload`: `SolanaSettlementAccount { transactionSignature: SIG, settleSecretKey: null }`
3. Replay/nonce check passes (`SIG` first use).
4. `verify_settlement_account` fetches `SIG`, sums USDC credits across all ATAs in that tx; `total_credit >= required` → `Ok` (**`pay_to` never checked**).
5. `settle_settlement_account` sees `settleSecretKey == None` → returns `success:true`, `transaction = SIG`.
6. Facilitator reports a successful payment with a real on-chain hash; the merchant ships goods; the merchant received **0 USDC**.

Variant: supply a freshly generated, empty settlement keypair's `settleSecretKey` instead of `null` to land in the `on_chain_balance == 0` branch (b) for the same outcome.

Blast radius: every merchant accepting the Solana settlement-account scheme and trusting the facilitator verdict without independent on-chain confirmation. Severity P1 (not P0): no facilitator hot-key/fund drain — the harm is counterparty (merchant) fund loss / settlement forgery.

## Fix

Three changes, all in-repo, no protocol change. Implement in this order.

### Fix A — bind the credited ATA to `pay_to` inside `verify_settlement_account`

**File:** `src/chain/solana.rs`, function `verify_settlement_account` (`:1349-1558`), credit loop region `:1473-1540`.

**A.1** Resolve `pay_to` and its ATA once, before the credit loop. Insert immediately **after** `let asset_str = asset_pubkey.to_string();` (`:1455`) and before the `B1 NOTE` comment block:

```rust
// FIX 03: bind the credited ATA to the merchant's pay_to address.
let pay_to: Pubkey = match &requirements.pay_to {
    MixedAddress::Solana(pk) => *pk,
    _ => {
        return Err(FacilitatorLocalError::InvalidAddress(
            "expected Solana payTo address".to_string(),
        ))
    }
};
let token_program = spl_token::id();
let (pay_to_ata, _) = Pubkey::find_program_address(
    &[pay_to.as_ref(), token_program.as_ref(), asset_pubkey.as_ref()],
    &ATA_PROGRAM_PUBKEY,
);
let pay_to_str = pay_to.to_string();
let pay_to_ata_str = pay_to_ata.to_string();
```

**A.2** Replace the unconditional `total_credit += diff;` accumulation (`:1492-1500`) so that **only credits landing in `pay_to`'s ATA of the required mint count**. A post-token-balance entry binds to `pay_to` if either its `owner == pay_to` (the wallet owns the credited ATA) or the credited account's pubkey equals the derived `pay_to_ata`. The JsonParsed entry exposes `owner` (already used) and, on this encoding, an account address resolvable from `meta`/`account_keys`. The robust, RPC-free check is on `owner`:

Before (`:1491-1500`):
```rust
let diff = post_amount.saturating_sub(pre_amount);
if diff > 0 {
    total_credit += diff;
    tracing::debug!(
        account_index = post_bal.account_index,
        credit = diff,
        owner = ?post_bal.owner,
        "Found USDC credit in settlement transaction"
    );
}
```

After:
```rust
let diff = post_amount.saturating_sub(pre_amount);
if diff > 0 {
    // FIX 03: count the credit ONLY if it landed in pay_to's ATA of the
    // required mint. The owner field is present on JsonParsed token balances.
    let credited_to_pay_to = matches!(
        post_bal.owner,
        OptionSerializer::Some(ref o) if *o == pay_to_str
    );
    if credited_to_pay_to {
        total_credit += diff;
        tracing::debug!(
            account_index = post_bal.account_index,
            credit = diff,
            owner = ?post_bal.owner,
            "Found USDC credit to pay_to in settlement transaction"
        );
    } else {
        tracing::debug!(
            account_index = post_bal.account_index,
            credit = diff,
            owner = ?post_bal.owner,
            "Ignoring USDC credit to non-pay_to ATA in settlement transaction"
        );
    }
}
```

The existing gate at `:1535` (`if total_credit < required_amount { return Err(...) }`) now means "at least `required_amount` reached `pay_to`'s ATA", because `total_credit` only accumulates `pay_to`-owned credits. If no credit lands at `pay_to`, `total_credit == 0 < required_amount` → hard error. **The `pay_to_ata` variable is referenced in the error/log path** (silences `unused` if you choose owner-only matching); if `clippy` flags `pay_to_ata`/`pay_to_ata_str` as unused, either log it in the rejection branch or remove the derivation and keep the `owner` comparison only.

**Why this closes it:** the attacker's cited `SIG` credits the *real* recipients' ATAs, never the merchant's `pay_to` ATA, so `total_credit` stays `0` and verify returns an error before any `success:true` can be produced.

### Fix B — make the no-sweep branches hard-error instead of forging success

**File:** `src/chain/solana.rs`, `settle_settlement_account` (`:1569-1626`) and `sweep_settlement_account` (`:1717-1734`).

After Fix A, the only legitimate way `required_amount` reaches `pay_to` is a sweep from the settlement ATA into `pay_to_ata`, OR a transfer that already landed *directly* in `pay_to`'s ATA (now provably checked by Fix A). Tighten the two success-forging branches:

**B.1 — `settleSecretKey == None` branch** (`:1608-1625`). Fix A already guarantees `required_amount` reached `pay_to`'s ATA on-chain, so returning success here is now *safe only when the referenced tx itself credited `pay_to`*. Keep the success return, but make the assumption explicit and provable. Replace the comment "No secret key: funds already at payTo" with an assertion-style guard. Since `verify_settlement_account` (post-Fix-A) already failed unless `>= required_amount` reached `pay_to`, no code change is strictly required for correctness here — but to defend against future regressions add a redundant guard documenting the invariant:

Before (`:1608-1625`):
```rust
// No secret key: funds already at payTo, return original tx signature.
tracing::info!( /* ... */ "Settlement account: no sweep needed (no settleSecretKey), returning original tx" );
Ok(SettleResponse { success: true, /* ... */ })
```

After:
```rust
// No settleSecretKey: the referenced transaction itself must have credited
// pay_to's ATA. verify_settlement_account (Fix 03) already enforced that
// total_credit-to-pay_to >= required_amount before we reach this point, so
// returning the original tx signature is sound. This branch is NO LONGER a
// blind "trust the client" path.
tracing::info!(
    network = %self.network(),
    tx_signature = %verification.tx_signature,
    "Settlement account: no sweep needed, on-chain credit to pay_to already verified"
);
Ok(SettleResponse { success: true, /* unchanged fields */ })
```

**B.2 — empty settlement ATA branch** (`:1717-1734`). When a `settleSecretKey` IS supplied, the contract is "sweep from the settlement ATA into `pay_to`". An `on_chain_balance == 0` here means there is nothing to sweep. This is only safe if Fix A already proved the funds reached `pay_to` directly; if a `settleSecretKey` was provided AND the settlement ATA is empty AND the referenced tx did not credit `pay_to`, the request is a forgery. Because Fix A makes `verify_settlement_account` fail in the latter case, `on_chain_balance == 0` here is reachable only when the funds already reached `pay_to`. To make this explicit and to remove the false comment, change branch (b) to a hard error UNLESS this is genuinely a "direct-to-pay_to, secret key redundant" case — which, after Fix A, is exactly what `verification` proved. Safest implementation: hard-error, because if a `settleSecretKey` was supplied the caller asserted funds are in the settlement account, not at `pay_to`:

Before (`:1717-1734`):
```rust
if on_chain_balance == 0 {
    // No balance to sweep - funds went directly to payTo
    tracing::info!(/* ... */ "Settlement account ATA has 0 balance, no sweep needed");
    return Ok(SettleResponse { success: true, /* ... */ });
}
```

After:
```rust
if on_chain_balance == 0 {
    // FIX 03: a settleSecretKey was supplied, which asserts funds are in the
    // settlement account awaiting a sweep. An empty settlement ATA means there
    // is nothing to move to pay_to. Returning success here previously forged a
    // payment. Hard-error instead; the only sound "no sweep" path is the
    // settleSecretKey==None branch, which is guarded by Fix A on-chain.
    return Err(FacilitatorLocalError::ContractCall(
        "settlement account ATA is empty: no funds to sweep to pay_to".to_string(),
    ));
}
```

**Why this closes it:** the empty-ATA variant of the exploit (supply a fresh empty keypair) now returns an error instead of `success:true`.

### Fix C — `ENABLE_SETTLEMENT_ACCOUNT` opt-in gate (defense-in-depth)

**File:** `src/chain/solana.rs`. Mirror the `is_escrow_enabled()` pattern at `src/escrow.rs:271-276`.

**C.1** Add a free function near the top of `src/chain/solana.rs` (next to `solana_settle_nonce_key`, `:76`):

```rust
/// Settlement-account (Crossmint custodial) scheme is opt-in and OFF by default.
/// Set ENABLE_SETTLEMENT_ACCOUNT=true to allow SolanaSettlementAccount payloads.
pub fn is_settlement_account_enabled() -> bool {
    std::env::var("ENABLE_SETTLEMENT_ACCOUNT")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}
```

**C.2** Gate both dispatch sites. In `verify` (`:1937-1945`) and `settle` (`:1963-1970`), before routing a `SolanaSettlementAccount` payload, reject when the gate is off:

```rust
if let ExactPaymentPayload::SolanaSettlementAccount(sa_payload) =
    &request.payment_payload.payload
{
    if !is_settlement_account_enabled() {
        return Err(FacilitatorLocalError::Other(
            "settlement_account_disabled: set ENABLE_SETTLEMENT_ACCOUNT=true to enable".to_string(),
        ));
    }
    // ... existing routing ...
}
```

**C.3 (infra, do NOT change here — note for the deploy engineer):** the Terraform task definition does **not** currently set `ENABLE_SETTLEMENT_ACCOUNT`. After this fix the path is OFF in prod by default. If the Crossmint integration is actually in use, the operator must add `ENABLE_SETTLEMENT_ACCOUNT=true` to `terraform/environments/production/main.tf` env vars and redeploy — and only after Fix A/B are live. This is a separate Terraform change outside this doc's scope.

### Fix D — extend compliance screening for the settlement-account payload

**File:** `src/facilitator_local.rs:517-524`. Today this returns `Ok(())` with a comment promising "will verify on-chain transaction" — but no screening ever runs. Replace the no-op with real screening of the recovered payer and `pay_to`:

Before (`:517-524`):
```rust
ExactPaymentPayload::SolanaSettlementAccount(_sa_payload) => {
    // Settlement account payloads contain an already-submitted transaction signature.
    // Compliance screening will be done when verifying the on-chain transaction.
    tracing::debug!("Settlement account compliance check: will verify on-chain transaction");
    Ok(())
}
```

After (screen `requirements.pay_to`, and screen the payer once recovered in verify). The payer is not known at screening time (it is recovered from the on-chain tx inside `verify_settlement_account`). Two options:

- **Minimal:** screen `requirements.pay_to` here (the destination is known up front), and add a payer screen inside `verify_settlement_account` after `payer_pubkey` is resolved (`src/chain/solana.rs:1542-1544`), calling the same `ComplianceChecker` used elsewhere. Return `Block` as an error.
- **Preferred:** thread the `ComplianceChecker` into `SolanaProvider` and screen both `payer` and `pay_to` at the end of `verify_settlement_account` before returning `Ok(SettlementAccountVerifyResult { .. })`.

Use the same checker invocation as the EVM/Solana-exact paths (`facilitator_local.rs:336-385` / `:444-455`); on `ScreeningDecision::Block`/`Review` return a 403-mapped error. Document that this path is now screened in the no-op's place.

## Test plan

Add to the `#[cfg(test)] mod tests` block in `src/chain/solana.rs` (`:2268+`), alongside the existing `test_balance_delta_*` and `test_sweep_cap_*` unit tests.

1. **`test_settlement_credit_to_pay_to_counts`** — pure-logic test of the Fix A predicate: given a post-token-balance entry whose `owner == pay_to_str`, the credit accumulates and `total_credit >= required_amount` passes. Mirror the arithmetic style of `test_balance_delta_exact_match` (`:2418`).

2. **`test_settlement_credit_to_non_pay_to_rejected`** — the core regression: a post-token-balance entry whose `owner != pay_to_str` (the attacker's "borrowed" `SIG` scenario) must contribute `0` to `total_credit`, so the `total_credit < required_amount` gate fires and verify errors. Assert the error is the `"settlement account transfer amount 0 < required"` `DecodingError`.

3. **`test_settlement_no_secret_key_requires_pay_to_credit`** — assert that the `settleSecretKey == None` branch can only return `success:true` after a `pay_to`-bound credit (i.e. that branch is unreachable when Fix A errored). Where full mocking of RPC is impractical, factor the credit/binding decision into a small pure helper (e.g. `fn settlement_credit_to_pay_to(post_balances, pre_balances, asset_str, pay_to_str) -> u64`) and unit-test that helper directly — this also keeps the `verify_settlement_account` body testable.

4. **`test_settlement_empty_ata_hard_errors`** — assert the `on_chain_balance == 0` branch now returns `Err(FacilitatorLocalError::ContractCall(...))` ("settlement account ATA is empty") rather than `Ok(SettleResponse { success: true, .. })`. Drive via the extracted branch logic if direct invocation needs RPC.

5. **`test_is_settlement_account_enabled`** — mirror `test_is_escrow_enabled` (`src/escrow.rs:915-938`): unset → `false`; `"true"`/`"TRUE"`/`"1"` → `true`; `"false"`/`"0"` → `false`. Clean up the env var at the end (the existing escrow test does `env::remove_var` last; do the same to avoid cross-test contamination, and keep this test serialized if other tests read the same var).

6. **Integration (optional, `tests/crossmint-smart-wallet/`):** extend `test.mjs` with a negative case that submits a confirmed-but-unrelated USDC transfer signature (not paying `pay_to`) and asserts `/settle` returns a failure, plus a positive case with a real Crossmint settlement that still succeeds (regression guard so the fix does not break legitimate Crossmint flows).

Run: `cargo test -p x402-rs --features solana chain::solana::tests` (and `cargo test -p x402-rs escrow::tests::test_is_settlement_account_enabled` style for the gate test if relocated). `just clippy-all` to catch any unused `pay_to_ata` binding.

## Rollback notes

- Pure source changes; no schema/DB/contract migration. Revert is a clean `git revert` of the commit.
- **Behavioral break for legitimate Crossmint flows:** Fix A now *requires* the on-chain credit to reach `pay_to`'s ATA. If real Crossmint settlements deposit into an intermediate settlement ATA and rely on the *sweep* (with `settleSecretKey`) to move funds to `pay_to`, those flows must still pass — they do, because the sweep path is untouched and Fix A's verify still succeeds as long as the *eventual* `pay_to` credit can be proven. **Before merge, confirm against a real Crossmint settlement transaction** (use `tests/crossmint-smart-wallet/`) that the post-token-balances actually show a credit owned by `pay_to`; if the legitimate flow credits only the settlement ATA (owner == settlement keypair) and never `pay_to` in the *referenced* tx, then verify must instead accept the settlement-ATA credit AND require a `settleSecretKey` (so the sweep, which is hardcoded to `pay_to_ata`, performs the binding) — in that case relax Fix A to: "credit to `pay_to` OR (credit to settlement ATA AND `settleSecretKey` present)", and keep Fix B.1's `None`-branch hard-error.
- **Prod default flips to OFF** via Fix C. If Crossmint is in active use, the rollback of Fix C (or setting `ENABLE_SETTLEMENT_ACCOUNT=true`) is required to restore the path — but never roll back Fix A/B without re-introducing the forgery.

## Verification

**Before (vulnerable):**
```bash
# Pick any confirmed, unrelated Solana mainnet USDC transfer signature SIG (e.g. from Solscan)
curl -s -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H 'content-type: application/json' \
  -d '{
    "x402Version":1,
    "paymentRequirements":{"network":"solana","scheme":"exact",
      "asset":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      "payTo":"<MERCHANT_PUBKEY>","maxAmountRequired":"1000000"},
    "paymentPayload":{"x402Version":1,"scheme":"exact","network":"solana",
      "payload":{"transactionSignature":"<SIG>"}}
  }'
# VULNERABLE RESPONSE: {"success":true,"transaction":"<SIG>", ...}  <-- forged
```

**After (fixed):**
- With `ENABLE_SETTLEMENT_ACCOUNT` unset/false (default): `{"success":false, ... "settlement_account_disabled ..."}` (Fix C).
- With `ENABLE_SETTLEMENT_ACCOUNT=true` but `SIG` not crediting `payTo`: `{"success":false, ... "settlement account transfer amount 0 < required 1000000"}` (Fix A).
- Empty-keypair variant (`settleSecretKey` for a fresh empty account): `{"success":false, ... "settlement account ATA is empty: no funds to sweep to pay_to"}` (Fix B.2).
- A genuine Crossmint settlement that does credit/sweep to `payTo`: still `{"success":true, ...}` (regression-preserved).

Local: `cargo test -p x402-rs --features solana` (new tests green) and `just clippy-all`. Compliance: confirm `perform_compliance_screening` now screens `pay_to`/payer for this payload type (Fix D) by checking that a blacklisted `payTo` from `config/blacklist.json` is rejected on `/settle` with a settlement-account body.

> Per CLAUDE.md, do **not** auto-build or auto-deploy — hand the patch to the user for `./scripts/fast-build.sh <ver> --push` and ECS rollout.

## Residual risk / related findings

- **`payer` recovery is heuristic.** `verify_settlement_account` infers the payer from the debited ATA's `owner` (`:1502-1533`) and falls back to the facilitator's own pubkey when it cannot (`:1542-1544`). After Fix D this affects which address gets compliance-screened as payer; the `pay_to` screen (always known) is the stronger guarantee.
- **JsonParsed `owner` availability.** The fix relies on `post_token_balances[].owner` being populated on `UiTransactionEncoding::JsonParsed` (it is for SPL token balances on `confirmed`). If a future RPC/encoding change drops `owner`, the `OptionSerializer::None` case must be treated as *not* `pay_to` (the `matches!` guard already does this — a missing owner contributes `0`, failing closed).
- **PDA migration (preferred long-term).** The dev comment at `:1470-1472` and `:55-63` recommends moving to a facilitator-controlled PDA derived from `(payer, pay_to, nonce)`, eliminating `settleSecretKey` entirely. That removes branch (a)/(b) and the borrowed-`settleSecretKey` class of issues. Track as a follow-up.
- **Related audit findings (same compliance-bypass theme):** finding 01 (escrow path bypasses OFAC/blacklist screening) and the "compliance entirely bypassed on escrow/commerce/refund/upto" finding share the root cause that screening lives only inside `FacilitatorLocal::verify/settle` and several alternate paths skip it. Fix D here closes the settlement-account no-op; those findings close the escrow/upto no-ops. The Solana *standard* settle path also has a sibling P1 (reports success for a confirmed-but-`meta.err` transaction — `send_and_confirm` checks commitment, not `meta.err`); that is a separate fix in `TransactionInt::send_and_confirm`.
