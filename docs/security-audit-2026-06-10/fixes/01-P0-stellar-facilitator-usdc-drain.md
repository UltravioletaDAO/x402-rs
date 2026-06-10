# Stellar auth-entry inversion + SourceAccount bypass: repeatable drain of facilitator mainnet USDC (P0)

> Security audit 2026-06-10 — finding 01 — **P0 (confirmed by both adversarial lenses: control-hunt and exploit-repro)**
> Component: `src/chain/stellar.rs` (`validate_soroban_auth_entry` Check 5a + `verify_authorization_signature` SourceAccount branch)
> Status: **LIVE AND EXPLOITABLE IN PRODUCTION RIGHT NOW.** `stellar:pubnet` is in `/supported`; the mainnet wallet `GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB` holds ~2.0025 USDC + ~18.39 XLM. Do the **IMMEDIATE CONTAINMENT** section first, then ship the code fix.

---

## Summary

The Stellar payment path validates the Soroban authorization entry so that the transfer's `from` argument (`args[0]`) **must equal the facilitator's own public key** instead of the payer's, and it **accepts `SorobanCredentials::SourceAccount` with no signature check at all**. Because the facilitator builds the settlement transaction with *itself* as the transaction source account and signs it, on-chain `require_auth(from=facilitator)` is satisfied by the facilitator's own signature — so an unauthenticated `POST /settle` makes the facilitator sign and broadcast a transfer of **its own USDC** to an attacker-chosen `pay_to`. Recipient (`args[1]`) and amount (`args[2]`) are attacker-controlled and validated only to match the attacker's own `paymentRequirements`, and the off-chain nonce key is attacker-controlled, so the attack is repeatable until the facilitator's Stellar balance is empty. (As a side effect the same inversion rejects every *legitimate* payer with `InvalidSender`, which is supporting evidence that this path shipped never exercised end-to-end — but that does **not** reduce drain severity.)

---

## IMMEDIATE CONTAINMENT (do this NOW, before the code fix)

The blast radius is the live balance of the facilitator's Stellar mainnet wallet. Shrink it to zero and close the door before touching code. None of these steps require a rebuild.

### 1. Sweep the facilitator Stellar mainnet wallet to a safe address

Move both the USDC and the XLM out of `GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB` so an attacker who fires the exploit before the code fix lands drains nothing.

- Destination: a cold/treasury address you control that is **not** a facilitator hot wallet. Leave a minimal XLM base reserve (~1.5 XLM for one trustline) on the facilitator account so it stays valid; sweep everything above that.
- The signing key is in AWS Secrets Manager (`facilitator-stellar-keypair-mainnet`, injected as `STELLAR_PRIVATE_KEY_MAINNET`). Use it **only** from a secure operator machine (never paste it into any file, log, or chat).
- USDC issuer/SAC to sweep: canonical Circle USDC on pubnet (SAC `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75`, classic issuer `GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN`).
- Suggested path: Stellar Laboratory or `stellar-cli` building a classic `payment` op for the USDC asset and a second `payment` op for native XLM, signed once with the mainnet key, submitted to Horizon `https://horizon.stellar.org`.

Verify the sweep landed (public, no key needed):

```bash
curl -s "https://horizon.stellar.org/accounts/GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB" \
  | jq '.balances'
# Expect: USDC balance ~0 and native XLM down to the base reserve.
```

### 2. Take the Stellar mainnet settle path out of service until the fix ships

The drain requires the Stellar **mainnet provider to be loaded with a hot key** and `/settle` to dispatch to it. Kill either one. Pick the fastest lever you can apply without a code change:

- **Preferred — remove the mainnet Stellar key from the running task.** In `terraform/environments/production` remove/blank the `STELLAR_PRIVATE_KEY_MAINNET` secret injection (`secrets.tf`) and `terraform apply`, then `aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2`. With no mainnet Stellar key, the Stellar mainnet provider fails to initialize / cannot sign, so the drain transaction can never be built. (Stellar testnet is unaffected and harmless.)
- **Alternative — block Stellar at the edge.** If you cannot redeploy immediately, add a Caddy/ALB/WAF rule that rejects `POST /settle` and `POST /verify` request bodies whose `network` is `stellar`/`stellar:pubnet`/`stellar-pubnet`. This is coarser (depends on body inspection) and is a stopgap only; the key-removal lever is authoritative.

