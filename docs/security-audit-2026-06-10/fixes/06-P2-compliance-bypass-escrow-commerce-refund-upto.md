# Compliance/OFAC screening entirely bypassed on escrow, commerce, refund-extension, and upto paths (P2)

> Severity: **P2** (reported P1; verifier-downgraded — the sanctioned party moves its **own** authorized funds, so the harm is OFAC/regulatory liability and defeat of the documented "we screen all payments" posture, not facilitator fund loss, key exposure, or replay/double-settle).
>
> Two independent auditors found this: `payment-operator-escrow-authz` (escrow-only scope) and `blocklist-enforcement-coverage` (escrow + commerce + refund-extension + upto scope). This doc merges both into the broader, correct scope and the single choke-point fix.

---

## Summary

`perform_compliance_screening` (OFAC SDN + `config/blacklist.json`) runs **only** inside `FacilitatorLocal::verify` and `FacilitatorLocal::settle`. But `post_settle` / `post_verify` in `src/handlers.rs` detect the alternate schemes (`upto`, `escrow`, `commerce`) and the `refund` extension **before** the request ever reaches `FacilitatorLocal`, route them to `crate::upto::settle_upto`, `crate::payment_operator::settle_escrow`/`verify_escrow`, and `crate::escrow::settle_with_escrow`, and `return` immediately. Those functions sign and broadcast the on-chain transaction with the facilitator EOA **without any compliance call**. All three gating flags are ON in production (`ENABLE_PAYMENT_OPERATOR=true`, `ENABLE_ESCROW=true`, `ENABLE_UPTO=true`), and `/settle`/`/verify` are unauthenticated. Net effect: an OFAC-/blacklist-listed EVM address moves USDC through the facilitator simply by selecting `scheme=escrow|commerce|upto` or attaching an `extensions.refund` block — completely skipping the screen the standard `exact` path enforces.

The fix hoists screening to **a single choke point that runs before scheme dispatch** in both `post_settle` and `post_verify`, screening the per-scheme payer **and** payee (and, on escrow `release`/`refund`, the client-supplied lifecycle recipient). The broader non-EVM screening no-op gaps (Solana fail-open + NEAR/Stellar/Algorand/Sui/XRPL return `Ok`) are noted as related scope but are NOT the subject of this fix.

---

## Root cause

Screening lives in one place and the alternate schemes never reach it.

**`src/facilitator_local.rs:328`** — the only screening function, private, called only from `verify` (`:107`) and `settle` (`:153`):

```rust
// src/facilitator_local.rs:328
async fn perform_compliance_screening(
    &self,
    payload: &crate::types::ExactPaymentPayload,
    network: crate::network::Network,
) -> Result<(), FacilitatorLocalError> {
    // ... EVM branch screens payer (authorization.from) + payee (authorization.to)
    //     via self.compliance_checker.screen_payment(...) and returns
    //     FacilitatorLocalError::BlockedAddress on Block/Review (-> HTTP 403)
}
```

The checker itself IS active in prod (`src/main.rs:121-137`, `.with_ofac(true).with_blacklist("config/blacklist.json")`, `process::exit(1)` on init failure). So the control exists — it is simply not wired into the alternate dispatch paths.

**`src/handlers.rs` `post_settle` — the four bypass exits** (each parses the body into `json_value` at `:1559`, reads `scheme` at `:1560`, then short-circuits with `return` and NEVER falls through to `FacilitatorLocal::settle` at `:1746+`):

```rust
// src/handlers.rs:1592  (upto)
if scheme == Some("upto") {
    // ... ENABLE_UPTO gate ...
    match crate::upto::settle_upto(body_str, &facilitator).await { ... return ... }   // :1607
}

// src/handlers.rs:1628  (nested v2 escrow/commerce)
if crate::payment_operator::is_escrow_scheme(scheme) {
    // ... ENABLE_PAYMENT_OPERATOR gate ...
    match crate::payment_operator::settle_escrow(body_str, &facilitator).await { ... return ... }  // :1643
}

// src/handlers.rs:1665  (top-level escrow/commerce)
let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());
if crate::payment_operator::is_escrow_scheme(top_level_scheme) {
    match crate::payment_operator::settle_escrow(body_str, &facilitator).await { ... return ... }  // :1681
}

// src/handlers.rs:1706  (refund extension)
if extensions.contains_key("refund") {
    // ... ENABLE_ESCROW gate ...
    match crate::escrow::settle_with_escrow(body_str, &facilitator).await { ... return ... }  // :1722
}
```

