# Sui `validate_ptb` never verifies the transferred coin is USDC — coin-type confusion (P1)

## Summary

**What:** On the Sui payment path, the facilitator validates the *object ID* of the coin a payer is spending, but never validates that the coin object's Move *type* is USDC. A payer can build a Programmable Transaction Block (PTB) that splits and transfers a **worthless `Coin<JUNK>`** object while the facilitator reports `success: true` with a real on-chain digest.

**Where:** `src/chain/sui.rs` — `validate_ptb` (lines 231–445), `verify_transaction` (447–565), and `check_balance` (567–605). The two claimed defenses (the `check_balance` USDC query and "Sui's own type-checker") are documented in the comment at `sui.rs:224–230` and **both fail** for type confusion.

**Impact:** A merchant accepting Sui payments via the facilitator ships goods/services on the facilitator's `success:true` attestation, but receives a junk coin and zero USDC. The attacker keeps their USDC (it is never spent) and keeps the goods. The facilitator's signing key and treasury are **not** drained (only minor sponsor gas), which is why the verifier confirmed **P1**, not P0.

---

## Root cause

`src/chain/sui.rs`, `validate_ptb` (the PTB validator). The function's only coin-related check binds the PTB's split-coin object ID to the **client-declared** `coin_object_id` — both sides are attacker-controlled, so it is a tautology. The Move type is explicitly *not* checked, per the in-code comment (lines 224–230):

```rust
// NOTE: We do NOT validate the coin object's Move type here because `CallArg::Object` only
// carries the ObjectID/SequenceNumber/Digest tuple, not the Move type. Coin type enforcement
// is handled by (a) the USDC balance check in `check_balance` which queries only the
// `usdc_coin_type` coins, and (b) Sui's own type-checker which will reject a `SplitCoins`
// on a non-Coin object at execution time. ...
```

The only coin check that actually runs (lines 315–332):

```rust
let ptb_coin_id = match coin_call_arg {
    CallArg::Object(sui_types::transaction::ObjectArg::ImmOrOwnedObject(obj_ref)) => {
        obj_ref.0
    }
    other => { /* ... reject ... */ }
};

if ptb_coin_id != *expected_coin_id {           // expected_coin_id = payload.coin_object_id (client-supplied)
    return Err(/* ... coin object ID mismatch ... */);
}
```

`expected_coin_id` is parsed from the fully client-controlled `ExactSuiPayload.coin_object_id` (a `String`, see `src/types.rs:624`) in `verify_transaction` (sui.rs:519–525). So the assertion is "the PTB spends the coin the client said it would spend" — it constrains nothing about the coin's *type*.

**Why both claimed controls are absent:**

- **(a) `check_balance` (sui.rs:567–605)** calls `get_coins(*address, Some(self.usdc_coin_type.clone()), ...)` (line 583) and only **sums** the sender's total USDC balance (line 589). It never cross-references `coin_object_id` against the returned coin set. An attacker who merely *holds* `>= required_amount` USDC (never spent) passes this check, while the PTB splits a *different* (junk) coin object. `usdc_coin_type` is referenced **only** in the constructor (sui.rs:80–91), `check_balance` (583), the disclaiming comment, and tests — never in `validate_ptb`.
- **(b) "Sui's type-checker rejects `SplitCoins` on a non-Coin object"** is false for type confusion. `0x2::coin::split<T>` / `0x2::transfer` are generic over `Coin<T>`; splitting and transferring a `Coin<JUNK>` executes successfully on-chain. `submit_sponsored_transaction` (sui.rs:612–671) calls `execute_transaction_block` directly with **no dry-run** (and a dry-run would succeed anyway since the PTB is type-valid).

Sui compliance screening is a documented no-op (`facilitator_local.rs:509–516`), so nothing there blocks it. Sui mainnet is compiled into production (`Dockerfile` builds `--features ...,sui`) and is a live payment network, so this is exploitable in the production config.

---

## Exploit (end-to-end, production config)

Preconditions: a merchant exposes a Sui-priced x402 resource; the facilitator has a funded Sui sponsor key (live in prod).

1. **Attacker (the payer) holds `>= required_amount` USDC** in their Sui wallet — it is never spent, only used to pass `check_balance`'s balance sum.
2. **Attacker mints/owns a worthless `Coin<JUNK>`** object with balance `>= required_amount`; note its object id `COIN_JUNK`. (Any module the attacker controls that produces a `Coin<T>` works; `T` is arbitrary.)
3. **Build a PTB** with their own wallet as sender and the facilitator as gas sponsor:
   - `inputs[0] = Object(ImmOrOwnedObject(COIN_JUNK))`
   - `inputs[1] = Pure(required_amount as u64 LE)`
   - `inputs[2] = Pure(merchant_addr 32 bytes)`
   - `commands[0] = SplitCoins(Input(0), [Input(1)])`
   - `commands[1] = TransferObjects([Result(0)], Input(2))`
   - `gas_data.owner = facilitator` (so the `gas_owner` check at sui.rs:429–435 passes)
   Sign it with the attacker (sender) key.