After containment, confirm the wallet is empty and that a drain attempt now fails (see **Verification**). Then implement the code fix below and only re-fund + re-enable Stellar mainnet once the fix is deployed and the regression tests pass.

---

## Root cause

All citations are `src/chain/stellar.rs`. Two independent defects on the same path compound into a self-drain.

### Defect A — `args[0]` (`from`) is pinned to the FACILITATOR, not the payer

`validate_soroban_auth_entry`, Check 5a (`stellar.rs:1046-1071`) derives the facilitator's own key from `self.public_key` and rejects the entry unless the transfer's `from` argument equals it:

```rust
// --- Check 5a: args[0] = ScVal::Address(ScAddress::Account) matching facilitator ---
let facilitator_bytes = StellarPublicKey::from_string(&self.public_key)  // <-- facilitator's OWN key
    .map_err(/* ... */)?
    .0;
match &invoke_args.args[0] {
    ScVal::Address(ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
        Uint256(key_bytes),
    )))) => {
        if *key_bytes != facilitator_bytes {            // <-- INVERTED: requires from == facilitator
            // ...
            return Err(StellarError::InvalidSender { /* expected: facilitator, ... */ });
        }
    }
    // ...
}
```

The doc comment at `stellar.rs:943` even codifies the wrong model: `args has exactly 3 elements: [from: facilitator, to: pay_to, amount: ...]`. The intended x402 Stellar design (`docs/STELLAR_IMPLEMENTATION_PLAN.md:81-90` and the TS SDK `providers/stellar/index.ts` `transfer(from=user,...)`) requires `args[0]` to be the **payer**. The control is inverted: it forces the facilitator to be the source of funds, and rejects real payers.

This inversion flows straight into transaction construction. `build_unsigned_transaction` (`stellar.rs:1360-1384`) attaches the client's auth entry verbatim (`auth = vec![verification.auth_entry.clone()]`, `stellar.rs:1361`) and sets the **transaction source account to the facilitator**:

```rust
let facilitator_bytes = StellarPublicKey::from_string(&self.public_key)?.0;
let source_account = MuxedAccount::Ed25519(Uint256(facilitator_bytes));  // tx source = facilitator
```

`build_signed_envelope` (`stellar.rs:1418-1428`) then signs the transaction hash with the facilitator's `signing_key`. On-chain, the USDC SAC `transfer(from, to, amount)` calls `from.require_auth()`; with `from == facilitator` and the transaction source account being the facilitator, the require_auth is satisfied by the facilitator's own transaction signature.

### Defect B — `SourceAccount` credentials skip signature verification entirely

`verify_authorization_signature` (`stellar.rs:626-633`) returns `Ok(())` for `SourceAccount` credentials with no check:

```rust
let credentials = match &auth_entry.credentials {
    SorobanCredentials::Address(addr_creds) => addr_creds,
    SorobanCredentials::SourceAccount => {
        // Source account credentials don't need signature verification here
        return Ok(());                       // <-- no signature is ever checked
    }
};
```

`validate_soroban_auth_entry` never inspects `auth_entry.credentials` at all (it only reads `root_invocation`), so a `SourceAccount` entry sails through both validation and signature verification. Combined with Defect A, the only invocation the facilitator will accept is `transfer(from=facilitator, to=attacker, amount=attacker)` — authorized by the facilitator's own transaction-source signature, with **no payer signature required anywhere**.

### Why the other controls do not save you

- `/settle` is unauthenticated by design (recon §1); safety is supposed to come from signed-payload validation, which is exactly what is broken here.
- Compliance screening for Stellar is an explicit **no-op** (`facilitator_local.rs:484-491`: "allow Stellar transactions through ... TODO: implement Stellar compliance"), so the OFAC/blacklist choke point never runs on this path.
- The off-chain nonce key is `stellar_nonce_key(chain, stellar_payload.from, stellar_payload.nonce)` (`check_nonce_unused` call at `stellar.rs:1290`), both client-controlled. `SourceAccount` carries no on-chain Soroban nonce, so a fresh `payload.nonce` per request evades the off-chain store and nothing is consumed on-chain → **repeatable until the wallet is empty**.
- Simulation does not block it — the facilitator holds the USDC, so the transfer simulates and executes successfully.

---

## Exploit (concrete, production config)

Prereqs all hold in prod: `stellar:pubnet` in `/supported`, `STELLAR_PRIVATE_KEY_MAINNET` injected (`terraform secrets.tf:184-186`), Dockerfile builds `--features ...,stellar`, facilitator wallet funded.