`post_verify` mirrors the escrow bypass at `:1106` and `:1144` (both call `verify_escrow` and `return`).

`is_escrow_scheme` (`src/payment_operator/operator.rs:53`) matches BOTH `"escrow"` and `"commerce"`.

**Why the control is absent:** `grep -iE 'compliance|screen_payment|screening|blacklist|ofac|blocked_address'` over `src/payment_operator/`, `src/escrow.rs`, `src/upto/`, and `src/chain/` returns ZERO hits — there is no screening on these paths and no re-screen inside the chain-level provider `settle` either. The downstream signing/broadcast confirms funds actually move with the facilitator key:

```rust
// src/payment_operator/operator.rs execute_authorize_flow / release / refund
let evm_provider = get_evm_provider(facilitator, network)?;   // facilitator's signing provider
let tx_hash = execute_authorize(payload, extra, &addrs, evm_provider).await?;  // signs + broadcasts
```

The on-chain `validOperator` / `conditions` modifiers in `PaymentOperator.sol` are **authorization** controls (who may move a given escrow), not OFAC/sanctions screening, so they do not mitigate this.

---

## Exploit (production config)

Prereqs (all true in prod): `/settle` + `/verify` unauthenticated, `ENABLE_PAYMENT_OPERATOR=true`, `ENABLE_ESCROW=true`, `ENABLE_UPTO=true`, OFAC SDN loaded, `config/blacklist.json` loaded.

1. A sanctioned EVM address (any of the entries in the loaded OFAC list / `config/blacklist.json`) is the payer (and/or the payee). On the standard `exact` path this address is rejected with `403 Address blocked`.
2. Instead of `POST /settle` with `scheme="exact"`, the attacker selects an alternate path:
   - **escrow / commerce:** wrap the EIP-3009 authorization in an `EscrowPayload` and set `scheme="escrow"` (or `"commerce"`). `payer = authorization.from`, `payee = paymentInfo.receiver`.
   - **upto:** build a Permit2 `UptoRequest` with `scheme="upto"`. `payer = permit2Authorization.from`, `payee = accepted.payTo` (== `witness.to`).
   - **refund extension:** attach `paymentPayload.extensions.refund`. `payer = payload.authorization.from`, recipient = the resolved `merchant_payout`.
3. `post_settle` detects the alternate scheme at `:1592 / :1628 / :1665 / :1706`, routes to `settle_upto` / `settle_escrow` / `settle_with_escrow`, and `return`s.
4. `perform_compliance_screening` is never called. The facilitator EOA signs and broadcasts the on-chain tx. The sanctioned funds move.

Variant: the escrow `release`/`refund` lifecycle flows let the **client supply the recipient** (`EscrowLifecyclePayload.payer` / `.payment_info.receiver`), so a clean payer could route the released funds to a sanctioned payout address — which must also be screened.

---

## Fix

Hoist screening to a **single choke point** that runs in `post_settle` and `post_verify` immediately after `json_value` is parsed and `scheme` is detected, **before** the upto/escrow/commerce/refund branches. Extract `(payer, payee)` per scheme and run the same `ComplianceChecker.screen_payment` the EVM exact path uses, returning `403` on `Block`/`Review`. Additionally screen the client-supplied lifecycle recipient on escrow `release`/`refund`.

The handlers receive `State<A>` where `A: Facilitator + HasProviderMap`. To reach the checker without coupling to a concrete type, add a small trait mirroring `HasProviderMap` and expose the checker from `FacilitatorLocal` (which already owns `compliance_checker: Arc<Box<dyn ComplianceChecker>>`).

### Step 1 — expose the checker via a trait

