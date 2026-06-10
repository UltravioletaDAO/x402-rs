# Master Execution Plan — Facilitator Security Remediation

> Source: security audit 2026-06-10. Repo: `/mnt/z/ultravioleta/dao/x402-rs`. Service version at audit: v1.46.0.
> Fix docs: `docs/security-audit-2026-06-10/fixes/01..07-*.md`. This plan is the granular, phased task list a separate execution team can run COLD.
>
> **Operating constraints (project policy — do NOT violate):**
> - **NEVER auto-build or auto-deploy.** Make code changes, run `just clippy-all` + `cargo test`, then hand off to the user for `./scripts/fast-build.sh <version> --push` and ECS rollout.
> - **NEVER use emojis in Rust code/log strings** (breaks CloudWatch). Use `[OK]`/`[FAIL]`/`[WARN]`.
> - **NEVER print a real private key, seed, mnemonic, or API key** in any file, log, commit, or chat. Mask as `<REDACTED>`.
> - Git author MUST be `0xultravioleta <0xultravioleta@gmail.com>`. Commit only when the user asks; branch off `main` first.
> - Edition 2021 / Rust 1.82. `regex`, `url`, `uuid` (v4), `std::sync::LazyLock` are already available — no new deps for the redaction work.
>
> **Severity ladder in this plan:** Phase 0 (containment, today) → Phase 1 (P0 Stellar) → Phase 2 (P1 non-EVM binding + ERC-8004) → Phase 3 (P2 compliance choke-point, RPC key redaction+rotation, dependency CVEs) → Phase 4 (P3 hardening clusters + cross-chain invariant suite). Cross-cutting: a single regression matrix proving every chain rejects wrong-recipient / under-amount / wrong-asset / replay / sanctioned-address.
>
> **Sequencing rule:** Phase 0 is the only thing that must ship today (no rebuild). Phase 1 ships next as a standalone hotfix. Phases 2–3 can be parallelized across owners but each task is independently mergeable. Phase 4 lands after 1–3 to avoid churn on the same files.

---

## Phase 0 — IMMEDIATE CONTAINMENT (today, NO rebuild required)

The live P0 (finding 01) is a repeatable drain of the facilitator's **Stellar mainnet** USDC via an unauthenticated `POST /settle`. Containment shrinks the blast radius to zero before any code lands. None of these require a Docker rebuild.

### Task 0.1 — Sweep the Stellar mainnet hot wallet
- **Owner-area:** Ops / key custodian (secure operator machine only).
- **Action:** Move all USDC and all but the base XLM reserve out of `GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB` to a cold/treasury address that is **not** a facilitator hot wallet. Leave ~1.5 XLM base reserve so the account stays valid. Signing key lives in AWS Secrets Manager `facilitator-stellar-keypair-mainnet` (injected `STELLAR_PRIVATE_KEY_MAINNET`) — use it only from a secure operator machine, never paste into any file/log/chat. USDC SAC `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75` (classic issuer `GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN`).
- **Success criterion:** `curl -s "https://horizon.stellar.org/accounts/GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB" | jq '.balances'` shows USDC ~0 and native XLM at the base reserve.
- **Proof of done:** Horizon balances snapshot saved (USDC == 0).

### Task 0.2 — Take the Stellar mainnet settle path out of service (preferred: remove the hot key)
- **Owner-area:** TERRAFORM/AWS.
- **Action:** In `terraform/environments/production` remove/blank the `STELLAR_PRIVATE_KEY_MAINNET` secret injection in `secrets.tf`, `terraform apply`, then `aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2`. With no mainnet Stellar key the Stellar mainnet provider cannot sign, so the drain tx can never be built. Stellar testnet is unaffected.
- **Success criterion:** A drain attempt against `POST /settle` with `network: "stellar"` no longer signs anything (returns unsupported/not-configured); `/supported` no longer advertises a usable Stellar mainnet signer.
- **Proof of done:** Post-deploy curl of the drain payload returns 4xx with no broadcast; Horizon balance unchanged on repeated attempts.

### Task 0.3 — (Alternative to 0.2 if redeploy is blocked) Edge block of Stellar
- **Owner-area:** Ops / edge (Caddy / ALB / WAF).
- **Action:** Add a rule rejecting `POST /settle` and `POST /verify` whose body `network` is `stellar` / `stellar:pubnet` / `stellar-pubnet`. Stopgap only — key removal (0.2) is authoritative.
- **Success criterion:** Stellar settle/verify requests are 4xx at the edge.

### Task 0.4 — Decide on pausing other non-EVM payment paths until Phase 2 binds them
- **Owner-area:** STRATEGIST + AEGIS (risk call).
- **Context:** Findings 03 (Solana settlement-account), 04 (Sui coin-type), 05 (Algorand recipient/amount) are **merchant-fund-loss** forgeries (not treasury drain), so they do not require same-day key removal. But any merchant trusting `/settle` on those chains is exposed until Phase 2 ships.
- **Action (choose per chain):** (a) Set `ENABLE_SETTLEMENT_ACCOUNT` to remain **unset/false** (it already is; finding 03 Fix C makes the gate explicit) — Solana settlement-account path stays off by default. (b) For Sui/Algorand, notify known integrating merchants to independently confirm on-chain receipt until Phase 2 is live, OR temporarily edge-block `network: "sui"` / `network: "algorand"` on `/settle` if no live merchant traffic depends on them.
- **Success criterion:** A documented, signed-off decision per non-EVM chain (pause vs. accept-with-merchant-warning), recorded in the incident channel. No silent exposure.

### Task 0.5 — Confirm production replay/idempotency stores are the durable backends (not in-memory)
- **Owner-area:** TERRAFORM/AWS.
- **Action:** Verify `NONCE_STORE_TABLE_NAME` and `IDEMPOTENCY_TABLE_NAME` are set to the DynamoDB tables in the running task (recon §6 says prod sets both at `main.tf:757/761`). In-memory/Noop fallback = replay window on restart for non-EVM and double-settle on retries.
- **Success criterion:** `aws ecs describe-task-definition` shows both env vars populated; CloudWatch shows DynamoDB store init, not the in-memory fallback log line.

---

## Phase 1 — P0 FIX: Stellar auth-entry inversion + SourceAccount bypass

