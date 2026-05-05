# Handoff: migrate Stellar signing path to canonical `TransactionSignaturePayload` XDR

> Status: ready to start in a fresh session
> Priority: medium-low (tech debt cleanup, not a bug fix)
> Estimated effort: 1 focused session (~3-5 hours including tests)
> Risk: manageable with byte-parity invariant test landed FIRST
> Created: 2026-05-05

## Context (read first)

While auditing OWS PR #181 (Stellar chain support being added to `open-wallet-standard/core`), we noticed their implementation uses a more canonical pattern than ours:

**Their approach** (`open-wallet-standard/core` PR #181, file `ows/crates/ows-signer/src/chains/stellar.rs`):

```rust
TransactionSignaturePayload {
    network_id: self.network_id.into(),
    tagged_transaction,
}
.to_xdr(Limits::none())
```

**Our current approach** (`src/chain/stellar.rs`, around line 1130):

```rust
// Manual concatenation
let mut buf = Vec::new();
buf.extend_from_slice(&network_id);
buf.extend_from_slice(&[0, 0, 0, 2]); // ENVELOPE_TYPE_TX
buf.extend_from_slice(&tx_xdr_bytes);
```

**Both produce the same bytes on the wire** — the Stellar signature base format IS exactly that concatenation. The difference is purely structural: nominal struct serialization vs manual concat. Verified byte-equivalent during the OWS PR #181 review (see comment https://github.com/open-wallet-standard/core/pull/181#issuecomment-4372055984).

Why migrate:
1. **Resilience to envelope-type changes.** TxV0, Tx (V1), and TxFeeBump all have different signing semantics; the manual concat hardcodes a single tagged-tx variant assumption. The struct path handles all three via the `tagged_transaction` field.
2. **Removes magic constants.** `[0, 0, 0, 2]` for `ENVELOPE_TYPE_TX` is a footgun. `TransactionSignaturePayloadTaggedTransaction::Tx(...)` self-documents.
3. **Aligns with `stellar-xdr` upstream semver.** If Stellar adds new tagged variants in a future protocol upgrade, our code keeps compiling correctly. Manual concat would silently sign the wrong thing for the new variant.
4. **Future feature: FeeBump support.** When we add FeeBump (used for sponsored transactions), the struct path makes it a one-line addition; the manual path requires a second concat function.

Why NOT urgent:
- Current code processes production payments correctly
- No known bugs
- Wire-format equivalence verified in production for ~6 months

## Out of scope (do NOT touch)

This refactor is the **classic transaction signing path only**. Do NOT touch:

- **Soroban authorization entry signing** (`HashIdPreimageSorobanAuthorization`) — different preimage, different SHA256 input, separately handled. Lives in `assert_soroban_authorization_signature` and related functions.
- **Smart wallet support** — Squads/Crossmint paths in `chain/solana.rs` (not Stellar).
- **Settlement account flow** — Crossmint custodial wallets (not Stellar).
- **Network passphrase string constants** — these stay literal:
  - Mainnet: `"Public Global Stellar Network ; September 2015"`
  - Testnet: `"Test SDF Network ; September 2015"`
- **Horizon / Soroban RPC URLs**, fee estimation logic, sequence number handling, simulation flow — none of those change.

If during the work you find that a refactor would touch any of the above, **stop and re-scope** instead of expanding the diff.

## Files in scope

Primary:
- `src/chain/stellar.rs` — specifically the functions that build the signature preimage and produce the signed envelope bytes (around lines 1030-1150 in current main; locate via `grep -n "ENVELOPE_TYPE_TX\|build_signed_envelope\|fn assert_signature_payload\|fn verify_signature\|fn build_unsigned" src/chain/stellar.rs`).

Reference (read-only, do not modify):
- `src/types.rs::ExactStellarPayload` — payload shape from clients (unchanged)
- `src/chain/stellar.rs` Soroban auth entry functions (unchanged — different signing path)

Cargo:
- `Cargo.toml` — `stellar-xdr` is already a dep at 21.2.0; **no version bump needed**. The `TransactionSignaturePayload`, `TransactionSignaturePayloadTaggedTransaction`, `TransactionExt`, `MuxedAccount` types are all already imported elsewhere in the file. **No new imports needed in most cases**; verify with `grep` before adding.

## Migration steps (red-green-refactor)

Do them in this order. Do not reorder.

### Step 1 — Land the byte-parity invariant test FIRST

Before touching any signing code, add a test that pins the current byte output. This is the regression net.

```rust
#[test]
fn stellar_signature_payload_byte_invariant() {
    // Hard-code a known transaction envelope XDR (existing fixture from
    // tests/fixtures/stellar/ or build via the existing test helpers).
    let tx_xdr_bytes: Vec<u8> = /* fixture */ ;
    let network_id: [u8; 32] = sha2::Sha256::digest(
        "Public Global Stellar Network ; September 2015".as_bytes()
    ).into();

    let current_output = /* call current concat path with tx_xdr_bytes + network_id */;

    // Hard-code the expected bytes (capture from current output once).
    let expected_hex = "..."; // capture-and-pin

    assert_eq!(hex::encode(&current_output), expected_hex);
}
```

Run `cargo test stellar_signature_payload_byte_invariant` and capture the output hex. Hard-code it. Re-run; it must pass.

This test will keep both BEFORE and AFTER versions producing the same bytes. If the refactor breaks anything, this fails first.

### Step 2 — Add the canonical-path equivalence test

```rust
#[test]
fn stellar_canonical_xdr_matches_manual_concat() {
    let tx_xdr_bytes: Vec<u8> = /* same fixture as step 1 */;
    let network_id: [u8; 32] = /* same */;

    let manual = /* current concat output */;
    let canonical = {
        let envelope = TransactionEnvelope::from_xdr(&tx_xdr_bytes, Limits::none()).unwrap();
        let tagged = match envelope {
            TransactionEnvelope::Tx(env) => {
                TransactionSignaturePayloadTaggedTransaction::Tx(env.tx.clone())
            }
            // TxV0 + TxFeeBump variants — see OWS PR #181 stellar.rs for the full match
            _ => unimplemented!("scope: classic Tx envelope only for invariant test"),
        };
        TransactionSignaturePayload {
            network_id: network_id.into(),
            tagged_transaction: tagged,
        }
        .to_xdr(Limits::none())
        .unwrap()
    };

    assert_eq!(manual, canonical, "canonical XDR must equal manual concat");
}
```

This proves the equivalence empirically with our actual fixture before any production code switches over.

### Step 3 — Refactor the signing path

Replace the manual `network_id || ENVELOPE_TYPE_TX || tx_xdr` concat with the canonical struct construction. Delete the `ENVELOPE_TYPE_TX` constant. Add `TxFeeBump` and `TxV0` arms in the tagged-transaction match — even if we don't use them today, handling them correctly costs nothing extra and removes a future footgun.

Reference the OWS PR #181 stellar.rs `transaction_signature_payload` function as the structural template (it handles all three variants cleanly).

After refactor:
- Step 1 invariant test must still pass (same bytes out)
- Step 2 equivalence test must still pass

### Step 4 — Verify on real production-like fixtures

Run the integration tests in `tests/integration/` that touch Stellar:

```bash
cd tests/integration
python test_x402_integration.py --network stellar-mainnet  # or testnet
```

If they don't already cover Stellar end-to-end, capture a real signed envelope (against testnet) before and after the refactor and assert byte equality.

### Step 5 — Verify against on-chain settlement (testnet)

```bash
cargo run --release  # start facilitator with testnet config
```

Then in another shell, submit a small Stellar testnet payment through `/settle` and confirm the tx hash on Stellar Expert. **One end-to-end testnet settlement is the strongest signal** the refactor is correct.

## Definition of Done

The migration is complete when ALL of these are true:

1. `cargo test --release` — all tests pass, including the two new invariant tests from steps 1-2
2. `cargo clippy --all-targets -- -D warnings` — no new warnings
3. `cargo fmt --check` — clean
4. The `ENVELOPE_TYPE_TX` constant has been removed from the file (search returns no matches)
5. The signature preimage construction goes through `TransactionSignaturePayload::to_xdr()`, not manual `Vec::extend_from_slice` concat
6. A real Stellar **testnet** settlement succeeds end-to-end through the facilitator (capture tx hash from Stellar Expert and document it in the commit message)
7. No production wire format change — clients still send the same `ExactStellarPayload` shape, and our facilitator produces the same bytes for the same input
8. Diff stays under ~150 LoC net change (red flag if much larger — likely scope creep)

## Verification commands cheat-sheet

```bash
# Step 1+2 invariants
cargo test --release stellar_signature_payload_byte_invariant
cargo test --release stellar_canonical_xdr_matches_manual_concat

# Full Stellar test suite
cargo test --release stellar

# Clippy + fmt gates
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Sanity grep — must return zero matches after the refactor
grep -n "ENVELOPE_TYPE_TX\|0x00, 0x00, 0x00, 0x02\b" src/chain/stellar.rs
```

## Rollback plan

If anything breaks:

1. The byte-parity invariant test (step 1) catches it before any production deploy.
2. If somehow a regression slips past tests, our facilitator wallet hasn't moved keys in this branch — revert via `git revert <sha>` and redeploy.
3. The Soroban authorization entry path is **untouched** by this refactor, so even partial regressions can't affect smart-contract USDC settlements (which is most of our Stellar volume).

## Reference materials

- OWS PR #181 stellar.rs (the template): https://github.com/open-wallet-standard/core/pull/181/files#diff-ows-crates-ows-signer-src-chains-stellar-rs
- Our verification comment that confirmed byte-equivalence: https://github.com/open-wallet-standard/core/pull/181#issuecomment-4372055984
- `stellar-xdr` crate docs: https://docs.rs/stellar-xdr/21.2.0/stellar_xdr/curr/struct.TransactionSignaturePayload.html
- SEP-0005 (HD derivation, hashed payloads): https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0005.md
- Our current impl: `src/chain/stellar.rs` (commit base for diff: latest main as of handoff date 2026-05-05)

## What NOT to do

- Do not bump `stellar-xdr` version. Same version. Same dep.
- Do not "while you're there" refactor the Soroban auth entry path. That's a separate, riskier migration with its own preimage scheme (`HashIdPreimageSorobanAuthorization`). Open a separate handoff if you want to do it.
- Do not delete the `MAINNET_PASSPHRASE` / `TESTNET_PASSPHRASE` string constants. They're load-bearing for SHA256 hashing — keep them as strings, not as moved-into-a-struct values.
- Do not refactor smart wallet (Squads/Crossmint) flows — Stellar doesn't have them, those are Solana paths.
- Do not skip the byte-parity test (step 1). If you skip it, the rollback plan loses its first line of defense.