**File:** `src/provider_cache.rs` (next to `HasProviderMap`, `:57`).

```rust
// ADD near HasProviderMap (src/provider_cache.rs)
use std::sync::Arc;
use x402_compliance::ComplianceChecker;

/// Trait for types that expose a compliance checker, so the alternate-scheme
/// dispatch paths in handlers.rs can screen payer/payee before signing.
pub trait HasComplianceChecker {
    fn compliance_checker(&self) -> &Arc<Box<dyn ComplianceChecker>>;
}
```

**File:** `src/facilitator_local.rs` (after the existing `HasProviderMap` impl at `:63-69`).

```rust
// ADD (src/facilitator_local.rs)
use crate::provider_cache::HasComplianceChecker;

impl<A> HasComplianceChecker for FacilitatorLocal<A> {
    fn compliance_checker(&self) -> &Arc<Box<dyn ComplianceChecker>> {
        &self.compliance_checker
    }
}
```

> `FacilitatorLocal.compliance_checker` already exists (`:38`). No struct change needed. `Arc<Arc<...>>` is fine because the State is `Arc<FacilitatorLocal>` and we hand out `&Arc<Box<dyn ...>>`.

### Step 2 — add a free helper that screens by scheme (in `src/handlers.rs`)

Add this helper near the top of `src/handlers.rs`. It parses the per-scheme payer/payee out of the already-parsed `json_value` and screens them. Returns `Ok(())` to proceed, or an `IntoResponse` (`403`) to short-circuit.

```rust
// ADD to src/handlers.rs imports
use crate::provider_cache::HasComplianceChecker;
use x402_compliance::{ScreeningDecision, TransactionContext};

/// Screen payer + payee for an alternate-scheme (escrow/commerce/upto/refund)
/// request BEFORE any on-chain signing. EVM-only schemes; non-EVM alt-schemes
/// (none today) would be added here. Returns Err(Response) on Block/Review.
async fn screen_alt_scheme<C: HasComplianceChecker>(
    facilitator: &C,
    json_value: &serde_json::Value,
    scheme: Option<&str>,
    top_level_scheme: Option<&str>,
) -> Result<(), axum::response::Response> {
    // Collect (label, addr) pairs to screen for this request.
    let mut addrs: Vec<(&str, String)> = Vec::new();

    let pp = json_value.get("paymentPayload");
    let payload = pp.and_then(|p| p.get("payload"));

    // upto: payer = payload.permit2Authorization.from ; payee = paymentRequirements.payTo
    if scheme == Some("upto") {
        if let Some(from) = payload
            .and_then(|p| p.get("permit2Authorization"))
            .and_then(|a| a.get("from"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("payer", from.to_string()));
        }
        if let Some(to) = json_value
            .get("paymentRequirements")
            .and_then(|r| r.get("payTo"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("payee", to.to_string()));
        }
    }

    // escrow / commerce (nested OR top-level): authorize uses authorization.from +
    // paymentInfo.receiver ; release/refund uses lifecycle payer + paymentInfo.receiver.
    if crate::payment_operator::is_escrow_scheme(scheme)
        || crate::payment_operator::is_escrow_scheme(top_level_scheme)
    {
        if let Some(from) = payload
            .and_then(|p| p.get("authorization"))
            .and_then(|a| a.get("from"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("payer", from.to_string()));
        }
        // lifecycle (release/refund) carries payer at payload.payer
        if let Some(payer) = payload.and_then(|p| p.get("payer")).and_then(|v| v.as_str()) {
            addrs.push(("payer", payer.to_string()));
        }
        if let Some(recv) = payload
            .and_then(|p| p.get("paymentInfo"))
            .and_then(|pi| pi.get("receiver"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("payee", recv.to_string()));
        }
        // also screen the fee receiver if present
        if let Some(fee) = payload
            .and_then(|p| p.get("paymentInfo"))
            .and_then(|pi| pi.get("feeReceiver"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("feeReceiver", fee.to_string()));
        }
    }

    // refund extension: authorization.from + paymentRequirements.payTo (merchant payout)
    let has_refund = pp
        .and_then(|p| p.get("extensions"))
        .and_then(|e| e.as_object())
        .map(|o| o.contains_key("refund"))
        .unwrap_or(false);
    if has_refund {
        if let Some(from) = payload
            .and_then(|p| p.get("authorization"))
            .and_then(|a| a.get("from"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("payer", from.to_string()));
        }
        if let Some(to) = json_value
            .get("paymentRequirements")
            .and_then(|r| r.get("payTo"))
            .and_then(|v| v.as_str())
        {
            addrs.push(("payee", to.to_string()));
        }
    }

    if addrs.is_empty() {
        return Ok(()); // nothing alt-scheme to screen; standard path screens normally
    }

    let checker = facilitator.compliance_checker();
    let context = TransactionContext {
        amount: "unknown".to_string(),
        currency: "USDC".to_string(),
        network: "evm-alt-scheme".to_string(),
        transaction_id: None,
    };

    for (label, addr) in addrs {
        let decision = match checker.screen_address(&addr).await {
            Ok(d) => d,
            Err(e) => {
                let id = uuid::Uuid::new_v4();
                tracing::error!(%id, error = %e, %addr, "alt-scheme screening errored; failing closed");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": format!("compliance_unavailable (ref: {id})")})),
                )
                    .into_response());
            }
        };
        match decision {
            ScreeningDecision::Block { reason } | ScreeningDecision::Review { reason } => {
                tracing::warn!(%addr, %label, %reason, "Alt-scheme payment blocked by compliance");
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(json!({
                        "success": false,
                        "errorReason": format!("Address blocked: {}", reason)
                    })),
                )
                    .into_response());
            }
            ScreeningDecision::Clear => {}
        }
    }
    // keep `context` used for parity with screen_payment audit path if you later
    // switch to screen_payment(payer, payee, &context).
    let _ = context;
    Ok(())
}
```