> Reference: **fixes/01-P0-stellar-facilitator-usdc-drain.md**. All edits in `src/chain/stellar.rs`. The fix is strictly **stricter** validation: it rejects entries it used to accept (facilitator-as-`from`, `SourceAccount`) and accepts payer-signed `Address`-credential entries it used to reject. Ship as a standalone hotfix.

### Task 1.1 — Reject `SourceAccount` credentials on the payment path
- **Owner-area:** AEGIS (Rust).
- **File/fn:** `src/chain/stellar.rs` → `verify_authorization_signature`, lines ~626–633.
- **Change:** Replace the `SorobanCredentials::SourceAccount => { return Ok(()); }` arm (which skips all signature verification) with a hard reject returning `StellarError::UnsupportedCredentialType` (variant already exists ~`stellar.rs:99-100`) and a `tracing::warn!`. Before/after verbatim in fix doc Change 1.
- **Why it closes the hole:** Removes the no-signature path; every accepted entry now requires a real ed25519 signature (Defect B).
- **Success criterion:** A `SourceAccount`-credentialed entry is rejected before any signing.
- **Test (add):** `source_account_credentials_rejected` — `make_auth_entry` (which emits `SourceAccount`) → assert `verify_authorization_signature` returns `Err(UnsupportedCredentialType)`.

### Task 1.2 — Bind the `SorobanAddressCredentials.address` to the declared payer
- **Owner-area:** AEGIS.
- **File/fn:** `src/chain/stellar.rs` → `verify_authorization_signature`, immediately after the `Address(addr_creds)` arm binds `credentials`, before the signature-format match (~`stellar.rs:639`).
- **Change:** Extract the credential's ed25519 account address; if it is not an ed25519 `Account` or `!= expected_address` (the declared payer `stellar_payload.from`), return `StellarError::InvalidSignature`. Verbatim in fix doc Change 2.
- **Why:** Ties the signed credential, the signature subject, and the on-chain `require_auth` target to the same payer.
- **Success criterion:** An `Address`-credential entry whose credential address != declared payer is rejected before signature math.
- **Test (add):** `credential_address_must_match_payer` — build an `Address`-credential entry with mismatched `credentials.address` → assert `InvalidSignature`.

### Task 1.3 — Invert Check 5a: require `args[0] == payer`, hard-reject `args[0] == facilitator`
- **Owner-area:** AEGIS.
- **File/fn:** `src/chain/stellar.rs` → `validate_soroban_auth_entry`. (a) Add a new first parameter `expected_from: &str` to the signature (~`stellar.rs:948-953`). (b) Replace Check 5a (~`stellar.rs:1046-1096`) so it parses `expected_from`, computes `facilitator_bytes`, and: rejects `args[0] == facilitator` (self-drain) with `InvalidSender`, rejects `args[0] != expected_from` with `InvalidSender`, rejects `Contract` and other ScVal arms. Verbatim in fix doc Change 3.
- **Also:** Update the stale doc comment at `stellar.rs:943` `[from: facilitator, ...]` → `[from: payer, to: pay_to, amount: max_amount_required]`, and the `InvalidSender` string at `stellar.rs:117` from `"must be facilitator"` → `"must match payer"`.
- **Why:** Forces the transfer `from` to be the payer and never the facilitator — this is the core of the drain primitive (Defect A).
- **Success criterion:** `validate_soroban_auth_entry` accepts payer-as-`from` and rejects facilitator-as-`from`.
- **Tests (invert/add):**
  - Invert existing `b4_valid_auth_entry_passes` (builds `from=facilitator`, asserts OK — codifies the bug) → `b4_payer_as_from_passes` (`from=OTHER_ADDRESS`, asserts OK).
  - New `b4_facilitator_as_from_rejected` — `from=facilitator` → assert `InvalidSender`.
  - Convert `b4_wrong_sender_rejected` → `b4_from_payer_mismatch_rejected` — entry `from != declared payer` → assert `InvalidSender`.
  - Add the new `expected_from` arg to all existing `validate_soroban_auth_entry` call sites in tests (`b4_wrong_contract_rejected`, `b4_wrong_function_name_rejected`, `b4_wrong_recipient_rejected`, `b4_wrong_amount_rejected`, `b4_create_contract_fn_rejected`).

### Task 1.4 — Pass the payer at the call site + early facilitator-as-payer guard
- **Owner-area:** AEGIS.
- **File/fn:** `src/chain/stellar.rs` → `verify_payment`. After the payer is validated (~`stellar.rs:1253`, `let payer = StellarAddress::try_from(...)`), add an early guard `if payer.address == self.public_key { return Err(InvalidSender ...) }`. Change the `validate_soroban_auth_entry(...)` call (~`stellar.rs:1271`) to pass `&payer.address` as `expected_from` (match the arg order chosen in 1.3). Verbatim in fix doc Change 4.
- **Why:** Redundant early reject so `from == facilitator` can never reach signing.
- **Success criterion:** Drain payloads error before tx construction.

### Task 1.5 — (Optional hardening, same PR if cheap) Bind nonce store to the signed `credentials.nonce`
- **Owner-area:** AEGIS.
- **File/fn:** `src/chain/stellar.rs` → nonce key at `check_nonce_unused` call (~`stellar.rs:1290`). After 1.1–1.4 only `Address` credentials are accepted, which carry a signed `credentials.nonce`. Key the nonce store on `credentials.nonce` (committed in the signature) rather than the unsigned `payload.nonce`.
- **Why:** Defense-in-depth; a signed entry cannot be re-submitted under a different unsigned nonce.
- **Success criterion:** Replay key is derived from the signed nonce. (Not required to close the drain — Tasks 1.1–1.4 already do.)

### Task 1.6 — Phase 1 verification gate (before re-enabling Stellar mainnet)
- **Owner-area:** AEGIS + Ops.
- **Action:**
  1. `cargo test --features stellar -p x402-rs chain::stellar` — all new + inverted tests green; `just clippy-all` clean.
  2. With the mainnet Stellar key STILL removed (Phase 0): confirm the drain payload to deployed `/settle` returns 4xx (`UnsupportedCredentialType` / `InvalidSender`), no broadcast, Horizon balance unchanged (script in fix doc Verification §3).
  3. Positive path: a real payer-signed `Address`-credential payment settles on **testnet** first, then a small mainnet smoke test.
- **Success criterion:** Drain blocked (balance unchanged) AND a legitimate payer-signed payment settles. Only then re-add `STELLAR_PRIVATE_KEY_MAINNET` and re-fund.
- **Rollback note:** Revert the commit + redeploy prior image, but KEEP the wallet swept and the mainnet key removed — a rollback re-opens the drain.