1. Read the facilitator's Stellar mainnet USDC balance publicly (Horizon `/accounts/GCHPGXJT.../balances`). Call it `B`.
2. Build a `SorobanAuthorizationEntry`:
   - `credentials = SorobanCredentials::SourceAccount`
   - `root_invocation = ContractFn { contract = USDC SAC (CCW67TSZ...), function = "transfer", args = [ Address(Account(facilitator G-addr)), Address(Account(attacker G-addr)), I128(B) ] }`
   - `sub_invocations = []`
   - XDR-encode it and base64 it.
3. `POST /settle` with:
   - `paymentRequirements = { network: "stellar", scheme: "exact", asset: USDC, pay_to: <attacker G-addr>, max_amount_required: B }`
   - `paymentPayload.payload = ExactStellarPayload { from: <any valid G-addr>, to: <attacker>, amount: B, token_contract: USDC SAC, authorization_entry_xdr: <base64 from step 2>, nonce: <random fresh u64>, signature_expiration_ledger: <current_ledger + 1000> }`
4. Server-side: `validate_soroban_auth_entry` passes (args[0]=facilitator ✓ via Defect A, args[1]=pay_to=attacker ✓, args[2]=B ✓); `verify_authorization_signature` returns `Ok` (Defect B, `SourceAccount` branch); `check_nonce_unused` passes (fresh nonce).
5. The facilitator builds the tx with itself as source (`stellar.rs:1380-1384`), signs it (`stellar.rs:1428`), simulates (succeeds — it holds the USDC), and submits. On-chain `require_auth(facilitator)` is satisfied by the source-account signature; **`B` USDC moves facilitator → attacker.**
6. Repeat with a fresh `nonce` until the wallet is drained.

Attacker cost: one unauthenticated HTTP request per drain (subject only to the ~30 req/min per-IP `/settle` rate budget). No payer key, no funded attacker account, no on-chain setup.

---

## Fix

All edits are in `src/chain/stellar.rs`. The goal: (1) reject `SourceAccount` credentials on the payment path, (2) bind `args[0]` to the **payer**, never the facilitator, and (3) add a belt-and-suspenders guard that the payer is never the facilitator. The existing `args[1]`/`args[2]` recipient/amount checks (Check 5b/5c, `stellar.rs:1098-1187`) are correct and stay.

Because `validate_soroban_auth_entry` is the function with all the args plumbing but does **not** currently receive the payer, thread the payer in. `verify_payment` already has `stellar_payload.from` and validates it to a `StellarAddress` (`stellar.rs:1253`); pass it down.

### Change 1 — reject `SourceAccount` in `verify_authorization_signature` (`stellar.rs:626-633`)

**Before:**

```rust
let credentials = match &auth_entry.credentials {
    SorobanCredentials::Address(addr_creds) => addr_creds,
    SorobanCredentials::SourceAccount => {
        // Source account credentials don't need signature verification here
        return Ok(());
    }
};
```

**After:**

```rust
let credentials = match &auth_entry.credentials {
    SorobanCredentials::Address(addr_creds) => addr_creds,
    SorobanCredentials::SourceAccount => {
        // SECURITY (audit 01): SourceAccount credentials carry NO payer signature.
        // On the payment path the tx source account is the facilitator, so accepting
        // SourceAccount would let the facilitator's own signature authorize the
        // transfer's `from` -- a self-drain. The x402 Stellar spec mandates
        // SorobanAddressCredentials (payer-signed). Reject hard.
        tracing::warn!(
            "Authorization entry uses SourceAccount credentials on payment path - rejecting"
        );
        return Err(StellarError::UnsupportedCredentialType);
    }
};
```

`StellarError::UnsupportedCredentialType` already exists (`stellar.rs:99-100`).

### Change 2 — bind the SorobanAddressCredentials.address to the declared payer

Still inside `verify_authorization_signature`, after the `Address(addr_creds)` match arm has bound `credentials`, assert the credential's address equals the `expected_address` the caller passed (`stellar_payload.from`). Add immediately after the `match` (before the signature-format match at `stellar.rs:639`):