4. **POST `/settle`** with `ExactSuiPayload { from: attacker, to: merchant, amount: required_amount, coin_object_id: COIN_JUNK, transaction_bytes: <PTB base64>, sender_signature: <sig> }` and matching `payment_requirements { network: "sui", scheme: "exact", pay_to: merchant, max_amount_required: required_amount }`.
5. **`verify_transaction` passes:** network/scheme/recipient/amount/zero-amount checks pass; `validate_ptb` passes (coin object id matches the *declared* `COIN_JUNK`, amount matches, recipient matches, gas owner matches); `verify_signature` binds the sig to the sender; `check_balance` passes (the attacker's *USDC* balance, which is never spent, satisfies the sum).
6. **`settle` co-signs as gas sponsor and submits.** The merchant receives `Coin<JUNK>`; the facilitator returns `success: true` with a real digest. Merchant ships goods, received **0 USDC**.

---

## Fix

Bind the **spent coin object** to the canonical USDC Move type **inside the verify/settle path**, before settlement. Do not defer to `check_balance`'s balance sum or to the on-chain type-checker. Use **Option 1** (no extra RPC round trip; reuses the existing `get_coins(usdc_coin_type)` call): require `coin_object_id` to be a member of the sender's USDC coin set. Because `get_coins` is already filtered to `self.usdc_coin_type`, set membership proves the spent coin is **both** USDC **and** owned by the sender.

### File: `src/chain/sui.rs`

#### Change 1 — `check_balance` signature + body (lines 567–605): thread in the coin id and assert membership

**Before:**

```rust
    /// Check USDC balance for the sender.
    async fn check_balance(
        &self,
        address: &SuiAddress,
        required_amount: u64,
    ) -> Result<(), FacilitatorLocalError> {
        let client = SuiClientBuilder::default()
            .build(&self.rpc_url)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to connect to Sui RPC: {}", e))
            })?;

        // Get all USDC coins owned by the address
        let coins = client
            .coin_read_api()
            .get_coins(*address, Some(self.usdc_coin_type.clone()), None, None)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to fetch USDC balance: {}", e))
            })?;

        let total_balance: u64 = coins.data.iter().map(|c| c.balance).sum();

        if total_balance < required_amount {
            return Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Sui(
                address.to_string(),
            )));
        }

        debug!(
            address = %address,
            balance = total_balance,
            required = required_amount,
            "Sui USDC balance check passed"
        );

        Ok(())
    }
```

**After:**

```rust
    /// Check USDC balance for the sender AND bind the spent coin object to USDC.
    ///
    /// `spent_coin_id` is the coin object the PTB splits from (the client's declared
    /// `coin_object_id`). Because `get_coins` is filtered to `self.usdc_coin_type`,
    /// requiring `spent_coin_id` to be a member of the returned set proves the spent
    /// coin is (a) USDC of the canonical type and (b) owned by `address`. This closes
    /// the coin-type-confusion hole where a payer splits a worthless `Coin<JUNK>`.
    async fn check_balance(
        &self,
        address: &SuiAddress,
        required_amount: u64,
        spent_coin_id: &ObjectID,
    ) -> Result<(), FacilitatorLocalError> {
        let client = SuiClientBuilder::default()
            .build(&self.rpc_url)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to connect to Sui RPC: {}", e))
            })?;

        // Get all USDC coins owned by the address (filtered to the canonical USDC type).
        let coins = client
            .coin_read_api()
            .get_coins(*address, Some(self.usdc_coin_type.clone()), None, None)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to fetch USDC balance: {}", e))
            })?;

        // CRITICAL: the coin the PTB spends MUST be one of the sender's canonical-USDC
        // coins. `get_coins` is filtered to `self.usdc_coin_type`, so membership proves
        // the spent coin object is USDC and owned by the sender. Without this, a payer
        // can split a worthless Coin<JUNK> and the facilitator would still report success.
        let spends_usdc = coins
            .data
            .iter()
            .any(|c| c.coin_object_id == *spent_coin_id);
        if !spends_usdc {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: spent coin object {} is not a USDC ({}) coin owned by {}",
                spent_coin_id, self.usdc_coin_type, address
            )));
        }

        let total_balance: u64 = coins.data.iter().map(|c| c.balance).sum();

        if total_balance < required_amount {
            return Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Sui(
                address.to_string(),
            )));
        }

        debug!(
            address = %address,
            balance = total_balance,
            required = required_amount,
            coin_id = %spent_coin_id,
            "Sui USDC balance + coin-type check passed"
        );

        Ok(())
    }
```

#### Change 2 — `verify` caller (lines 776–783): pass the declared coin id

The `verify` path currently re-parses `payload.amount` but does not parse `coin_object_id`. Add the parse and pass it through.

**Before:**

```rust
        // Check balance — parse explicitly; a non-numeric amount is a hard error, not 0.
        let required_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;
        self.check_balance(&payer_addr, required_amount).await?;
```

**After:**

```rust
        // Check balance — parse explicitly; a non-numeric amount is a hard error, not 0.
        let required_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;
        let spent_coin_id = ObjectID::from_str(&payload.coin_object_id).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid coin_object_id '{}': {}",
                payload.coin_object_id, e
            ))
        })?;
        self.check_balance(&payer_addr, required_amount, &spent_coin_id)
            .await?;
```

#### Change 3 — `settle` caller (lines 801–808): pass the declared coin id

**Before:**

```rust
        // Check balance before settlement — parse explicitly; a non-numeric amount is a hard error, not 0.
        let required_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;
        self.check_balance(&sender, required_amount).await?;
```

**After:**

```rust
        // Check balance before settlement — parse explicitly; a non-numeric amount is a hard error, not 0.
        let required_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;
        let spent_coin_id = ObjectID::from_str(&payload.coin_object_id).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid coin_object_id '{}': {}",
                payload.coin_object_id, e
            ))
        })?;
        self.check_balance(&sender, required_amount, &spent_coin_id)
            .await?;
```

#### Change 4 — delete the misleading comment (lines 224–230)

Replace the false "type-checker enforces coin type" note in the `validate_ptb` doc comment with the truth.

**Before:**

```rust
    /// NOTE: We do NOT validate the coin object's Move type here because `CallArg::Object` only
    /// carries the ObjectID/SequenceNumber/Digest tuple, not the Move type. Coin type enforcement
    /// is handled by (a) the USDC balance check in `check_balance` which queries only the
    /// `usdc_coin_type` coins, and (b) Sui's own type-checker which will reject a `SplitCoins`
    /// on a non-Coin object at execution time. What we CAN and MUST verify is that the coin
    /// object ID in the PTB matches the declared `coin_object_id` in the JSON payload, preventing
    /// the client from declaring one coin but signing a PTB that drains a different coin.
```

**After:**

```rust
    /// NOTE: `CallArg::Object` only carries the ObjectID/SequenceNumber/Digest tuple, not the
    /// Move type, so the coin's *type* cannot be checked from the PTB bytes alone. Sui's
    /// type-checker does NOT save us here: `SplitCoins`/`TransferObjects` are generic over
    /// `Coin<T>`, so splitting a worthless `Coin<JUNK>` executes successfully on-chain. The
    /// coin's USDC type is therefore enforced separately in `check_balance`, which requires the
    /// declared `coin_object_id` to be a member of the sender's `get_coins(usdc_coin_type)` set.
    /// Here we only bind the PTB's split-coin object ID to the declared `coin_object_id`, so a
    /// client cannot declare one coin but sign a PTB that drains a different coin.
```

### Why this closes the hole

- `get_coins(*address, Some(self.usdc_coin_type.clone()), ...)` returns **only** coins whose Move type is the canonical USDC type AND that are owned by `address`. Requiring `coin_object_id` (the object the PTB actually splits, already bound to the PTB at sui.rs:327) to be in that set means the split coin is provably USDC. A `Coin<JUNK>` object is never returned by that filtered query, so step 5 of the exploit now fails with a hard rejection.
- Both `verify` and `settle` already call `check_balance`, so adding the coin-id argument enforces the new check on both the simulation (`verify`) and the fund-moving (`settle`) paths — no new call sites or RPC round trips are introduced.
- `ObjectID` is already imported (`use sui_types::base_types::{ObjectID, SuiAddress};`, sui.rs:28) and `FromStr` is in scope (sui.rs:23), so the new `ObjectID::from_str` calls compile without new imports.

---

## Test plan

Add Rust `#[test]` / `#[tokio::test]` coverage in the existing `mod tests` block (`src/chain/sui.rs:881`). The existing `test_validate_ptb_wrong_coin_id` (sui.rs:1073–1091) only covers a *mismatch between the declared and PTB coin id* — it does **not** cover a non-USDC coin that matches the declaration. That gap is exactly this vulnerability.

1. **`test_check_balance_rejects_non_usdc_coin` (new, unit, offline-friendly):** The cleanest no-RPC assertion is on the membership predicate. Refactor the membership check into a small pure helper, e.g. `fn coin_set_contains(coins: &[Coin], id: &ObjectID) -> bool`, and unit-test it with a synthetic `Coin { coin_object_id: dummy_coin_id(), .. }` vector: assert `coin_set_contains(&coins, &dummy_coin_id())` is `true` and `coin_set_contains(&coins, &other_coin_id())` is `false`. This proves a junk coin id (not in the USDC set) is rejected, with zero RPC. (Use `use sui_json_rpc_types::Coin;` in the test module.)

2. **`test_settle_rejects_junk_coin_via_balance_binding` (new, integration, `#[ignore]` by default):** Build the exploit PTB with `build_valid_ptb(sender_addr(), facilitator_addr(), merchant_addr(), 1_000_000, junk_coin_id())` where `junk_coin_id()` is NOT in the sender's on-chain USDC set, point the provider at a Sui testnet RPC, and assert `settle()` returns `Err(FacilitatorLocalError::Other(msg))` with `msg.contains("not a USDC")`. Gate with `#[ignore]` so CI does not require live RPC; run manually with `cargo test --features sui -- --ignored sui`.

3. **Keep existing tests green:** `test_validate_ptb_valid`, `test_validate_ptb_wrong_recipient`, `test_validate_ptb_wrong_amount`, `test_validate_ptb_wrong_coin_id`, `test_validate_ptb_wrong_gas_owner`, `test_validate_ptb_extra_commands`, `test_validate_ptb_malformed_bcs_via_decode` operate on `validate_ptb` directly (no `check_balance`), so they are unaffected by the signature change.

Run: `cargo test --features sui -p x402-rs chain::sui` (and `-- --ignored` for the live test).

---

## Rollback notes

- The change is contained to `src/chain/sui.rs` (one private method signature + two callers + one comment + optional helper/tests). No public API, wire format, or `ExactSuiPayload` shape changes — `coin_object_id` was already a required payload field, so existing/legitimate Sui clients are unaffected.
- To roll back: revert `check_balance` to the 4-line signature `(&self, address, required_amount)`, drop the `spent_coin_id` membership block, and revert the two callers in `verify`/`settle` to `self.check_balance(&addr, required_amount).await?`. Restore the original comment if desired (NOT recommended — it documents a false control).
- **No infra/Terraform/secret changes.** This is a pure code fix; redeploy is a normal Docker image rebuild + ECS rollout (user-driven per project policy — do NOT auto-build/deploy).

---

## Verification

**Build/lint:**
- `just clippy-all` and `cargo build --release --features sui` must pass.
- `cargo test --features sui -p x402-rs chain::sui` — new tests pass, all existing Sui PTB tests stay green.

**Behavioral (live, after deploy):** Use a Sui testnet wallet that holds USDC but crafts a PTB spending a non-USDC `Coin<T>`.
- **Before fix:** `POST /settle` with the junk-coin PTB returns `{"success": true, "transaction": {"sui": "<digest>"}}` and the merchant address receives a junk coin (0 USDC).
- **After fix:** the same request returns an error response (`success: false`) whose `errorReason` contains `not a USDC ... coin owned by` — settlement is refused, no transaction is broadcast, no junk coin reaches the merchant.

**Smoke (no regression for legitimate payments):** A genuine USDC PTB (spending a real `coin_object_id` returned by `sui client gas`/`get_coins(usdc_coin_type)`) still returns `success: true` with a real digest, and the merchant receives USDC. Confirm `GET /supported | jq '.kinds[] | select(.network=="sui")'` still lists Sui after redeploy.

---

## Residual risk / related findings

- **Related (same module/auditor `nonevm-solana-sui`):** the two Solana settlement findings (settlement-account `pay_to` non-binding, and `send_and_confirm` ignoring `meta.err`) are independent payment-success-forgery bugs on Solana and must be fixed separately — they are not addressed here.
- **Residual:** This fix proves the spent coin is the canonical USDC type at verify time via an RPC read. If the coin object changes type between the `check_balance` read and `execute_transaction_block` (not possible for a `Coin<T>`'s type, which is immutable once created), the binding would be stale — practically a non-risk on Sui since a coin object's Move type cannot mutate. The amount/recipient/gas-owner bindings in `validate_ptb` remain unchanged and are unaffected.
- **Defense-in-depth (optional, not required for closure):** Option 2 from the verifier — `client.read_api().get_object(coin_object_id, ObjectDataOptions::new().with_type())` and asserting the returned `StructTag` string equals `self.usdc_coin_type` — gives an authoritative type assertion independent of the sender's wallet contents, at the cost of one extra RPC call. Option 1 (chosen) is preferred because it reuses the existing `get_coins` call and simultaneously proves ownership.
- **Compliance note:** Sui compliance screening remains a no-op (`facilitator_local.rs:509–516`); a sanctioned Sui address can still transact. That is tracked as a separate screening-coverage finding and is out of scope for this fix.