---

## Phase 2 — P1 FIXES (non-EVM binding + ERC-8004 forgery)

> Four independent task-groups. Each is mergeable on its own. Common theme (recon §14 hotspot #4): client-supplied payload not bound to `requirements` (recipient/amount/asset/network) and/or no proof-of-payment. Mirror the EVM `assert_valid_payment` discipline and the NEAR B3 / Stellar B4 precedents.

### Group 2A — Solana settlement-account forgery (fixes/03)

> `verify_settlement_account` never binds the credited ATA to `pay_to`; two `settle` branches return `success:true` moving zero funds. **Merchant fund loss, not treasury drain.**

- **2A.1 — Bind credited ATA to `pay_to` in `verify_settlement_account`.** Owner: AEGIS. File: `src/chain/solana.rs` `verify_settlement_account` (~`:1349-1558`, credit loop ~`:1473-1540`). Resolve `pay_to` + its ATA before the loop (insert after `let asset_str = ...` ~`:1455`); accumulate `total_credit` ONLY for post-token-balance entries whose `owner == pay_to_str` (the `owner` field is already read at `:1497/:1504` — no extra RPC). Verbatim before/after in fix doc Fix A. **Success:** an unrelated `SIG` crediting other ATAs yields `total_credit == 0 < required_amount` → hard `DecodingError`. **Test:** `test_settlement_credit_to_pay_to_counts` (owner==pay_to accumulates) and the regression `test_settlement_credit_to_non_pay_to_rejected` (owner!=pay_to contributes 0, gate fires). Refactor the predicate into a pure helper `settlement_credit_to_pay_to(...)` for offline unit testing.
- **2A.2 — Make the no-sweep branches hard-error instead of forging success.** Owner: AEGIS. File: `src/chain/solana.rs` `settle_settlement_account` (~`:1601-1626`, `settleSecretKey==None`) and `sweep_settlement_account` (~`:1717-1734`, empty settlement ATA). Branch (b) empty-ATA → `Err(FacilitatorLocalError::ContractCall("settlement account ATA is empty: no funds to sweep to pay_to"))`. Branch (a) None: after 2A.1, verify already proved `>= required` reached `pay_to`, so keep success but replace the false "funds already at payTo" comment with the documented invariant guard. Verbatim in fix doc Fix B. **Success:** empty-keypair variant returns error, not `success:true`. **Test:** `test_settlement_empty_ata_hard_errors`, `test_settlement_no_secret_key_requires_pay_to_credit`.
- **2A.3 — Add `ENABLE_SETTLEMENT_ACCOUNT` opt-in gate (default OFF).** Owner: AEGIS. File: `src/chain/solana.rs`. Add `is_settlement_account_enabled()` (mirror `escrow::is_escrow_enabled`) near `:76`; gate both dispatch sites in `verify` (~`:1937-1945`) and `settle` (~`:1963-1970`) — reject `SolanaSettlementAccount` payloads when off. Verbatim in fix doc Fix C. **Success:** default config returns `settlement_account_disabled`. **Test:** `test_is_settlement_account_enabled` (mirror `test_is_escrow_enabled`; `env::remove_var` at end; serialize). **Infra note (deploy engineer, NOT this code):** if Crossmint is in active use, operator must add `ENABLE_SETTLEMENT_ACCOUNT=true` to Terraform env AFTER 2A.1/2A.2 are live.
- **2A.4 — Replace the settlement-account compliance no-op with real screening.** Owner: SENTINEL. File: `src/facilitator_local.rs:517-524`. Screen `requirements.pay_to` up front; screen recovered payer inside `verify_settlement_account` after `payer_pubkey` resolves (~`:1542-1544`), using the same `ComplianceChecker` as the EVM/Solana-exact paths; `Block`/`Review` → 403-mapped error. Verbatim in fix doc Fix D. **Success:** a blacklisted `payTo` is rejected on a settlement-account `/settle`.
- **Group success criterion:** With the gate ON, an unrelated `SIG` not crediting `payTo` → `success:false`; a genuine Crossmint settlement that does credit/sweep to `payTo` still succeeds (regression-preserved via `tests/crossmint-smart-wallet/`). **CAUTION (rollback note in fix doc):** before merge, confirm against a REAL Crossmint settlement tx that post-token-balances show a `pay_to`-owned credit; if legitimate flows only credit the settlement ATA, relax 2A.1 to "credit to pay_to OR (credit to settlement ATA AND settleSecretKey present)".
- **Sibling (separate task, same module):** Solana **standard** settle reports success for a confirmed-but-FAILED tx — `send_and_confirm` checks commitment, never `meta.err` (`solana.rs:2224-2235`, success built at `:2017-2026`). **Fix:** after `send_and_confirm`, fetch status/meta and reject if `meta.err.is_some()` (mirror the settlement-account check at `:1406-1417`). Owner: AEGIS. Effort: S. Track as **2A.5**. Test: forge a tx that confirms with `meta.err` set → `settle` returns failure.

### Group 2B — Sui coin-type confusion (fixes/04)

> `validate_ptb` binds the spent coin OBJECT ID but never its Move TYPE; a payer settles with a worthless `Coin<JUNK>` while the facilitator reports USDC success. **Merchant fund loss.**

- **2B.1 — Thread the spent coin id into `check_balance` and assert USDC membership.** Owner: AEGIS. File: `src/chain/sui.rs` `check_balance` (~`:567-605`). Add `spent_coin_id: &ObjectID` param; after `get_coins(*address, Some(self.usdc_coin_type.clone()), ...)`, require `coins.data.iter().any(|c| c.coin_object_id == *spent_coin_id)` else `Err(FacilitatorLocalError::Other("... not a USDC ... coin owned by ..."))`. Because `get_coins` is filtered to `usdc_coin_type`, membership proves the coin is canonical USDC AND owned by the sender. Verbatim in fix doc Change 1. `ObjectID`/`FromStr` already imported (`sui.rs:23,28`).
- **2B.2 — Pass the declared coin id at both callers.** Owner: AEGIS. File: `src/chain/sui.rs` `verify` (~`:776-783`) and `settle` (~`:801-808`). Parse `ObjectID::from_str(&payload.coin_object_id)` and pass to `check_balance`. Verbatim in fix doc Changes 2 & 3.
- **2B.3 — Delete the misleading "type-checker enforces coin type" comment** at `sui.rs:224-230`; replace with the truthful note (fix doc Change 4).
- **Group success criterion:** A PTB splitting a non-USDC `Coin<T>` → `success:false` with `not a USDC ... coin owned by`; a genuine USDC PTB still settles. `GET /supported` still lists `sui`.
- **Tests:** `test_check_balance_rejects_non_usdc_coin` (pure helper `coin_set_contains`), `test_settle_rejects_junk_coin_via_balance_binding` (`#[ignore]`, live testnet RPC). Keep existing `validate_ptb` tests green (signature change doesn't touch them). Run `cargo test --features sui -p x402-rs chain::sui`.

### Group 2C — Algorand recipient/amount unbound (fixes/05)

> `verify`/`settle` never pass `request.payment_requirements`; the signed ASA transfer's `receiver`/`amount` are never compared to `pay_to`/`max_amount_required`. A 1-microUSDC self-transfer confirms a 10-USDC resource. **Merchant fund loss.**

- **2C.1 — Thread `requirements` + `scheme` into `verify_payment_group`.** Owner: AEGIS. File: `src/chain/algorand.rs` `verify_payment_group` (~`:481-484`). Add params `requirements: &PaymentRequirements, payment_scheme: Scheme`; add `PaymentRequirements` to the `use crate::types::{...}` import (`:37-41`; `Scheme`/`MixedAddress` already imported). Verbatim in fix doc Step 1.
- **2C.2 — Add binding checks after field extraction.** Owner: AEGIS. File: `src/chain/algorand.rs`, after the USDC `asset_id` gate (~`:558-563`), before the validity-window block (`:565`): (a) `requirements.network == self.chain.network`; (b) scheme is `Exact` on both sides; (c) `receiver.to_string() == requirements.pay_to` (as `MixedAddress::Algorand`); (d) `U256::from(amount) == requirements.max_amount_required.0` (strict equality). Verbatim in fix doc Step 2. **IMPORTANT:** move these checks ABOVE the `self.algod.status()` network call (`:566`) so unit tests don't hit algod.
- **2C.3 — Pass requirements at both call sites.** Owner: AEGIS. File: `src/chain/algorand.rs` `verify` (~`:877-880`) and `settle` (~`:903-906`): `verify_payment_group(p, &request.payment_requirements, request.payment_payload.scheme)`. Verbatim in fix doc Step 3.
- **2C.4 — (Optional, recommended) Cross-check `requirements.asset` ASA id** (fix doc Step 4) and **(optional hardening) enforce the lease bind** `lease == SHA256(canonical requirements)` (fix doc Step 5 — DO NOT block the P1 on this; needs SDK rollout coordination).
- **Group success criterion:** Wrong-recipient and under/over-amount `/settle` and `/verify` return failure with a receiver/amount mismatch reason and NO group broadcast (confirm no new tx on `https://allo.info`); an exact-match payload still settles.
- **Tests:** `algorand_wrong_recipient_is_rejected`, `algorand_under_amount_is_rejected`, `algorand_over_amount_is_rejected`, `algorand_exact_match_passes`, `algorand_wrong_network_requirements_rejected`, `algorand_non_algorand_pay_to_rejected` (and `algorand_wrong_asa_in_requirements_rejected` if Step 4). Run `cargo test -p x402-rs --features algorand chain::algorand`.

### Group 2D — ERC-8004 reputation forgery + feedback destruction (fixes/02)

> Unauthenticated `/feedback`, `/feedback/revoke`, `/feedback/response`, `/register` sign+broadcast with the facilitator EOA; no proof the caller controls the agent/wallet or paid. The `FeedbackParams.proof` field exists but is never consumed; routes have **no rate limit**.

- **2D.1 — Layer A: rate-limit the ERC-8004 write routes (ship first, low risk).** Owner: AEGIS. Files: `src/handlers.rs` + `src/main.rs`. Add `erc8004_write_routes()` in `handlers.rs` (analogous to `discovery_register_routes()` ~`:189`) holding `/register`, `/feedback`, `/feedback/revoke`, `/feedback/response`; move those `.route(...)` lines out of `routes()` (~`:118-124`). In `main.rs` (~`:359-362`) merge `erc8004_writes` behind a `GovernorLayer` reusing the strict discovery-register config (per_second(12)/burst(5) ≈ 5/min). Verbatim in fix doc Layer A. **Success:** the 6th rapid `/feedback` from one IP returns 429. **Test:** `rate_limit_present_on_erc8004_writes`.
- **2D.2 — Layer C: `ENABLE_ERC8004_WRITES` flag, default OFF in prod (ship immediately, defense-in-depth).** Owner: AEGIS. Files: `src/handlers.rs`. Check at the top of `post_feedback`/`post_revoke_feedback`/`post_append_response`/`post_register`; return 403 when disabled. **Success:** with the flag off, all four write endpoints return 403. This makes the unauthenticated-forgery surface zero by default while Layer B is built/reviewed.
- **2D.3 — Layer B: require verified proof-of-interaction in `post_feedback` (the real fix).** Owner: AEGIS + SENTINEL. File: `src/handlers.rs` `post_feedback` — insert a proof-validation block after `is_erc8004_supported` (~`:2374`) and before provider dispatch (~`:2390`): (1) require `request.feedback.proof` (else 400); (2) `proof.network == request.network`; (3) recompute payment hash — make `ProofOfPayment::compute_payment_hash` (`erc8004/types.rs:416`) `pub` or add `verify_payment_hash()`, reject on mismatch; (4) inside the EVM branch where `provider` exists: fetch `get_transaction_receipt(proof.transaction_hash)`, reject if missing/unconfirmed/`status()==false`; resolve `IIdentityRegistry::ownerOf(agentId)` (ABI already used ~`:3754-3762`) and require `proof.payee == owner`; (5) add a `signature` field to the feedback envelope, recover the EIP-712 signer over `(agentId, value, valueDecimals, tag1, feedbackIndex)`, reject unless `recovered == proof.payer`; (6) one-feedback-per-`payment_hash` via the nonce/idempotency store. Sketch in fix doc Layer B. **Mirror for Solana** (ed25519 over the same tuple; fetch tx via `rpc_client().get_transaction`, check `meta.err.is_none()`) — handlers.rs Solana branch ~`:2393-2487`.
- **2D.4 — Apply the proof gate to revoke/response/register.** Owner: AEGIS. `post_revoke_feedback` (~`:2657-2865`): require an EIP-712 signature from the ORIGINAL submitter over `(agentId, feedbackIndex)` and a per-submitter record keyed to the verified payment_hash recorded at feedback time. `post_append_response` (~`:2884`) and `post_register` (~`:4118`): require a recipient/responder signature, at minimum gate behind 2D.2's flag.
- **Group success criterion:** `/feedback` with no proof → 400; tampered `paymentHash` → 400; signature from non-payer → 400; `payee != ownerOf(agentId)` → 400; duplicate `payment_hash` → 409/400; valid proof + signature + confirmed receipt → reaches the `giveFeedback` build step (mock provider). Flag off → 403. Rate limit → 429 on 6th rapid call.
- **Tests:** `test_proof_payment_hash_roundtrip`, `test_feedback_request_requires_proof`, and handler module `erc8004_feedback_auth_tests` (`feedback_rejected_without_proof`, `..._when_payment_hash_mismatch`, `..._when_signature_not_payer`, `..._when_payee_not_agent_owner`, `..._on_duplicate_payment_hash`, `revoke_rejected_without_submitter_signature`, `feedback_accepted_with_valid_proof_and_signature`). Extend `tests/integration/test_erc8004_feedback.py` with a no-proof 400 and forged-proof 400.
- **Rollback note:** Additive (new block, route group, flag, field, `pub` visibility). To roll back behavior without reverting code: `ENABLE_ERC8004_WRITES=false`. NEVER re-expose the open relayer. Residual: revoke attribution relies on the durable submission store — ensure the prod DynamoDB nonce/idempotency store backs it (Phase 0 Task 0.5).

---

## Phase 3 — P2 FIXES (compliance choke-point, RPC key redaction + rotation, dependency CVEs)

### Group 3A — Compliance choke-point hoist (fixes/06) + non-EVM screening no-ops

> Screening lives only in `FacilitatorLocal::verify/settle`; `post_settle`/`post_verify` route `upto`/`escrow`/`commerce`/`refund` away before reaching it → sanctioned EVM address moves USDC by picking an alternate scheme. Separately, non-EVM `exact` paths are no-op/fail-open screens.

- **3A.1 — Expose the compliance checker via a trait.** Owner: SENTINEL. Files: `src/provider_cache.rs` (add `HasComplianceChecker` next to `HasProviderMap` ~`:57`); `src/facilitator_local.rs` (impl it on `FacilitatorLocal`, returning the existing `compliance_checker: Arc<Box<dyn ComplianceChecker>>` field ~`:38`). Verbatim in fix doc Step 1. No struct change.
- **3A.2 — Add `screen_alt_scheme` helper in `src/handlers.rs`.** Owner: SENTINEL. Parses per-scheme `(payer, payee[, lifecycle recipient, fee receiver])` from the already-parsed `json_value` and screens each via `checker.screen_address(...)`; `Block`/`Review` → 403 `Address blocked`; checker error → **fail-closed 503**. Covers upto (`permit2Authorization.from` + `payTo`), escrow/commerce (`authorization.from`/lifecycle `payer` + `paymentInfo.receiver` + `feeReceiver`), and `extensions.refund`. Verbatim in fix doc Step 2.
- **3A.3 — Call the choke point before scheme dispatch.** Owner: SENTINEL. File: `src/handlers.rs` `post_settle` after `scheme` is computed (~`:1567`) and `post_verify` before the escrow branches (~`:1089`). Compute `top_level_scheme` ONCE and reuse (delete the duplicate decls at `:1664`/`:1126`). Add `+ HasComplianceChecker` to the `A:` bound on both handlers (~`:1378-1382`). Verbatim in fix doc Steps 3–5.
- **3A.4 — (Broader scope, related finding) Fix the non-EVM `exact`-path screening.** Owner: SENTINEL. File: `src/facilitator_local.rs` `perform_compliance_screening` (~`:386-532`). Flip the Solana branch from **fail-OPEN** (`:444-455`, "ALLOWING transaction temporarily") to fail-closed, fix the Solana extractor (screens the facilitator's own fee-payer + wrong account, and v0 versioned txs deterministically fail-open — `crates/x402-compliance/src/extractors/solana.rs`), and replace the no-op `Ok` arms for NEAR (`:466`), Stellar (`:484`), Algorand (`:501`), Sui (`:509`), SolanaSettlementAccount (`:517`, see 2A.4), XRPL (`:525`) with per-chain payer/payee extraction. This is the larger sub-task; track each chain as its own ticket but under this group. **Also:** OFAC list is baked into the image and never refreshed (`auto_update=false`) — add a runtime refresh or a CI step to re-bake; `config/blacklist.json` is empty and its base58 (Solana) matching is broken by lowercase normalization (`crates/x402-compliance/src/lists/blacklist.rs`).
- **Group success criterion:** A `config/blacklist.json` address under `scheme=escrow|commerce|upto` or with a `refund` extension returns 403 on `/settle` and `/verify`; the standard `exact` path is unchanged; a clean alt-scheme request still routes through. Non-EVM `exact` paths reject a sanctioned payer/payee.
- **Tests:** `mod alt_scheme_screening_tests` with a stub checker — `test_escrow_authorize_blocked_payer`, `..._blocked_receiver`, `test_commerce_scheme_blocked`, `test_escrow_release_blocked_lifecycle_payer`, `test_escrow_fee_receiver_blocked`, `test_upto_blocked_payer`, `test_upto_blocked_payee`, `test_refund_extension_blocked`, `test_clean_alt_scheme_passes`, `test_exact_scheme_is_noop`, `test_checker_error_fails_closed`. Python: `tests/integration/test_compliance_alt_schemes.py` posting blacklisted addresses under each scheme to `base-sepolia` → 403.

### Group 3B — RPC API key leak in escrow/commerce/upto/escrow-state error responses (fixes/07)

> Escrow/upto/escrow-state error paths interpolate raw alloy/reqwest errors (which embed the API-keyed mainnet RPC URL) into the client response body — unauthenticated credential exposure. EVM-exact path already redacts; these do not.

- **3B.1 — Layer 1: add `scrub_urls()` and collapse transport errors at the `map_err` source.** Owner: SENTINEL. File: `src/redact.rs` (add `scrub_urls()` using a `LazyLock<Regex>` for `https?://\S+` → `<redacted-url>`; verbatim in fix doc Layer 1). Then wrap the SIX source sites: `payment_operator/operator.rs:909,930`, `escrow.rs:862`, `upto/permit2.rs:370,399,503` — `crate::redact::scrub_urls(&format!("...{e:?}..."))`. **Why highest leverage:** URL never enters the error string, so any downstream interpolation is automatically safe.
- **3B.2 — Layer 2: make the SEVEN handler sinks opaque (mirror EVM-exact `handlers.rs:2127-2138`).** Owner: SENTINEL. File: `src/handlers.rs` sinks at `:1117, :1155` (verify escrow), `:1618` (upto), `:1654, :1692` (escrow settle), `:1733` (refund), `:2242` (`/escrow/state`). Replace raw interpolation with `let id = uuid::Uuid::new_v4(); error!(%id, error = %crate::redact::scrub_urls(&e.to_string()), ...)` and body `"...failed (ref: {id})"`. Keep each existing JSON field name/HTTP status. Verbatim in fix doc Layer 2.
- **3B.3 — Layer 3: scrub the server-side log lines** at `handlers.rs:1112,1150,1613,1649,1687,1728,2238` and the direct raw error at `upto/permit2.rs:369` (`warn!(error = %e, ...)`). Per policy, logs are live-streamed. Also opportunistically scrub the provider-init connect error at `evm.rs:241`.
- **3B.4 — Rotate the exposed RPC key (operational, not code).** Owner: TERRAFORM/AWS. Because the leak was reachable by unauthenticated callers in prod, treat any premium key in `facilitator-rpc-mainnet` as potentially exposed and rotate after the code fix ships: `aws secretsmanager update-secret --secret-id facilitator-rpc-mainnet ...` then force-new-deployment.
- **Group success criterion:** `POST /escrow/state` (and `/settle` with `scheme=escrow`/`upto`) error responses contain NO `http(s)://` substring (`grep -Eo 'https?://[^"]+'` finds nothing); body is `{"error":"escrow_state_failed (ref: <uuid>)"}`; server log shows `<redacted-url>`, not the key.
- **Tests:** `src/redact.rs` — `scrub_urls_strips_quicknode_in_error_string`, `scrub_urls_strips_infura_query`, `scrub_urls_handles_multiple_urls`, `scrub_urls_noop_without_url`. `operator.rs`/`escrow.rs`/`permit2.rs` — `contract_call_error_display_has_no_url`. Use a synthetic error string carrying `https://host/<REDACTED>/` — never a real key.

### Group 3C — Dependency CVE upgrades (cargo-audit findings)

> 20 known RUSTSEC advisories on the TLS / Solana-signing / QUIC stacks; **all currently MITM/peer-conditional, none remotely exploitable from the public payment surface**. Triage + bump where semver allows, `[patch]` where blocked, accept-with-doc where git-pinned.

- **3C.1 — Bump in-range patched versions via `cargo update`.** Owner: AEGIS. Pull patched in-range versions where semver allows: `rustls-webpki >= 0.103.13` (0.103 line already present at `Cargo.lock:9378`), force AWS-SDK / jsonrpsee legacy TLS forward where possible. **Targets:**
  - **`quinn-proto 0.11.13` → `>= 0.11.14`** — RUSTSEC-2026-0037 endpoint DoS, **CVSS 8.7** (via solana-client/-quic/-streamer + reqwest + anemo/Sui). Highest score.
  - **`aws-lc-sys 0.38.0` → `>= 0.39.0`** — RUSTSEC-2026-0048/0044, **CVSS 7.4** (rustls 0.23 + AWS SDK TLS crypto backend).
  - **`rustls 0.20.9`** — RUSTSEC-2024-0336 `complete_io` infinite-loop DoS, **CVSS 7.5** (via jsonrpsee-http-client, NEAR RPC TLS, `Cargo.lock:5897`). Forward jsonrpsee/NEAR RPC client off the 0.20 line if possible.
  - **`rustls-webpki 0.101.x`** — reachable panic on crafted CRL (RUSTSEC-2024-0336-adjacent webpki advisory); bump to the patched 0.103 line.
- **3C.2 — `[patch.crates-io]` overrides where transitive bumps are blocked.** Owner: AEGIS. File: root `Cargo.toml`. Add overrides for `quinn-proto >= 0.11.14` and `aws-lc-sys >= 0.39.0` if 3C.1 cannot pull them transitively.
- **3C.3 — Document accepted-risk for git-pinned / no-upstream-fix advisories.** Owner: AEGIS + SENTINEL. These have NO available fix and are CONFIRMED-UNREACHABLE on the public payment path; record in a triage doc with reachability note:
  - `rsa 0.8.2` RUSTSEC-2023-0071 (Marvin timing) ← fastcrypto ← sui-sdk (git-pinned `mainnet-v1.37.3`). No fixed upgrade.
  - `ed25519-dalek 1.0.1` RUSTSEC-2022-0093 (double-pubkey oracle) ← solana-keypair/-signature. **Unreachable** — facilitator never calls the two-arg `sign(msg, pubkey)` API; an attacker cannot supply a mismatched verifying key. (NEAR/Stellar use the SAFE `ed25519-dalek 2.x`.)
  - `curve25519-dalek 3.2.0` RUSTSEC-2024-0344 (timing) ← ed25519-dalek 1.0.1.
  - `ring 0.16.20` RUSTSEC-2025-0009 ← algonaut.
  - `protobuf 2.28.0` ← prometheus (telemetry, off request path).
  Accept-risk is valid ONLY while the Sui path stays non-default-traffic and no code change starts using a vulnerable API.
- **3C.4 — Add a `cargo audit` / `cargo deny` CI gate.** Owner: AEGIS. Add to CI so the advisory set cannot grow silently between manual audits. Allow-list the documented accepted-risk advisories (3C.3) so CI stays green but FAILS on any NEW advisory.
- **Group success criterion:** `cargo audit` reports only the documented accepted-risk advisories; the four high-CVSS network advisories (quinn-proto 8.7, rustls 7.5, aws-lc-sys 7.4, webpki) are resolved or `[patch]`-ed; CI gate is live; full test suite + `just clippy-all` pass after the bump.

---

## Phase 4 — HARDENING (P3 clusters + cross-chain settle-binding invariant suite)

> The 33 P3 items clustered by theme. Land after Phases 1–3 to avoid churn. The capstone (4F) is a recurrence-prevention invariant suite.

### Cluster 4A — Rate-limit / XFF trust
- **4A.1** Per-IP governor is bypassable via `X-Forwarded-For` spoofing behind AWS ALB (`SmartIpKeyExtractor` keys on attacker-controlled leftmost IP). Owner: TERRAFORM + AEGIS. **Fix:** configure ALB to OVERWRITE (not append) XFF, and/or key the governor on the rightmost trusted hop. Files: `src/main.rs` governor key extractor + `terraform/.../main.tf` ALB XFF mode. **Success:** spoofed XFF does not reset the rate budget (load test from one source IP with varied XFF still throttles).
- **4A.2** No global concurrency cap / per-request timeout on the HTTP server. Owner: AEGIS. **Fix:** add tower `TimeoutLayer` + `ConcurrencyLimitLayer`/`LoadShed` in `src/main.rs`. **Success:** in-flight settle worker exhaustion sheds load instead of unbounded growth.
- **4A.3** ERC-8004 write routes rate limit — already covered by 2D.1 (cross-listed here for the P3 ratelimit auditor).

### Cluster 4B — Parser / DoS hardening
- **4B.1** `post_verify` panics on a non-char-boundary byte slice `&body_str[..2000]` in the deserialization-error path. Owner: AEGIS. **Fix:** use `body_str.char_indices().nth(2000)` / `.get(..n)` / a char-safe truncation. File: `src/handlers.rs`. **Success:** a crafted multi-byte body returns 400, no panic/backtrace in logs.
- **4B.2** `extensions` / `output_schema` JSON not nesting-depth-guarded (the `extra` field IS, at `MAX_EXTRA_JSON_DEPTH=16`). Owner: AEGIS. **Fix:** apply the existing `json_depth` guard to these fields. Files: `src/types_v2.rs`, `src/types.rs`. **Success:** a deeply nested `extensions` is rejected.
- **4B.3** `PAYMENT-SIGNATURE` header path bypasses the 64 KiB body limit. Owner: AEGIS. **Fix:** bound the header-carried payload the same way. Files: `src/handlers.rs` + `src/main.rs`. **Success:** an over-limit `PAYMENT-SIGNATURE` payload is rejected.

### Cluster 4C — Discovery SSRF / DNS-rebinding + listing poisoning
- **4C.1** `discovery_crawler.rs::fetch_well_known` has no private-IP/DNS-rebinding guard; the claimed "secondary DNS-resolution gate at fetch time" does not exist (`discovery.rs::validate_resource` only blocks IP-literal hosts, not domains resolving to private IPs). Owner: SENTINEL. **Fix:** resolve the host at fetch time and reject private/link-local/loopback (incl. `169.254.169.254` task-role creds); defense-in-depth even though the crawler is default-OFF. **Success:** a domain resolving to `169.254.169.254` is refused.
- **4C.2** Discovery aggregator (default-ON) merges unvalidated peer-supplied resources (incl. `payTo`) into the client-facing listing; `bulk_import` "update if newer" uses peer-controlled `last_updated` and can overwrite first-party (SelfRegistered) entries → permanent listing poisoning / `payTo` takeover. Owner: SENTINEL. **Fix:** provenance enforcement — never let a peer entry overwrite a `SelfRegistered` one; do not trust peer timestamps. Files: `discovery.rs::bulk_import`, `discovery_aggregator.rs`. **Success:** a peer cannot overwrite a first-party listing.
- **4C.3** No resource-count cap / TTL / eviction; each `/discovery/register` triggers O(n) full S3 load-modify-save → unbounded memory + quadratic storage DoS. Owner: AEGIS. **Fix:** add a cap + TTL + eviction. Files: `discovery.rs`, `discovery_store.rs`.

### Cluster 4D — Escrow / contracts authorization + idempotency (off-chain + Solidity)
- **4D.1** Unauthenticated escrow release/refund: anyone can force-capture or force-void a third party's escrow via the facilitator (no off-chain owner/replay/auth check); `validate_addresses(strict_operator=false)` lets an arbitrary client-controlled operator target burn facilitator gas on reverting txs. Owner: AEGIS + SENTINEL. **Fix:** add off-chain owner/replay binding before signing; consider an operator allowlist or pre-flight `eth_call` simulation to avoid gas burn. Files: `payment_operator/operator.rs` (`execute_release`/`execute_refund_in_escrow`, `validate_addresses`). **Success:** a third party cannot capture/void an escrow they don't own; reverting targets are simulated-out before broadcast.
- **4D.2** Escrow/upto/commerce/fhe settle branches never write an idempotency record, yet the fail-closed read-gate still blocks them on store outage. Owner: AEGIS. **Fix:** write the idempotency record on these paths (or exempt them from the read-gate). Files: `src/handlers.rs` post_settle dispatch, `src/upto/permit2.rs`. **Success:** a retried escrow/upto settle with the same `Idempotency-Key` does not re-broadcast.
- **4D.3** Solana settlement-account replay nonce is marked BEFORE the sweep with no rollback on failure → a failed sweep locks out a legitimate settlement for 7 days, stranding funds. Owner: AEGIS. **Fix:** mark the nonce only after a successful sweep, or roll back on failure. File: `src/chain/solana.rs`. **Success:** a failed sweep does not permanently consume the nonce.
- **4D.4 (Solidity)** Echidna no-double-spend / monotonic invariants are VACUOUS — release/refund fuzzers reconstruct `PaymentInfo` with the wrong salt (`salt=uint256(hash) != original`), so escrow always reverts and the safety properties never observe a real capture/refund. Owner: AEGIS (contracts). **Fix:** rebuild `PaymentInfo` with the original salt in `contracts/test/invariants/PaymentOperatorInvariants.sol` so `release_fuzz`/`refund_fuzz` reach the success branch; then break an accounting line and confirm Echidna now FAILS (proves non-vacuous). **Success:** `echidna . --contract PaymentOperatorInvariants --config echidna.yaml` exercises real captures/refunds and a deliberate accounting bug is caught.
- **4D.5 (Solidity)** Permissionless `release()`/`refundInEscrow()` when condition slot is `address(0)`; condition/recorder plugins invoked with no try/catch or gas cap (reverting plugin bricks the action + burns gas); `distributeFees()` is unauthenticated. Owner: AEGIS (contracts). **Fix:** require a non-zero condition or explicit operator gate; wrap plugin calls in bounded `try/catch`. Files: `contracts/src/operator/payment/PaymentOperator.sol` + combinators. **Success:** a reverting plugin cannot permanently brick release/refund.

### Cluster 4E — XRPL + doc/comment corrections + edge exposure
- **4E.1** XRPL: no supported-asset allowlist (accepts any IOU currency/issuer named in requirements — inconsistent with Stellar/EVM canonical-USDC binding); `DestinationTag` never validated; IOU/native-XRP asset strings can't be expressed on the wire (`MixedAddress` deserializer matches no XRPL-asset form); `wait_for_validation` defaults absent `meta` to `tesSUCCESS` (a malicious RPC could fake success); `x402Version` not enforced to 2. Owner: AEGIS. **Fix:** add a canonical RLUSD/USDC allowlist; validate `DestinationTag`; fix the `MixedAddress` XRPL form; reject absent `meta` instead of defaulting to success; enforce `x402Version == 2`. File: `src/chain/xrpl.rs`, `src/types.rs`. **Success:** a non-allowlisted IOU is rejected; an absent-`meta` validation is treated as failure.
- **4E.2** Doc/comment corrections (no behavior change, prevent future regressions): `assert_time` doc comment contradicts the (safe) code (`src/chain/evm.rs`); Solana settlement-account `facilitator_local.rs` comment falsely claims on-chain verification will screen (corrected by 2A.4); the Sui "type-checker enforces coin type" comment (corrected by 2B.3); the verify-as-exact / settle-as-escrow dispatch asymmetry for `extensions.refund` (`src/handlers.rs`). Owner: AEGIS.
- **4E.3** Edge exposure: balances Lambda is publicly invokable via two unauthenticated, unthrottled paths (API Gateway + ALB) with no concurrency cap (cost/shared-RPC-quota DoS) and logs a truncated private RPC URL prefix on RPC error; `docker-compose` binds the unauthenticated backend on `0.0.0.0:8080`; legacy generic single-wallet EVM/Solana keys still IAM-granted + injected (broader blast radius). Owner: TERRAFORM/AWS. **Fix:** throttle + concurrency-cap the Lambda, scrub its RPC-prefix log; bind compose to `127.0.0.1:8080`; remove the legacy generic-key IAM grants + injections. Files: `terraform/.../lambda-balances.tf`, `lambda/balances/handler.py`, `docker-compose.yml`, `secrets.tf`. **Success:** Lambda is throttled; no legacy generic keys reach the container.

### Cluster 4F — Cross-chain "settle-binding" invariant test suite (recurrence prevention) — CAPSTONE
- **4F.1** Owner: AEGIS + SENTINEL. **Action:** Add a single Rust test module (e.g. `tests/settle_binding_matrix.rs` or per-provider `mod tests`) that forces EVERY chain provider (EVM, Solana standard, Solana settlement-account, Sui, Algorand, NEAR, Stellar, XRPL) through the SAME table of negative cases, asserting each provider binds **recipient + amount + asset + network** to the signed payload AND to `requirements`. This is the structural guard that prevents a new provider from re-introducing the Phase 2 class of bugs.
- **Success criterion (cross-cutting, see below):** the matrix is green for every chain and a deliberately-unbound provider (e.g. reverting 2C's checks) makes it RED.

---

## Cross-cutting success criterion — the regression matrix

A single, CI-enforced regression test matrix proving **every chain provider rejects** each of these and **accepts the exact-match happy path**:

| Negative case | EVM | Solana (std) | Solana (settle-acct) | Sui | Algorand | NEAR | Stellar | XRPL |
|---|---|---|---|---|---|---|---|---|
| **Wrong recipient** (`to`/`receiver`/`pay_to` != requirements) | assert_valid_payment `ReceiverMismatch` | bound | 2A.1 owner==pay_to | validate_ptb recipient | **2C.2 (c)** | near.rs B3 | **1.3 args[1]** | 4E.1 |
| **Under-amount** (value < required) | assert_enough_value | bound | 2A.1 `< required` err | validate_ptb amount | **2C.2 (d)** | near.rs amount | stellar args[2] | 4E.1 |
| **Wrong asset** (token != canonical USDC) | is_supported_asset | mint check | mint check | **2B.1 USDC membership** | usdc_asa_id + **2C.4** | requirements.asset | USDC SAC | **4E.1 allowlist** |
| **Replay** (re-submit same signed payload) | on-chain EIP-3009 nonce | nonce_store | **4D.3 nonce-after-sweep** | digest | group-id nonce | nonce | **1.5 signed nonce** | **4E.1 Sequence + idem** |
| **Sanctioned address** (payer/payee on OFAC/blacklist) | screened | **3A.4 fail-closed** | **2A.4** | **3A.4** | **3A.4** | **3A.4** | **3A.4** | **3A.4** |
| **Confirmed-but-failed tx** reported as success | n/a | **2A.5 meta.err** | meta.err `:1406` | dry-run | status check | — | sim | **4E.1 meta** |

**Definition of done for the whole remediation:** every cell above is covered by a passing `#[test]` (or `#[ignore]` live-RPC test for the chains requiring it), the `cargo audit`/`cargo deny` CI gate (3C.4) is green against the documented accepted-risk allowlist, and `just clippy-all` + `cargo test --all-features -p x402-rs` pass. Bold cells are NEW coverage introduced by this plan; non-bold cells must be confirmed to already pass (and added to the matrix harness if not).

---

## Phase summary / ship order

1. **Phase 0 (today, no rebuild):** sweep Stellar wallet (0.1), remove mainnet Stellar key (0.2), decide non-EVM pause posture (0.4), confirm durable stores (0.5).
2. **Phase 1 (standalone hotfix):** Stellar fix 1.1–1.5, verify 1.6, re-fund only after green.
3. **Phase 2 (parallel task-groups):** 2A Solana settlement-account (+2A.5 std-settle), 2B Sui, 2C Algorand, 2D ERC-8004 (ship 2D.1/2D.2 first, then 2D.3/2D.4).
4. **Phase 3:** 3A compliance choke-point + non-EVM screening, 3B RPC key redaction + rotation, 3C dependency CVEs + CI gate.
5. **Phase 4:** hardening clusters 4A–4E, then the 4F invariant capstone + cross-cutting matrix.

All Rust changes: `just format-all` → `just clippy-all` → `cargo test` per feature flag, then HAND OFF to the user for `./scripts/fast-build.sh <version> --push` + ECS deploy. Never auto-build/deploy.