> `screen_address` (`crates/x402-compliance/src/checker.rs:22`) screens a single address against all loaded lists and returns `ScreeningDecision` — ideal for the variable-arity set (payer, payee, lifecycle recipient, fee receiver). Alloy `Address`/string fields here are already `0x...` hex (camelCase JSON keys: `permit2Authorization`, `payTo`, `paymentInfo`, `feeReceiver`, `extensions.refund`), matching the wire format the schemes deserialize from. It **fails closed** on checker error (`503`), matching the EVM exact path's posture of refusing rather than allowing.

### Step 3 — call the choke point before dispatch in `post_settle`

**File:** `src/handlers.rs`, inside `post_settle`, in the `if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(body_str)` block, AFTER `scheme` is computed (`:1560-1567`) and AFTER the `top_level_scheme` is available. The simplest is to compute `top_level_scheme` once at the top of the block and screen immediately:

```rust
// src/handlers.rs  — inside post_settle, right after `let scheme = ...;` (:1567)
let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());

// CHOKE POINT: screen alternate-scheme payer/payee BEFORE any dispatch/signing.
if let Err(resp) = screen_alt_scheme(&facilitator, &json_value, scheme, top_level_scheme).await {
    return resp;
}
```

Then change the existing top-level escrow branch at `:1664` from declaring its own `top_level_scheme` to reusing the one above (delete the duplicate `let top_level_scheme = ...` at `:1664`). The `fhe-transfer` branch (`:1569`) is unaffected — it forwards to a remote facilitator and is out of scope.

### Step 4 — mirror in `post_verify`

**File:** `src/handlers.rs`, inside `post_verify`, in its `if let Ok(json_value) = ...` block, after `scheme` is computed and before the escrow branches at `:1089`/`:1127`. Add the same `top_level_scheme` + `screen_alt_scheme` call (verify is read-only but MUST mirror so a blocked address never even gets a "valid" preflight, and so `verify` and `settle` stay consistent).

```rust
// src/handlers.rs — inside post_verify, before the is_escrow_scheme branches (:1089)
let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());
if let Err(resp) = screen_alt_scheme(&facilitator, &json_value, scheme, top_level_scheme).await {
    return resp;
}
```