```rust
// SECURITY (audit 01): the credential address (the account whose signature
// authorizes this transfer) MUST be the declared payer, not some other account.
let cred_addr_str = match &credentials.address {
    ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(b)))) => {
        StellarPublicKey(*b).to_string()
    }
    other => {
        tracing::warn!(?other, "auth credential address is not an ed25519 account - rejecting");
        return Err(StellarError::InvalidSignature { address: expected_address.to_string() });
    }
};
if cred_addr_str != expected_address {
    tracing::warn!(
        credential_address = %cred_addr_str,
        expected = %expected_address,
        "auth credential address does not match declared payer - rejecting"
    );
    return Err(StellarError::InvalidSignature { address: expected_address.to_string() });
}
```

This ties the signed credential, the signature subject, and the on-chain `require_auth` target to the same payer.

### Change 3 — invert Check 5a to require `args[0] == payer` and reject `args[0] == facilitator`

Thread the payer into `validate_soroban_auth_entry`. Update the signature (`stellar.rs:948-953`):

**Before:**

```rust
fn validate_soroban_auth_entry(
    &self,
    auth_entry: &SorobanAuthorizationEntry,
    expected_pay_to: &str,
    expected_amount: TokenAmount,
) -> Result<(), StellarError> {
```

**After:**

```rust
fn validate_soroban_auth_entry(
    &self,
    auth_entry: &SorobanAuthorizationEntry,
    expected_from: &str,   // SECURITY (audit 01): the PAYER (stellar_payload.from)
    expected_pay_to: &str,
    expected_amount: TokenAmount,
) -> Result<(), StellarError> {
```

Replace Check 5a (`stellar.rs:1046-1096`) so it pins `args[0]` to `expected_from` and explicitly rejects the facilitator:

**Before (the inverted check):**

```rust
// --- Check 5a: args[0] = ScVal::Address(ScAddress::Account) matching facilitator ---
let facilitator_bytes = StellarPublicKey::from_string(&self.public_key)
    .map_err(/* ... */)?
    .0;
match &invoke_args.args[0] {
    ScVal::Address(ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
        Uint256(key_bytes),
    )))) => {
        if *key_bytes != facilitator_bytes {
            // ... return Err(StellarError::InvalidSender { ... })
        }
    }
    // ... Contract / other arms ...
}
```

**After:**

```rust
// --- Check 5a: args[0] (transfer `from`) must be the PAYER, never the facilitator ---
// SECURITY (audit 01): previously this required args[0] == self.public_key
// (the facilitator), which made every accepted transfer drain the facilitator's
// own USDC. Bind it to the declared payer and hard-reject the facilitator.
let expected_from_bytes = StellarPublicKey::from_string(expected_from)
    .map_err(|e| {
        StellarError::InvalidXdr(format!(
            "Could not parse payer public key '{}': {}",
            expected_from, e
        ))
    })?
    .0;
let facilitator_bytes = StellarPublicKey::from_string(&self.public_key)
    .map_err(|e| {
        StellarError::InvalidXdr(format!(
            "Could not parse facilitator public key '{}': {}",
            self.public_key, e
        ))
    })?
    .0;
match &invoke_args.args[0] {
    ScVal::Address(ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
        Uint256(key_bytes),
    )))) => {
        // Never allow the facilitator to be the source of funds.
        if *key_bytes == facilitator_bytes {
            tracing::warn!(
                network = %self.chain.network,
                "B4/audit01: auth entry `from` is the facilitator - rejecting self-drain"
            );
            return Err(StellarError::InvalidSender {
                expected: expected_from.to_string(),
                actual: self.public_key.clone(),
            });
        }
        if *key_bytes != expected_from_bytes {
            let actual_pk = StellarPublicKey(*key_bytes).to_string();
            tracing::warn!(
                network = %self.chain.network,
                expected_sender = %expected_from,
                actual_sender = %actual_pk,
                "B4/audit01: auth entry `from` does not match declared payer"
            );
            return Err(StellarError::InvalidSender {
                expected: expected_from.to_string(),
                actual: actual_pk,
            });
        }
    }
    ScVal::Address(ScAddress::Contract(Hash(bytes))) => {
        let actual_str = Contract(*bytes).to_string();
        tracing::warn!(
            network = %self.chain.network,
            expected_sender = %expected_from,
            actual_sender = %actual_str,
            "B4/audit01: auth entry `from` is a contract address, expected payer account"
        );
        return Err(StellarError::InvalidSender {
            expected: expected_from.to_string(),
            actual: actual_str,
        });
    }
    other => {
        tracing::warn!(
            network = %self.chain.network,
            "B4/audit01: auth entry args[0] has unexpected ScVal type"
        );
        return Err(StellarError::InvalidArgType(format!(
            "args[0] must be ScVal::Address(Account), got {:?}",
            std::mem::discriminant(other)
        )));
    }
}
```

