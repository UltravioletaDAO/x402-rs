# Ultravioleta Facilitator (x402-rs) Security Audit — Risk Report

**Date:** 2026-06-10
**Version audited:** 1.46.0 (currently LIVE in production at `https://facilitator.ultravioletadao.xyz`)
**Repo:** `/mnt/z/ultravioleta/dao/x402-rs` @ branch `main`
**Classification:** Internal — contains a LIVE, exploitable P0. Treat as need-to-know until the Stellar wallet is swept and the fix is shipped.

**Methodology (one line):** recon attack-surface map → 22 non-overlapping finders each owning a disjoint slice of the codebase → adversarial verification of *every* P0/P1 by two independent lenses (control-hunt + exploit-repro) reading the actual `file:line` (not the finder's quote) → PM synthesis. P2/P3 findings were collected and deduped but **not** adversarially verified.

---

## Executive summary

**There is one live P0 that can drain the facilitator's own funds today.** On the **Stellar** payment path, the Soroban auth-entry validator is *inverted*: it forces the transfer's `from` field to equal the **facilitator's own** public key, and the `SourceAccount` credential branch skips signature verification entirely. The only invocation the facilitator will ever build and sign is `transfer(facilitator → pay_to)`, paid out of the **facilitator's own USDC**, with no payer signature required. An unauthenticated attacker can repeatedly call `POST /settle` with `pay_to = attacker` and drain the facilitator's Stellar hot wallet up to the per-IP rate budget (~30 req/min), choosing a fresh `nonce` each call to defeat the off-chain replay store.

**This is not theoretical.** As of this report (verified live against Horizon and `/supported`):

- Wallet `GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB` holds **2.0024895 USDC + 18.3884492 XLM** right now.
- `stellar:pubnet` is live in `/supported` (74 network entries returned).

The standing balance is small (~$2), but it is a **hot wallet that is refilled** to service real Stellar payments — every top-up is immediately drainable. **Sweep it now** (see Immediate operator actions) and ship the fix before re-funding.

**Systemic theme (the spine of this report):** the EVM settle path is the strong reference implementation — `assert_valid_payment` (`src/chain/evm.rs:1497-1661`) is a single, mandatory gate that binds **recipient, amount, asset, EIP-712 domain, timing, signature malleability, and balance** to the *signed* EIP-3009 authorization before anything moves on-chain. But **every non-EVM chain re-implements that validation independently**, and at least one binding is missing or inverted on each one:

| Chain | Missing/broken binding | Result |
|---|---|---|
| **Stellar** | `from` *inverted* to facilitator + `SourceAccount` sig bypass | **Facilitator's OWN funds drained** (P0) |
| **Sui** | transferred coin's Move type never bound to USDC | Pay with worthless `Coin<T>`, merchant gets junk (P1) |
| **Algorand** | `receiver`/`amount` never bound to requirements | Under-pay / wrong-recipient, merchant ships for ~$0 (P1) |
| **Solana (settlement-account)** | referenced tx not bound to `pay_to`; no-sweep branches return `success` | Replay any public USDC tx → free goods (P1) |

Plus two systemic **path-bypass** classes where the alternate-scheme handlers skip the choke point the EVM path enforces:

- **ERC-8004** reputation/identity writes (`/feedback`, `/feedback/revoke`, `/register`) are unauthenticated and signed with the facilitator key → reputation forgery + cross-customer feedback destruction (P1).
- **Escrow / commerce / upto / refund-extension** settlement paths never call `perform_compliance_screening` → OFAC/blacklist bypass (downgraded to P2 by the verifier: the sanctioned party moves their *own* funds, so it is a regulatory/sanctions-evasion exposure, not direct fund theft).

**Posture grade: C+** (justification below). The trustless EVM core is genuinely strong; the non-EVM surfaces and the escrow/ERC-8004 dispatch are not, and one of them is a live drain.

---

## Posture grade

**Grade: C+** — *strong core, dangerous edges, one live drain.*

The EVM "exact" payment path — the overwhelming majority of production volume — is well-built. Recipient, amount, asset allowlist, EIP-712 domain resolution, low-`s` malleability rejection, EIP-3009 timing, and balance are all validated against the **signed** authorization in a single mandatory gate (`assert_valid_payment`), and the signature is enforced by *on-chain* simulation rather than a spoofable off-chain `ecrecover`. Replay defense (DynamoDB nonce + idempotency stores, confirmed active in prod), Terraform IAM least-privilege (scoped `GetSecretValue` on explicit ARNs, no wildcard), key redaction in init logs, and the Foundry escrow contracts' `nonReentrant`/`validOperator` guards are all correct. Were the service EVM-only, this would be a B+/A−.

But the service is multi-chain, and **each non-EVM chain re-derives validation from scratch** with no shared invariant — so the *worst* implementation, not the best, sets the security ceiling. Stellar's inverted binding is a live, unauthenticated drain of the facilitator's own USDC (P0). Sui, Algorand, and the Solana settlement-account path each independently fail to bind a field the EVM path binds, each enabling merchant fund-loss. And the escrow/commerce/upto/ERC-8004 handlers bypass the compliance (and, for escrow, authorization) assumptions the EVM path enforces. A single live P0 that drains the operator's own funds caps the grade at C-range regardless of how good the core is; the strong core and the fact that the live exposure is small-balance-but-fixable pulls it up to **C+**.

---

## Confirmed findings — ranked table

Fund-loss / key-exposure first. All rows below were **adversarially verified** against source at the cited `file:line`.

| Rank | Severity | Title | Component | Status | Fix doc |
|---|---|---|---|---|---|
| **1** | **P0** | Stellar auth-entry inversion + `SourceAccount` sig bypass → drain of facilitator's OWN Stellar USDC hot wallet (LIVE) | `src/chain/stellar.rs` (`validate_soroban_auth_entry` Check 5a + `verify_authorization_signature`) | **CONFIRMED** (both lenses; operator-verified live) | `fixes/01-P0-stellar-facilitator-usdc-drain.md` |
| 2 | P1 | Solana settlement-account path forges payment success — referenced tx not bound to `pay_to`; no-sweep branches return `success:true` without moving funds | `src/chain/solana.rs` (`verify_settlement_account` / `settle_settlement_account`) | **CONFIRMED** | `fixes/03` |
| 3 | P1 | Sui coin-type confusion — worthless `Coin<T>` settles as USDC; facilitator reports success | `src/chain/sui.rs` (`validate_ptb` / `verify_transaction`) | **CONFIRMED** | `fixes/04` |
| 4 | P1 | Algorand recipient/amount never bound to requirements — under-pay / wrong-recipient confirmation forgery | `src/chain/algorand.rs` (`verify_payment_group`) | **CONFIRMED** | `fixes/05` |
| 5 | P1 | ERC-8004 reputation forgery + cross-customer feedback destruction (unauthenticated `/feedback`, `/feedback/revoke`) | `src/handlers.rs` (`post_feedback`, `post_revoke_feedback`) | **CONFIRMED** | `fixes/02` |
| 6 | P2 *(was P1)* | OFAC/blacklist screening bypassed on escrow / commerce / upto / refund paths | `src/handlers.rs` dispatch + `payment_operator/`, `escrow.rs`, `upto/` | **CONFIRMED, downgraded** | `fixes/06` |
| 7 | P2 *(was P1)* | Premium RPC API key leaked to unauthenticated clients via escrow/upto error responses | `src/handlers.rs` + `payment_operator/operator.rs`, `upto/permit2.rs`, `escrow.rs` | **CONFIRMED, downgraded** | `fixes/07` |
| — | NOT A FINDING | Solana *standard* settle "reports success for failed tx" | `src/chain/solana.rs` `send_and_confirm` | **REFUTED** (see below) | — |

> Note: ranks 2–5 are all P1 with the same blast-radius class (merchant fund-loss); they are ordered by exploit ease/breadth, not by a severity gap. The P0 at rank 1 is categorically worse because it drains the **facilitator's own** funds with no payer involvement.

---

## Per-finding detail — the 5 confirmed P0/P1 fund-integrity bugs

### 1. [P0] Stellar — auth-entry inversion + `SourceAccount` signature bypass → facilitator USDC drain (LIVE)

- **Impact:** Direct, repeatable, **unauthenticated** drain of the facilitator's *own* Stellar mainnet USDC. The only on-chain invocation the facilitator builds is `transfer(from=facilitator, to=pay_to, amount)`, signed by the facilitator as transaction source — so `require_auth(facilitator)` is satisfied by the facilitator's own source-account signature, no payer signature needed. Live wallet `GCHPGXJT…ZNHB` holds **2.0024895 USDC** right now and is refilled to service payments; `stellar:pubnet` is live in `/supported`.
- **Root cause (two inverted controls):**
  - `src/chain/stellar.rs:1046-1071` — **Check 5a** forces `args[0]` (the transfer's `from`) to equal the *facilitator's* key: it parses `facilitator_bytes` from `self.public_key` and returns `StellarError::InvalidSender` if `*key_bytes != facilitator_bytes`. This is inverted: `args[0]` must be the **payer**, never the facilitator. The intended design (`docs/STELLAR_IMPLEMENTATION_PLAN.md` and the TS SDK) is `args = [from=PAYER, to=pay_to, amount]`.
  - `src/chain/stellar.rs:629-632` — `verify_authorization_signature` returns `Ok(())` immediately for `SorobanCredentials::SourceAccount` ("Source account credentials don't need signature verification here"), and `validate_soroban_auth_entry` never rejects `SourceAccount` creds. So with `SourceAccount` credentials the payer-signature requirement is fully bypassed.
  - The transaction is then built with the facilitator as source account (`build_unsigned_transaction`, `src/chain/stellar.rs:1380-1395`: `MuxedAccount::Ed25519(facilitator_bytes)`), signed by the facilitator, and submitted.
- **Exploit:** Build a `SorobanAuthorizationEntry { credentials: SourceAccount, root_invocation: transfer(facilitator_G_addr, attacker_G_addr, balance) }`; `POST /settle` with `network=stellar`, `pay_to=attacker`, `max_amount_required=balance`, and an `ExactStellarPayload` carrying that entry XDR plus a random `nonce`. Check 5a passes (`args[0]==facilitator`), the signature check is bypassed (`SourceAccount`), the facilitator signs+submits, USDC moves facilitator→attacker. Repeat with a fresh `nonce` (the off-chain nonce store keys on the unsigned payload `nonce`; `SourceAccount` has no Soroban nonce) until drained. *Legitimate SDK payments (`args[0]=payer`) are simultaneously rejected as `InvalidSender`, so the drain is the only working Stellar path.*
- **Fix doc:** `fixes/01-P0-stellar-facilitator-usdc-drain.md`. Summary: in Check 5a require `args[0] == payer` and **explicitly reject `args[0] == self.public_key`** (re-add the GAP-S3 guard); in `verify_authorization_signature` **reject `SorobanCredentials::SourceAccount`** for payment entries (spec mandates `sorobanCredentialsAddress` only) and bind the address-credential to `stellar_payload.from`; add `if stellar_payload.from == self.public_key { return Err(InvalidSender) }` early in `verify_payment`; add a regression test reproducing the `SourceAccount`/facilitator-as-`from` drain.

### 2. [P1] Solana settlement-account path forges payment success

- **Impact:** Complete payment bypass for any merchant accepting the Solana settlement-account (Crossmint) scheme. The facilitator returns `success:true` with a real-but-unrelated transaction hash while the merchant receives **nothing**. Attacker needs only a publicly observable confirmed USDC transfer signature.
- **Root cause:** `verify_settlement_account` (`src/chain/solana.rs:1457-1473+`) only checks that **some** ATA of the correct mint received `>= required_amount` (summed as `total_credit`), explicitly *not* that `pay_to` received it — the dev comment at `1457-1462` admits the `pay_to` binding is "enforced in `sweep_settlement_account`". But the sweep is **skipped** in two branches that still return `success:true`: (a) `settle_secret_key == None`, and (b) the settlement ATA on-chain balance is `0`. In both, `settle_settlement_account` returns `Ok(SettleResponse { success: true, transaction: Some(verification.tx_signature), .. })` with no funds moved to `pay_to`.
- **Exploit:** Observe any confirmed mainnet USDC transfer `≥` price → `SIG`. `POST /settle` with a `SolanaSettlementAccount { transactionSignature: SIG, settleSecretKey: null }`. Nonce check passes (first use of `SIG`); `verify_settlement_account` sums credits across all ATAs, `total_credit ≥ required` → `Ok`; settle sees `settleSecretKey==None` → returns `success` with `transaction=SIG`. Merchant ships goods; received 0 USDC.
- **Fix doc:** `fixes/03`. Require the credited ATA's owner `== requirements.pay_to` (or its derived ATA) in `verify_settlement_account`, and never return `success` on the `None`/`balance==0` branches unless funds have provably reached `pay_to`'s ATA on-chain.

### 3. [P1] Sui coin-type confusion — worthless `Coin<T>` settles as USDC

- **Impact:** A payer obtains goods for free: the merchant receives a worthless/attacker-minted Sui coin object instead of USDC while the facilitator reports `success:true` with a real digest. The attacker's USDC is never moved.
- **Root cause:** `validate_ptb` (`src/chain/sui.rs:224-230`) explicitly does **not** validate the coin object's Move type ("We do NOT validate the coin object's Move type here … Coin type enforcement is handled by (a) the USDC balance check … and (b) Sui's own type-checker"). Both assumptions are false for type-confusion: (a) `check_balance` only proves the *sender holds* USDC (it `get_coins(usdc_coin_type)`), it never constrains which coin the PTB splits; (b) `SplitCoins` is valid on **any** `Coin<T>`, so splitting a `Coin<JUNK>` executes fine. The PTB coin id is only checked to equal the client's own declared `coin_object_id` — a self-consistent but worthless constraint. `usdc_coin_type` is referenced only in `check_balance`, never in `validate_ptb`.
- **Exploit:** Hold `≥ required` USDC (so `check_balance` passes; never spent). Mint a worthless `Coin<JUNK>`; build a PTB `SplitCoins(COIN_JUNK, [amount]) → TransferObjects([split], merchant)`; `POST /settle` with `coin_object_id=COIN_JUNK`. All checks pass; facilitator co-signs as gas sponsor and submits; merchant receives `Coin<JUNK>`.
- **Fix doc:** `fixes/04`. Resolve the PTB `coin_object_id` on-chain (`get_object` with type options) and assert its `StructTag` equals `self.usdc_coin_type` inside `validate_ptb`; equivalently require `coin_object_id ∈ get_coins(usdc_coin_type)`. Hard rejection, not deferred.

### 4. [P1] Algorand — recipient/amount never bound to requirements

- **Impact:** The facilitator confirms (`verify` valid, `settle` `success:true`) **any** USDC ASA transfer the client signed, regardless of recipient or amount. An attacker requesting a 10-USDC resource submits a self-signed 0.000001-USDC transfer to an arbitrary recipient; the facilitator co-signs the fee, broadcasts, and returns success with the attacker as payer. Merchant ships the 10-USDC resource for ~$0.000001. Facilitator's own funds are not drained (only ~0.001 ALGO fee) — merchant fund-loss.
- **Root cause:** `AlgorandProvider::verify`/`settle` (`src/chain/algorand.rs:862-953`) reference only `payment_payload.network` and **never read `request.payment_requirements`**. `verify_payment_group` (`:481-596`) extracts `(asset_id, amount, receiver, sender)` from the signed tx (`:543-549`) and checks only `asset_id == self.chain.usdc_asa_id` (`:558`) and the validity window — it never compares `receiver` to `pay_to` nor `amount` to `max_amount_required`. Grep confirms `requirements`/`pay_to`/`max_amount_required` appear **nowhere** in `algorand.rs`. No scheme check and no `requirements.network` check either.
- **Fix doc:** `fixes/05`. Thread `request.payment_requirements` into `verify_payment_group` and reject unless `receiver == pay_to`, `amount == max_amount_required`, the ASA matches `requirements.asset`, and network/scheme match. Mirror the NEAR `validate_delegate_actions_inner` and Stellar `args[1]/args[2]` patterns.

### 5. [P1] ERC-8004 reputation forgery + cross-customer feedback destruction

- **Impact:** Two primitives, both unauthenticated and gaslessly funded by the facilitator across all 20 networks. **(1) Fabrication** — anyone can post arbitrary positive feedback to inflate, or negative `value` (`i128`) to defame, *any* agent. Because every entry is attributed to the single facilitator `clientAddress` (= `msg.sender`), all reputation signals collapse to one indistinguishable author, destroying the integrity of the system the facilitator vouches for. **(2) Destruction** — anyone can `POST /feedback/revoke` for any `(agentId, feedbackIndex)` the facilitator ever submitted; every revoke succeeds because `msg.sender` (the facilitator) was the recorded submitter, wiping out legitimate customers' on-chain feedback.
- **Root cause:** `post_feedback` (`src/handlers.rs:2326-2640`) reads everything from the body and signs with the facilitator key with **no caller check** — e.g. `reputation_registry.giveFeedback(agent_id, feedback.value, …)` at `:2532-2541` followed by `call.send()`, with on-chain `clientAddress = msg.sender =` the facilitator EOA. `giveFeedback` carries no `clientAddress` param. `post_revoke_feedback` (`:2657-2865`) is worse: `revokeFeedback(agent_id, feedback_index)` with the facilitator as `msg.sender` satisfies the only on-chain gate ("submitter may revoke") for *every* feedback it ever relayed. No proof/signature is checked anywhere (grep for `recover`/`ecrecover`/`authoriz` over the range is empty). The codebase already defines `ProofOfPayment` and `FeedbackParams.proof` ("required for authorized feedback") but **never consumes them**.
- **Fix doc:** `fixes/02`. Gate all ERC-8004 write endpoints on a verified proof-of-interaction: require `feedback.proof`, fetch the settlement tx on-chain, confirm payer/payee/amount/token match, recompute `payment_hash` via `ProofOfPayment::compute_payment_hash`, and require an EIP-712/ed25519 signature from `proof.payer` bound to `(agentId, value, tag, feedbackIndex)`. For revoke, additionally require the original submitter's signature. (Companion P2: these routes also have **no rate limiter** — see appendix.)

---

## Confirmed-but-downgraded (the 2 P2s)

Both were filed P1 and **confirmed real end-to-end**, but the adversarial verifier downgraded them to **P2** because neither directly steals funds — they are sanctions/compliance and infra-credential exposures.

- **[P2, was P1] OFAC/blacklist screening bypassed on escrow / commerce / upto / refund paths** (`fixes/06`). `perform_compliance_screening` (`src/facilitator_local.rs:328`) is invoked **only** from `FacilitatorLocal::verify/settle`. `post_settle` routes alternate schemes *away* from that path and returns before it is reached: `payment_operator::settle_escrow` (escrow/commerce), `escrow::settle_with_escrow` (refund extension), `upto::settle_upto` (upto) — all sign and submit via the provider map directly. Grep for `compliance|screen|blacklist|ofac` over those modules is empty. All three feature flags are ON in prod. **Why downgraded:** a sanctioned party moving their *own* funds is a regulatory/sanctions-evasion liability for the operating entity, not theft of the facilitator's or a third party's funds. *Fix:* hoist screening to a single choke point before scheme dispatch in `post_verify`/`post_settle`, screening both payer and payee.

- **[P2, was P1] Premium RPC API key leaked to unauthenticated clients in escrow/upto error responses** (`fixes/07`). alloy/reqwest transport errors embed the full request URL (including the API key in the path/query). The escrow/upto paths interpolate that error verbatim into the client HTTP response — e.g. `OperatorError::ContractCall(format!("eth_call failed: {:?}", e))` → `handlers.rs` returns `{"error": format!("Escrow state query failed: {}", e)}`. A REDACTED sample of the leaked shape: `… reqwest::Error { … url: "https://<host>.quiknode.pro/<REDACTED_API_KEY>/" … }`. An attacker induces a transient transport error (spam to 429, or natural RPC blips) and reads the key from `/escrow/state` or escrow/upto `/settle`. **Why downgraded:** the leaked credential is a paid *infrastructure* secret, not a signing key — its loss enables RPC-quota DoS and bill abuse (availability/cost), not direct fund movement. *Fix:* return an opaque message + correlation id; sanitize alloy/reqwest errors through a URL-scrubbing redactor before they reach response bodies **or** server logs (logs are streamed live).

---

## Refuted candidate (shows adversarial rigor)

**"Solana *standard* settle reports `success:true` for a tx that confirmed but FAILED on-chain"** — **REFUTED (NOT A FINDING).** The pinned `solana-rpc-client 2.3.13` `confirm_transaction_with_commitment` computes its value as `…map(|r| r.status.is_ok())…`, so it returns `value:true` **only** when the transaction's execution did *not* fail (the library's own doc says it returns `false` "if the transaction failed, even if it has been confirmed"). A failed-but-confirmed tx yields `value:false`, the `send_and_confirm` loop never returns `Ok`, it times out and propagates `Err` via `?` — `settle` returns an error, not `success`. The `meta.err` check the finder claimed was missing is performed *inside* the pinned library. Listed here to document that the candidate was chased to the dependency source and dismissed.

---

## Reported P2/P3 appendix (deduped, NOT independently verified)

The findings below were reported by the 22 finders and **deduped**, but were **not** put through adversarial verification (only P0/P1 were). Treat as leads for a follow-up pass, not as confirmed. Several are near-duplicates of the confirmed P2s above (compliance bypass, RPC-key-in-logs) and are retained for completeness.

### P2 (24 reported)

| # | Title | Component |
|---|---|---|
| 1 | upto sub-scheme dispatch bypasses OFAC/blacklist screening (fund-moving) | `handlers.rs` + `upto/permit2.rs` |
| 2 | Escrow refund-extension path bypasses OFAC/blacklist screening | `escrow.rs` / `handlers.rs` |
| 3 | `verify_proxy_onchain` is security-theater: client-supplied `factory_address` makes the only off-chain escrow gate trivially bypassable; no asset/amount/timing/recipient/sig validation | `escrow.rs` |
| 4 | Unauthenticated gas-treasury DoS on escrow refund path (no pre-flight simulation) | `escrow.rs` |
| 5 | Unauthenticated escrow release/refund: anyone can force-capture/void a third party's escrow via the facilitator | `payment_operator/operator.rs` |
| 6 | Arbitrary client-controlled operator target burns facilitator gas on reverting escrow txs (economic DoS) | `payment_operator/operator.rs` |
| 7 | `FeedbackParams.proof` ("required for authorized feedback") defined+documented but never validated (dead control) | `handlers.rs` `post_feedback` / `erc8004/types.rs` |
| 8 | Gasless ERC-8004 write endpoints (`/register`, `/feedback`, `/feedback/revoke`, `/feedback/response`) have NO rate limiter → gas-treasury drain | `main.rs` / `handlers.rs` routes |
| 9 | Compliance screening is a no-op (always `Ok`) on NEAR, Stellar, Algorand, Sui, XRPL, Solana-settlement-account | `facilitator_local.rs` |
| 10 | Solana compliance screening fails OPEN on extraction error; legacy-only parser defeated by versioned txs | `facilitator_local.rs` + `extractors/solana.rs` |
| 11 | OFAC list is stale and never refreshed at runtime (`auto_update=false`, baked into image) | `config/ofac_addresses.json` + compliance crate + Dockerfile |
| 12 | Premium RPC API key written to server-side logs (CloudWatch + live stream) via alloy error on EVM verify/settle/balance | `chain/evm.rs` + `handlers.rs:2131` |
| 13 | XRPL Payment `DestinationTag` never validated → false success attest for custodial/exchange destinations | `chain/xrpl.rs` |
| 14 | NEAR/Stellar/Algorand value paths perform no OFAC/blacklist screening | `facilitator_local.rs` + chain modules |
| 15 | Discovery aggregator merges unvalidated peer-supplied resources (incl. `payTo`) into client-facing listing (default-ON) | `discovery_aggregator.rs` / `discovery.rs` |
| 16 | `bulk_import` "update if newer" uses peer-controlled `last_updated`, can overwrite first-party entries (listing poisoning / `payTo` takeover) | `discovery.rs` / `discovery_aggregator.rs` |
| 17 | Discovery registry has no count cap/TTL/eviction; each register triggers O(n) full S3 load-modify-save (unbounded memory + quadratic cost DoS) | `discovery.rs` / `discovery_store.rs` |
| 18 | Solana compliance screening non-functional: extractor screens facilitator's own fee-payer + wrong account; v0 txs fail-open | `extractors/solana.rs` |
| 19 | Permissionless `release()`/`refundInEscrow` when condition slot is `address(0)`: anyone can force-capture/void escrow (no attacker profit) | `PaymentOperator.sol` + `PaymentOperatorAccess.sol` |
| 20 | Echidna no-double-spend/monotonic invariants are VACUOUS: fuzzers reconstruct `PaymentInfo` with wrong salt → escrow always reverts, properties never observe a real capture/refund | `PaymentOperatorInvariants.sol` |
| 21 | Gasless ERC-8004 write endpoints have NO rate limit (duplicate of #8, different finder) | `main.rs` + `handlers.rs` |
| 22 | Per-IP rate limiter bypassable via `X-Forwarded-For` spoofing behind ALB (`SmartIpKeyExtractor` keys leftmost IP) | `main.rs` + `main.tf` ALB XFF mode |
| 23 | Balances Lambda publicly invokable via two unauthenticated, unthrottled paths (API GW + ALB), no concurrency cap → cost / shared-RPC-quota DoS | `lambda-balances.tf` + `lambda/balances/handler.py` |
| 24 | Dependency tree carries 20 known RustSec advisories (incl. 8.7/7.4/7.5 CVSS) on TLS/Solana-signing/QUIC stacks; all MITM/peer-conditional, none remotely exploitable from the public payment surface | `Cargo.lock` |

### P3 (33 reported — titles only)

| # | Title | Component |
|---|---|---|
| 1 | EIP-6492 counterfactual & multi-byte EIP-1271 sigs unreachable (fail-closed); 6492 settle path lacks verify path's `isValidSig` pre-check | `chain/evm.rs` |
| 2 | `assert_time` doc comment contradicts (safe) code, inviting a future expired-auth regression | `chain/evm.rs` |
| 3 | verify/settle dispatch asymmetry: `extensions.refund` settles as escrow but verifies as standard payment | `handlers.rs` |
| 4 | upto/escrow/fhe settle branches never write an idempotency record; retries re-run settlement (gas burn) | `handlers.rs` + `upto/permit2.rs` |
| 5 | Escrow settle never writes an idempotency record → Idempotency-Key guard inert for escrow | `handlers.rs` / `idempotency_store` |
| 6 | Escrow scheme does not bind `paymentInfo.receiver` to `payTo` or value to `maxAmountRequired` | `payment_operator/operator.rs` |
| 7 | Stellar off-chain replay nonce key + expiry use UNSIGNED request fields (decoupled from signed auth) | `chain/stellar.rs` |
| 8 | Solana settlement-account replay nonce marked before sweep with no rollback → 7-day lockout / stranded funds | `chain/solana.rs` |
| 9 | Idempotency cache silently never written for escrow/upto/commerce/fhe; fail-closed read still blocks on store outage | `handlers.rs` |
| 10 | No length/format bounds on `agent_uri`/`feedback_uri`/`tag1`/`tag2`/`response_uri` → calldata gas amplification | `handlers.rs` + `erc8004/types.rs` |
| 11 | `post_register` mints identity NFT owned by facilitator and push-transfers to arbitrary recipient (no consent/ownership proof) | `handlers.rs` `post_register` |
| 12 | Custom blacklist (`config/blacklist.json`) empty; base58 (Solana) matching broken by lowercase normalization | `config/blacklist.json` + compliance crate |
| 13 | Algorand provider init logs algod RPC URL unredacted (defense-in-depth) | `chain/algorand.rs:356-362` |
| 14 | Solana Path-2 CPI inner-instruction scan fails open when RPC omits `stack_height`/post-balance | `chain/solana.rs` |
| 15 | Solana settlement-account path performs no compliance/OFAC screening despite comment claiming on-chain verification screens | `chain/solana.rs` + `facilitator_local.rs` |
| 16 | XRPL IOU/native-XRP asset strings cannot be expressed on the wire (`MixedAddress` deserializer matches no XRPL form) | `types.rs` + `chain/xrpl.rs` |
| 17 | XRPL path has no supported-asset allowlist (accepts any IOU currency/issuer) | `chain/xrpl.rs` |
| 18 | XRPL relies solely on on-chain Sequence for replay; concurrent identical-blob `/settle` race can return `success:true` twice | `chain/xrpl.rs` + `handlers.rs` |
| 19 | `wait_for_validation` defaults `meta.TransactionResult` to `tesSUCCESS` when meta absent → malicious RPC can fake success | `chain/xrpl.rs` |
| 20 | `x402Version` not enforced to 2 on XRPL path (check is documented no-op) | `chain/xrpl.rs` |
| 21 | Discovery crawler outbound fetches with no private-IP/DNS-rebinding guard; claimed "secondary DNS gate" does not exist (gap if crawler enabled) | `discovery_crawler.rs` |
| 22 | Verify handler panics on attacker body via non-char-boundary byte slice `&body_str[..2000]` | `handlers.rs` |
| 23 | `extensions`/`output_schema` JSON fields not nesting-depth-guarded (defense-in-depth) | `types_v2.rs` / `types.rs` |
| 24 | `PAYMENT-SIGNATURE` header path bypasses the 64 KiB body limit | `handlers.rs` + `main.rs` |
| 25 | x402-axum middleware default (settle-after-execution) lets side-effecting handlers run before payment committed → unpaid work | `crates/x402-axum/layer.rs` |
| 26 | x402-axum middleware trusts facilitator verify/settle response without re-binding to resource requirements | `crates/x402-axum/layer.rs` |
| 27 | `distributeFees()` unauthenticated (benign accounting-only; confirm accepted) | `PaymentOperator.sol` |
| 28 | Condition & recorder plugins invoked with no try/catch and no gas cap → reverting plugin bricks the action / burns gas | `PaymentOperator.sol` + combinators |
| 29 | No global concurrency cap or per-request timeout on the HTTP server → worker exhaustion | `main.rs` (missing tower Timeout/ConcurrencyLimit/LoadShed) |
| 30 | Balances Lambda logs a truncated private RPC URL prefix on every RPC error (partial API-key disclosure to log readers) | `lambda/balances/handler.py` |
| 31 | `docker-compose` binds unauthenticated backend on `0.0.0.0:8080` (bypasses Caddy rate-limit if exposed) | `docker-compose.yml` + `config/Caddyfile` |
| 32 | Legacy generic single-wallet EVM/Solana keys still IAM-granted and injected (broader blast radius) | `secrets.tf` + `main.tf` |
| 33 | No `cargo-audit`/`cargo-deny` gate in CI; flagged crates have no upstream fix (rsa Marvin, ed25519-dalek 1.0.1 oracle) until pinned Sui/Algorand SDKs upgraded | `Cargo.toml` / CI |

---

## Coverage — 22 areas audited

Each finder owned a disjoint slice and reported both findings and **explicitly reviewed-and-confirmed-safe** controls. Highlights of what was confirmed *safe* (cited from finders' `reviewed` lists):

- **EVM signed-payload validation** (`evm-signature-and-smartwallet`, `evm-eip3009-field-validation`): `assert_valid_payment` (`evm.rs:1497-1661`) confirmed as the single mandatory gate called first in both verify (`:672`) and settle (`:835`); recipient binding (`:1552-1564`, `authorization.to` vs `requirements.pay_to`), scheme/network match, asset allowlist, timing, balance, value all sourced from the **signed** authorization with no bypass path. **CONFIRMED SAFE.**
- **EVM EIP-712 domain + malleability** (`evm-signature-and-smartwallet`): `assert_domain` derives `chain_id`/`verifying_contract` from facilitator config (client `extra.name/version` only logged as warning, not trusted); inline low-`s` check (`:1609-1637`) rejects non-65-byte / high-`s` sigs per EIP-2; signature authority is on-chain `transferWithAuthorization`, not a spoofable off-chain `ecrecover`. Cross-token/cross-chain replay prevented. **CONFIRMED SAFE.**
- **Replay / idempotency** (`replay-and-idempotency-stores`): `DynamoNonceStore::check_and_mark_used` uses an atomic conditional `PutItem`; `create_nonce_store`/`create_idempotency_store` fall back to in-memory/Noop only when the env table name is unset — **prod sets both** (`main.tf`). EVM replay is the on-chain EIP-3009 nonce. **CONFIRMED SAFE in prod config.**
- **Key lifecycle & secret leakage** (`key-handling-and-secret-leakage`): all key loaders in `from_env.rs` carry no `Debug`/`Display`; `redact.rs::rpc_url()` strips API keys at init-log sites; `telemetry.rs` records method+URI only (no bodies/secrets in spans); graceful-shutdown handles no secrets. (Gap: alloy errors on **escrow/upto** paths — the confirmed P2 #7 — and on EVM logs — appendix P2 #12.) **CORE SAFE.**
- **Terraform IAM / secrets** (`terraform-iam-secrets-exposure`): `GetSecretValue` scoped to `local.all_secret_arns` (full ARNs incl. random suffix, **no wildcard**); RPC-with-API-key never in plaintext env (uses `valueFrom` selectors); `terraform.tfvars` gitignored, holds only names; `image_tag` has no default (prevents `latest` drift). **CONFIRMED LEAST-PRIVILEGE.** (Residual: legacy generic keys still wired — appendix P3 #32.)
- **Escrow contracts (Foundry)** (`contracts-operator-authz-reentrancy`): `authorize/charge/release/refundInEscrow/refundPostEscrow/distributeFees` all carry `nonReentrant`; `validOperator` enforces `paymentInfo.operator == address(this)`; CREATE2 factory binds operator address to full config. **REENTRANCY/ACL SAFE** (caveats: permissionless release/refund when condition slot is `address(0)` and vacuous echidna invariants — appendix P2 #19/#20).

The remaining areas audited: settle-dispatch + upto; escrow proxy/refund extension; payment-operator authz; ERC-8004 forgery; network/token config + recipient; blocklist/compliance coverage; non-EVM Solana/Sui; non-EVM NEAR/Stellar/Algorand; non-EVM XRPL; SSRF/discovery/FHE; parser/DoS/body-limits; crates middleware + compliance; contracts conditions/fees/echidna; rate-limit/economic-DoS/gas; lambda-balances edge exposure; supply-chain deps + ops scripts. (22 finders total; counts above are per-finder reviewed items.)

---

## Immediate operator actions

1. **SWEEP the Stellar hot wallet NOW.** Move the ~2.0024895 USDC + ~18.39 XLM out of `GCHPGXJT…ZNHB` to cold storage before anyone else does. The drain is live and the wallet is refilled — do not re-fund until the fix ships.
2. **Optionally hard-disable the Stellar path** until `fixes/01` is deployed: drop `stellar:pubnet` from `/supported` / disable the Stellar provider so `POST /settle` for Stellar fails closed. (Lowest-friction mitigation given the live exposure.)
3. **Ship `fixes/01` (P0)** before re-funding Stellar — see fix doc for the exact Check 5a + `SourceAccount` rejection changes and the regression test.
4. **Rotate the premium RPC API key(s)** after `fixes/07` lands (the key may already have been exfiltrated via escrow/upto error responses — assume compromise). Update the value in AWS Secrets Manager `facilitator-rpc-mainnet`, not the task definition.
5. **Schedule the four P1 merchant-fund-loss fixes** (`fixes/02`–`fixes/05`): Solana settlement-account `pay_to` binding, Sui coin-type binding, Algorand recipient/amount binding, ERC-8004 proof gating.
6. **Hoist compliance screening** (`fixes/06`) to a single pre-dispatch choke point so escrow/commerce/upto/refund are screened — sanctions/regulatory exposure.

---

## Residual risk & what was NOT covered

- **No live/dynamic exploitation was performed.** Every confirmed bug was verified by reading source at the cited `file:line` under two independent lenses; the Stellar P0's *impact* was corroborated by reading the **public** live wallet balance and `/supported`, but **no exploit transaction was broadcast**. The drain is asserted from code + protocol semantics, not a devnet repro.
- **Soroban `SourceAccount` on-chain semantics** (that the facilitator's source-account signature satisfies `require_auth(facilitator)`) are asserted from protocol knowledge, not reproduced on a Stellar devnet. The fix is correct regardless (reject `SourceAccount` for payment entries), but a devnet repro should accompany the fix's regression test.
- **P2/P3 findings were NOT adversarially verified** — only P0/P1 went through the verifier. The P2/P3 appendix is a deduped *report*, not a confirmation; some entries may be partly mitigated by controls not surfaced in the finder's slice. Re-verify before acting on any P2/P3 in isolation.
- **Contracts were reviewed by reading, not by running echidna.** Notably, finder `contracts-conditions-fees-echidna` flagged that the existing echidna no-double-spend/monotonic invariants are **vacuous** (wrong-salt `PaymentInfo` reconstruction → escrow always reverts), so the on-chain safety properties are currently *unproven by the test suite*. A corrected echidna harness run is required to claim the escrow contracts safe.
- **No automated dependency CVE gate** exists; the 20-advisory RustSec set was enumerated statically and judged not remotely exploitable from the public payment surface, but this judgment was not fuzzed.