Delete the duplicate `let top_level_scheme = ...` at `:1126` and reuse this one.

### Step 5 — add the bound to the handlers

`post_settle` and `post_verify` already bound `A: Facilitator + HasProviderMap`. Add `+ HasComplianceChecker`:

```rust
// src/handlers.rs post_settle (:1378-1382) and post_verify signature
where
    A: Facilitator + HasProviderMap + HasComplianceChecker,
    // ...
```

### Why this closes the hole

The screen now runs on the SAME request bytes that drive scheme dispatch, BEFORE any branch can call `settle_upto` / `settle_escrow` / `settle_with_escrow` (which sign). A `Block`/`Review` on any of payer, payee, lifecycle recipient, or fee receiver short-circuits with `403` and the facilitator never signs. Because the choke point is in the handler (not in `FacilitatorLocal`), it covers every alternate path uniformly and cannot be re-bypassed by adding a new scheme branch later, as long as new branches sit after the choke point. The standard `exact` path keeps its existing in-`FacilitatorLocal` screening unchanged (no behavior change, no double-screen for `exact` since `screen_alt_scheme` returns early when no alt-scheme fields are present).

---

## Test plan

### Rust unit tests — `src/handlers.rs` (new `#[cfg(test)] mod alt_scheme_screening_tests`)

Use a stub `ComplianceChecker` whose `screen_address` returns `Block` for a fixed sanctioned address and `Clear` otherwise, wired into a stub State that implements `HasComplianceChecker` (and trivially `HasProviderMap` / `Facilitator` if needed). Call `screen_alt_scheme` directly against representative JSON bodies:

- `test_escrow_authorize_blocked_payer` — `scheme="escrow"`, `payload.authorization.from` = sanctioned → `Err(403)`.
- `test_escrow_authorize_blocked_receiver` — clean payer, `payload.paymentInfo.receiver` = sanctioned → `Err(403)`.
- `test_commerce_scheme_blocked` — same as escrow but `scheme="commerce"` (covers the `is_escrow_scheme` alias).
- `test_escrow_release_blocked_lifecycle_payer` — release shape (`payload.payer` sanctioned, no `authorization`) → `Err(403)`.
- `test_escrow_fee_receiver_blocked` — `payload.paymentInfo.feeReceiver` = sanctioned → `Err(403)`.
- `test_upto_blocked_payer` — `scheme="upto"`, `payload.permit2Authorization.from` = sanctioned → `Err(403)`.
- `test_upto_blocked_payee` — clean payer, `paymentRequirements.payTo` = sanctioned → `Err(403)`.
- `test_refund_extension_blocked` — `paymentPayload.extensions.refund` present, `authorization.from` sanctioned → `Err(403)`.
- `test_clean_alt_scheme_passes` — all addresses clean → `Ok(())`.
- `test_exact_scheme_is_noop` — `scheme="exact"`, body has no alt-scheme fields → `Ok(())` (no screening here; the standard path screens it).
- `test_checker_error_fails_closed` — stub `screen_address` returns `Err` → `Err(503)`.

### Rust integration / regression — extend existing EVM-exact screening test

Mirror whatever test asserts the EVM `exact` path returns `403` for a blacklisted `authorization.from` (in `crates/x402-compliance` tests or `tests/`) by adding parameterized cases for `scheme=escrow`, `scheme=commerce`, `scheme=upto`, and a `refund` extension, asserting all return `403` / `errorReason: "Address blocked..."`.

### Integration (`tests/integration/`)

Add a Python case to `test_facilitator.py` (or a new `test_compliance_alt_schemes.py`) that POSTs `/settle` with a blacklisted address under each of `scheme=escrow|commerce|upto` and a `refund` extension against `base-sepolia`, asserting HTTP `403`. Use an address present in `config/blacklist.json` so no real OFAC entity is referenced in test fixtures.

---

## Rollback notes