Also update the stale doc comment at `stellar.rs:943` from `[from: facilitator, ...]` to `[from: payer, to: pay_to, amount: max_amount_required]`, and the `InvalidSender` error string at `stellar.rs:117` from `"must be facilitator"` to `"must match payer"`.

### Change 4 — pass the payer at the call site and add an early facilitator-as-payer guard

In `verify_payment`, the payer is already validated at `stellar.rs:1253` (`let payer = StellarAddress::try_from(stellar_payload.from.clone())?;`). Immediately after, add the early guard, then pass the payer into `validate_soroban_auth_entry` (`stellar.rs:1271`):

**Add after `stellar.rs:1253`:**

```rust
// SECURITY (audit 01): the facilitator must never be a payer/source of funds.
if payer.address == self.public_key {
    return Err(StellarError::InvalidSender {
        expected: "any payer != facilitator".to_string(),
        actual: self.public_key.clone(),
    }
    .into());
}
```

**Change the call at `stellar.rs:1271` from:**

```rust
self.validate_soroban_auth_entry(&auth_entry, pay_to_str, requirements.max_amount_required)
    .map_err(FacilitatorLocalError::from)?;
```

**to:**

```rust
self.validate_soroban_auth_entry(
    &payer.address,                          // expected_from = payer
    &auth_entry,
    pay_to_str,
    requirements.max_amount_required,
)
.map_err(FacilitatorLocalError::from)?;
```

(Match the final argument order you chose in Change 3.)

### Optional hardening — bind the nonce store to the signed nonce

The replay key uses the unsigned `stellar_payload.nonce` (`check_nonce_unused` at `stellar.rs:1290`). After Changes 1-3, only `Address` credentials are accepted, which DO carry a signed `credentials.nonce` inside the signed preimage. For defense-in-depth, key the nonce store on `credentials.nonce` (the value committed in the signature) rather than the separate unsigned `payload.nonce`, so a signed entry cannot be re-submitted under a different unsigned nonce. This is hardening, not required to close the drain (Changes 1-3 already eliminate the unsigned `SourceAccount` path).

### Why this closes the hole

- **Change 1** removes the no-signature `SourceAccount` path, so an attacker can never present an unsigned authorization. Every accepted entry now requires a real ed25519 signature.
- **Change 2** forces that signature to be the **payer's** and binds the credential address to `stellar_payload.from`.
- **Change 3** forces the transfer's `from` to be the payer and **hard-rejects the facilitator as source**, so even a (hypothetical) signed entry naming the facilitator as `from` is refused.
- **Change 4** adds a redundant early reject so `from == facilitator` can never reach signing.
  Net: the only thing the facilitator will sign is `transfer(from = a signed, non-facilitator payer, to = pay_to, amount = max_amount_required)` — the facilitator never spends its own USDC, and the drain primitive is gone. Legitimate payer-signed `Address`-credential payments now also work (fixing the side-effect breakage).

> Note (separate finding): Stellar still has **no compliance screening** (`facilitator_local.rs:484-491` is a TODO no-op). That is tracked as the OFAC/blacklist coverage finding and is **not** fixed here — but with this fix the facilitator no longer pays out its own funds, so the residual is sanctioned-payer screening, not treasury drain.

---

## Test plan

All tests live in the `#[cfg(test)] mod tests` block at the bottom of `src/chain/stellar.rs`. The current B4 tests **codify the bug** (`b4_valid_auth_entry_passes` at `stellar.rs:2103-2119` builds `from = facilitator` and asserts OK; `b4_wrong_sender_rejected` at `stellar.rs:2216-2237` asserts a non-facilitator `from` is rejected). These MUST be inverted.

The test helper `make_auth_entry` (`stellar.rs:2055`) currently always sets `credentials: SorobanCredentials::SourceAccount` (`stellar.rs:2092-2094`). Since `validate_soroban_auth_entry` does not inspect credentials, the unit tests for Check 5a still exercise the new payer/facilitator logic correctly; the `SourceAccount` rejection is tested separately at `verify_authorization_signature`. Note: `validate_soroban_auth_entry` now takes a new first arg `expected_from` — update every existing call in the tests.

### 1. Invert `b4_valid_auth_entry_passes` → `b4_payer_as_from_passes`

```rust
#[test]
fn b4_payer_as_from_passes() {
    let provider = test_provider(Network::StellarTestnet);
    // Payer is OTHER_ADDRESS (NOT the facilitator).
    let entry = make_auth_entry(
        &provider, TESTNET_USDC, "transfer",
        OTHER_ADDRESS,            // from = payer
        TEST_RECIPIENT,           // to = pay_to
        10_000_000i128,
        vec![],
    );
    assert!(provider
        .validate_soroban_auth_entry(OTHER_ADDRESS, &entry, TEST_RECIPIENT, token_amount(10_000_000))
        .is_ok());
}
```

### 2. New `b4_facilitator_as_from_rejected` (the drain attempt)

```rust
#[test]
fn b4_facilitator_as_from_rejected() {
    let provider = test_provider(Network::StellarTestnet);
    let facilitator = provider.public_key.clone();
    // Attacker sets from = facilitator (the self-drain primitive).
    let entry = make_auth_entry(
        &provider, TESTNET_USDC, "transfer",
        &facilitator,             // from = facilitator -> must be rejected
        TEST_RECIPIENT,
        10_000_000i128,
        vec![],
    );
    // Declared payer is the facilitator too (worst case); still rejected.
    let err = provider
        .validate_soroban_auth_entry(&facilitator, &entry, TEST_RECIPIENT, token_amount(10_000_000))
        .unwrap_err();
    assert!(matches!(err, StellarError::InvalidSender { .. }),
        "facilitator-as-from must be rejected, got {:?}", err);
}
```

### 3. Update `b4_wrong_sender_rejected` → mismatch between entry `from` and declared payer

```rust
#[test]
fn b4_from_payer_mismatch_rejected() {
    let provider = test_provider(Network::StellarTestnet);
    // Entry says from = OTHER_ADDRESS but we declare the payer as TEST_RECIPIENT.
    let entry = make_auth_entry(
        &provider, TESTNET_USDC, "transfer",
        OTHER_ADDRESS, TEST_RECIPIENT, 10_000_000i128, vec![],
    );
    let err = provider
        .validate_soroban_auth_entry(TEST_RECIPIENT, &entry, TEST_RECIPIENT, token_amount(10_000_000))
        .unwrap_err();
    assert!(matches!(err, StellarError::InvalidSender { .. }),
        "from != declared payer must be rejected, got {:?}", err);
}
```

### 4. New `source_account_credentials_rejected` (Defect B)

Build an `Address`-credentialed entry and a `SourceAccount`-credentialed entry and assert `verify_authorization_signature` rejects the latter:

```rust
#[test]
fn source_account_credentials_rejected() {
    let provider = test_provider(Network::StellarTestnet);
    // make_auth_entry already produces SourceAccount credentials.
    let entry = make_auth_entry(
        &provider, TESTNET_USDC, "transfer",
        OTHER_ADDRESS, TEST_RECIPIENT, 10_000_000i128, vec![],
    );
    let err = provider
        .verify_authorization_signature(&entry, OTHER_ADDRESS)
        .unwrap_err();
    assert!(matches!(err, StellarError::UnsupportedCredentialType),
        "SourceAccount credentials must be rejected, got {:?}", err);
}
```

(If `make_auth_entry` is later changed to take a credentials arg, keep one variant that emits `SourceAccount` for this test.)

### 5. New `credential_address_must_match_payer` (Change 2)

Construct an `Address`-credential entry whose `credentials.address != expected_address` and assert `InvalidSignature`. Add a small helper that builds an entry with `SorobanCredentials::Address(SorobanAddressCredentials { address, nonce, signature_expiration_ledger, signature })` so the credential-binding branch is exercised. Assert that a mismatched credential address is rejected before any signature math.

### 6. Keep passing: `b4_wrong_contract_rejected`, `b4_wrong_function_name_rejected`, `b4_wrong_recipient_rejected`, `b4_wrong_amount_rejected`, `b4_create_contract_fn_rejected` — only their `validate_soroban_auth_entry` calls need the new `expected_from` argument added (use the same address you pass as `from` in `make_auth_entry`).

Run: `cargo test --features stellar -p x402-rs chain::stellar`.

---

## Rollback notes