- Pure additive change: one new trait (`HasComplianceChecker`), one impl on `FacilitatorLocal`, one free helper (`screen_alt_scheme`), two call sites, one new `where` bound. No struct fields added, no migration, no config change, no contract change.
- To roll back, revert the `screen_alt_scheme` calls in `post_settle`/`post_verify` and the `+ HasComplianceChecker` bound; the trait/impl can stay (dead but harmless).
- Risk of over-blocking: the helper screens additional addresses (fee receiver, lifecycle recipient) the exact path does not. If a legitimate alt-scheme flow is falsely blocked, the immediate mitigation is to remove the offending entry from `config/blacklist.json` (the OFAC list is authoritative and should not be relaxed). The fail-closed `503` on checker error matches existing posture; if a transient checker outage causes alt-scheme `503`s, that is the same behavior the exact path already exhibits indirectly.
- Performance: `screen_address` is in-memory list lookup; the extra calls (2-4 per alt-scheme request) are negligible and only hit alt-scheme traffic (a small fraction of `/settle`).

---

## Verification

Build/clippy locally (do NOT auto-deploy — user deploys manually):

```bash
just clippy-all
cargo test -p x402-rs alt_scheme_screening
```

Run the facilitator locally with the flags ON and a known blacklisted address in `config/blacklist.json`:

```bash
ENABLE_PAYMENT_OPERATOR=true ENABLE_ESCROW=true ENABLE_UPTO=true cargo run --release
```

**Before the fix (vulnerable):** POST `/settle` with `scheme="escrow"` (or `commerce`/`upto`, or a `refund` extension) where `authorization.from` is a blacklisted address → the request is routed to `settle_escrow`/`settle_upto`/`settle_with_escrow` and proceeds to signing (succeeds, or fails for unrelated on-chain reasons), **never** `403`.

**After the fix:** the same request returns:

```
HTTP/1.1 403 Forbidden
{"success": false, "errorReason": "Address blocked: ..."}
```

while the identical request with all-clean addresses still routes to the alt-scheme handler unchanged. Confirm the standard `exact` path is unchanged: a blacklisted `exact` payment still returns `403 {"error":"Address blocked: ..."}` from `FacilitatorLocalError::BlockedAddress` (`src/handlers.rs:2178`), and a clean `exact` payment still settles.

Production smoke (post-deploy, prod URL): `curl` `/settle` with a `config/blacklist.json` address under `scheme=escrow` and confirm `403`. There is no public endpoint that reveals OFAC entries, so use a benign blacklist test entry, not a real SDN address, in any externally visible test.

---

## Residual risk / related findings

- **In-scope but not fixed here — broader non-EVM screening no-op (recon §7).** Even on the standard `exact` path, `perform_compliance_screening` (`src/facilitator_local.rs:386-532`) is a **fail-open** for Solana when address extraction fails (`:444-455`, explicitly "ALLOWING transaction temporarily") and a **no-op `Ok`** for NEAR (`:466`), Stellar (`:484`), Algorand (`:501`), Sui (`:509`), SolanaSettlementAccount (`:517`), and XRPL (`:525`). A sanctioned address on those chains is never screened even on the normal path. This fix only closes the EVM alternate-scheme bypass; the non-EVM gaps need their own per-chain extractors and a flip of the Solana branch to fail-closed. Track separately.
- **Screening lists are OFAC-only.** UN/UK/EU lists are TODO (`crates/x402-compliance/src/checker.rs:171`), and `config/blacklist.json` is a small operator-maintained file. Coverage is only as good as those lists; this fix does not change which entities are listed, only that the alt-scheme paths consult them.
- **Authorization vs. screening are orthogonal.** The on-chain `validOperator`/`conditions` modifiers still govern *who can move* a given escrow; this fix adds the *sanctions* gate the off-chain path was missing. They are complementary, not substitutes.
- **`screen_address` vs `screen_payment` audit logging.** This fix uses `screen_address` per-address (cleaner for the variable address set). If audit-log parity with the exact path (which uses `screen_payment(payer, payee, &context)`) is required for compliance records, switch the escrow/upto/refund payer+payee pair to a single `screen_payment` call and keep `screen_address` only for the extra lifecycle/fee recipients; the `TransactionContext` is already constructed in the helper for that purpose.