- This change is **stricter**, not looser: it rejects entries it previously accepted (the facilitator-as-`from` / `SourceAccount` cases) and accepts payer-signed entries it previously rejected. There is no on-disk state or migration; it is pure validation logic plus a function-signature change with all call sites internal to `stellar.rs`.
- If the deployed build regresses (e.g. a legitimate integrator's payload shape is unexpectedly rejected), rollback is: revert the commit and redeploy the prior image tag. **Do NOT re-fund the Stellar mainnet wallet on a rollback** — without this fix the wallet is drainable, so a rollback MUST be paired with keeping the wallet swept (containment step 1) and the mainnet Stellar key removed (containment step 2).
- The new `expected_from` parameter on `validate_soroban_auth_entry` is the only API change; it is a private method, so no external crate breaks.
- Keep the containment levers (swept wallet + key removed) in place until the fixed image is confirmed live (see Verification). Only then re-add `STELLAR_PRIVATE_KEY_MAINNET` and re-fund.

---

## Verification

### Confirm closed BEFORE re-enabling (with mainnet key still removed)

1. Unit tests green: `cargo test --features stellar -p x402-rs chain::stellar` — the new `b4_facilitator_as_from_rejected`, `source_account_credentials_rejected`, `b4_payer_as_from_passes`, `b4_from_payer_mismatch_rejected`, `credential_address_must_match_payer` all pass.
2. With Stellar mainnet key removed, the provider isn't loaded; `POST /settle` with `network: "stellar"` returns an unsupported/not-configured error rather than signing anything. Confirm `/supported` no longer lists `stellar:pubnet` (or lists it without a usable signer) while contained.

### Confirm fixed AFTER deploying the fixed image (and re-adding the mainnet key)

3. Reproduce the exploit against the deployed facilitator and confirm it now **fails**. Post the drain payload (SourceAccount auth entry, `from = facilitator`, `pay_to = attacker`) to `POST /settle`:
   - **Before fix:** HTTP 200 with `success: true` and a real tx hash; the facilitator's USDC balance drops.
   - **After fix:** HTTP 4xx with `errorReason` reflecting `UnsupportedCredentialType` (SourceAccount) or `InvalidSender` (facilitator-as-from); **no transaction is broadcast**; the wallet balance is unchanged.

   ```bash
   # After fix — drain attempt must be rejected, balance unchanged.
   BEFORE=$(curl -s "https://horizon.stellar.org/accounts/GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB" | jq -r '.balances[] | select(.asset_code=="USDC") | .balance')
   curl -s -X POST https://facilitator.ultravioletadao.xyz/settle \
     -H 'Content-Type: application/json' \
     -d @stellar_drain_attempt.json | jq '.success, .errorReason'
   # expect: false  and an InvalidSender / UnsupportedCredentialType reason
   AFTER=$(curl -s "https://horizon.stellar.org/accounts/GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB" | jq -r '.balances[] | select(.asset_code=="USDC") | .balance')
   test "$BEFORE" = "$AFTER" && echo "DRAIN BLOCKED (balance unchanged)" || echo "STILL DRAINABLE"
   ```

4. Positive path: a legitimate payer-signed `Address`-credential payment (`from = real payer`, valid signature) now settles successfully end-to-end on testnet first, then a small mainnet smoke test, before announcing Stellar mainnet as open for merchant traffic. This validates that fixing Defect A did not over-reject real payers.

---

## Residual risk / related findings

- **Stellar compliance is still a no-op** (`facilitator_local.rs:484-491`). After this fix the facilitator no longer drains its own funds, but a sanctioned **payer** can still settle on Stellar because OFAC/blacklist screening is not wired for this chain. Tracked as the separate "Compliance/OFAC screening bypassed on non-EVM paths" finding; fix there, not here.
- **Nonce binding hardening** (optional change above) is recommended but not required to close the drain; do it in the same PR if cheap.
- **Pre-fund hygiene:** keep the Stellar mainnet wallet swept and its key removed until the fixed image is confirmed live by step 3. Re-fund only after verification.
- **Sibling non-EVM bugs from the same auditor** (`nonevm-near-stellar-algorand`): the Algorand path never binds recipient/amount to requirements (separate P1 finding, `src/chain/algorand.rs`), and NEAR should be cross-checked against the same `args[0]==payer` discipline. These are independent fixes but share the "client-supplied payload not bound to requirements / payer" failure pattern — review them together.
