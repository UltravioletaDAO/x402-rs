# Handoff: authCapture Migration Planning

**Date**: 2026-04-28
**From**: Windows session (Claude Code)
**Continue on**: WSL session
**Status**: Pre-implementation — awaiting decisions before any code changes

---

## TL;DR

Coinbase x402-foundation maintainers (`fabrice-cheng`, `phdargen`) gave Ali (`A1igator`) final feedback on PR [x402-foundation/x402#1425](https://github.com/x402-foundation/x402/pull/1425). The `commerce` scheme is being **renamed `authCapture`** with significant structural changes. Ali messaged us 4/28 01:23 saying he's implementing the feedback. User responded "on it" at 16:08. We need to mirror those changes in the facilitator + SDKs.

The conversation in this Windows session collected the full context but **stopped before writing any code**. There are 5 open decisions blocking implementation. Resume by reviewing those and producing a migration plan, then code.

---

## What triggered this

Source: `.unused/ali-x402r-standard.txt` (chat log + full PR #1425 conversation, ~880 lines, gitignored — read it directly to see Ali's verbatim message and the Coinbase review).

Key Discord/chat excerpts from Ali (4/28):
1. "Coinbase maintainers came back to us with scheme feedback that we're implementing"
2. "It's mostly name changes. Few major things though outside that:
   - They want canonical base commerce-payments via create2 which needs same bytecode so we have to **drop SKALE support** until they upgrade their EVM.
   - They want **permit2** so you'll have **BSC/Tempo** support soon!
   - **Partial refund in escrow is dropped** but the same functionality can basically be gotten via partial release and then doing a full refund"
3. "We're getting pretty close to having it merged and it can become standard across the ecosystem"

Coinbase reviewer (`fabrice-cheng`) feedback summary in PR #1425:
- Trying to be consistent with x402 naming convention (instead of reusing AuthCaptureEscrow protocol naming).
- Defaulting to a canonical contract for simplicity (similar to Permit2 and Upto x402 specs).
- Two-phase payments: client authorizes a hold (escrowed on-chain), captureAuthorizer captures some/all later.

---

## Concrete spec changes (current → new)

### Scheme name
- **Current production (v1.43.0)**: accepts `"escrow"` and `"commerce"` (alias)
- **New**: `"authCapture"` is the canonical name

### `extra` field renames
| Current (commerce) | New (authCapture) |
|---|---|
| `operatorAddress` | `captureAuthorizer` |
| `authorizationExpiry` | `captureDeadline` |
| `refundExpiry` | `refundDeadline` |
| `preApprovalExpiry` | derived: `now + maxTimeoutSeconds` |
| `feeReceiver` | `feeRecipient` |
| `escrowAddress` / `tokenCollector` | **REMOVED** — universal CREATE2 constants |

### Universal CREATE2 constants (NEW)
- `AUTH_CAPTURE_ESCROW_ADDRESS` — same on every EVM chain via CREATE2
- `EIP3009_TOKEN_COLLECTOR_ADDRESS` — universal collector for EIP-3009 path
- `PERMIT2_TOKEN_COLLECTOR_ADDRESS` — universal collector for Permit2 path

Ali has not shared the new CREATE2 addresses yet (they will differ from current CREATE3 addresses like `0xe050bB89eD43BB02d71343063824614A7fb80B77`).

### `captureAuthorizer` is constrained by `onlySender(paymentInfo.operator)` — CRITICAL

The `AuthCaptureEscrow.authorize()` function has an `onlySender(paymentInfo.operator)` gate. Translation: **`msg.sender` of the `authorize()` tx must equal `paymentInfo.operator` (a.k.a. `extra.captureAuthorizer`)**. Source: Ali's last comment on PR #1425 (1h before chat snapshot), referencing https://github.com/base/commerce-payments/blob/main/docs/operations/Authorize.md.

In the x402 gasless flow the merchant does NOT send the on-chain tx — the facilitator does. So the `captureAuthorizer` field can only be one of two things:

**Model A — facilitator-as-operator (phdargen's proposal):**
- `captureAuthorizer` = our facilitator wallet (mainnet `0x103040545AC5031A11E8C03dd11324C7333a13C7`, testnet `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`)
- Facilitator wallet calls `AuthCaptureEscrow.authorize()` directly → `msg.sender` matches → gate passes
- Simple, but the facilitator gains full power over `capture()`, `void()`, `refund()` — Ali objected: "Make the facilitator heavily trusted which goes against x402 principles"

**Model B — smart-contract-operator (Ali's x402r.org approach, what we already use for `commerce`):**
- `captureAuthorizer` = a `PaymentOperator` contract deployed via x402r factory
- Facilitator wallet calls a function on the `PaymentOperator` contract; that contract calls `AuthCaptureEscrow.authorize()` → `msg.sender` = contract address = `paymentInfo.operator` → gate passes
- The contract internally enforces who can `capture`/`void`/`refund` (merchant, facilitator, on-chain arbiter, chained conditions)
- Trust-minimized; requires per-config deploy + extra hop. This is what production v1.43.0 already does.

**Model C — merchant-as-operator: IMPOSSIBLE in the x402 flow** without breaking gasless-ness (merchant would have to send the authorize tx themselves with their own gas).

Hence the merchant **cannot** be `captureAuthorizer` directly. The merchant address goes in `payTo`. The `captureAuthorizer` is either us (Model A) or a contract Ali's factory deploys (Model B).

Naming gripe Ali raised: "captureAuthorizer" is misleading because the entity also handles `authorize`, not just capture. fabrice-cheng acknowledged: kept for consistency with `payerAuthorizer` / `receiverAuthorizer` in x402BatchSettlement.

### `assetTransferMethod` (NEW field)
- `"eip3009"` (default) — current behavior
- `"permit2"` — new, enables BSC + Tempo

Two distinct payload shapes:

**EIP-3009 payload (similar to current)**:
```json
{
  "x402Version": 2,
  "accepted": { "scheme": "authCapture", ... },
  "payload": {
    "signature": "0x...",
    "authorization": {
      "from": "0xPayer",
      "to": "0xEIP3009TokenCollector",   // universal constant, NOT merchant
      "value": "50000000",
      "validAfter": "0",
      "validBefore": "1744243200",         // = preApprovalExpiry = now + maxTimeoutSeconds
      "nonce": "0xa1b2c3..."               // deterministic, see below
    }
  }
}
```

**Permit2 payload (new shape, no witness)**:
```json
{
  "payload": {
    "signature": "0x...",
    "permit2Authorization": {
      "from": "0xPayer",
      "permitted": { "token": "0x...", "amount": "50000000" },
      "spender": "0xPermit2TokenCollector",  // universal constant
      "nonce": "123456789",                   // uint256(deterministic hash)
      "deadline": "1744243200"
    }
  }
}
```

Note: authCapture's permit2 uses `permitTransferFrom` (no witness), unlike `exact` scheme which uses `permitWitnessTransferFrom`. Don't reuse our existing `src/upto/permit2.rs` blindly — it's for the `upto` scheme.

### Deterministic nonce (NEW)
Was random in current escrow flow. Now:
```
paymentInfoWithZeroPayer = PaymentInfo {
    operator:            extra.captureAuthorizer,
    payer:               0x0000000000000000000000000000000000000000,
    receiver:            payTo,
    token:               asset,
    maxAmount:           amount,
    preApprovalExpiry:   now + maxTimeoutSeconds,    // client picks
    authorizationExpiry: extra.captureDeadline,
    refundExpiry:        extra.refundDeadline,
    minFeeBps:           extra.minFeeBps,
    maxFeeBps:           extra.maxFeeBps,
    feeReceiver:         extra.feeRecipient,
    salt:                extra.salt,
}
innerHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfoWithZeroPayer))
nonce     = keccak256(abi.encode(chainId, AUTH_CAPTURE_ESCROW_ADDRESS, innerHash))
```

Payer is zeroed so the nonce is identical regardless of who pays. Client computes nonce before knowing its own address.

### `autoCapture` flag (replaces `settlementMethod`)
- `extra.autoCapture: true` → calls `charge()` (1 tx, authorize+capture combined)
- `extra.autoCapture: false` (default) → calls `authorize()` (2-step flow, capture later)

This replaces the discarded `settlementMethod: "authorize" | "charge"` field.

### Partial refund REMOVED
Functionality preserved via "partial release + full refund". If we have partial-refund code paths, they get deleted/refactored.

### Salt — STILL IN DEBATE (1h before chat snapshot)
Last exchange: Ali asked if salt should be client-generated and moved out of `PaymentRequirements`. fabrice-cheng said "I'm in favor of dropping it in PaymentRequirements, will have to be passed in via PaymentPayload". Watch for resolution before implementing.

### Drop SKALE
Coinbase requires identical bytecode across all EVM chains via CREATE2. SKALE doesn't support Cancun opcodes, so **SKALE is dropped from authCapture support**. Implications:
- Remove `Network::SkaleBase` and `Network::SkaleBaseSepolia` from `ESCROW_NETWORKS` in `src/payment_operator/addresses.rs`
- The 0.10 USDC stuck in 2 lockboxes on SKALE Base via operator `0x28c23AE8f55aDe5Ea10a5353FC40418D0c1B3d33` — likely unrecoverable until SKALE upgrades EVM
- All `docs/MESSAGE_FOR_ALI_SKALE_*.md` and `docs/HANDOFF_RELEASE_DEBUG.md` become historical archive
- `docs/plans/x402r-skale-master-plan.md` and `docs/plans/x402r-skale-final-integration-plan.md` obsolete

---

## State of the facilitator before this work

### Production: v1.43.0 (deployed)
- `Scheme::Commerce` deserialization works
- `/supported` advertises `commerce` on 11 escrow networks (14 commerce + 14 escrow entries)
- Verify/settle route both via `is_escrow_scheme()`
- CREATE3 canonical addresses fix in `/supported` (commit `8e49004`)
- Merchant-provided `tokenCollector`/`escrow` passed to contract calls (commit `ff2cc67`)

### Recent commits
```
ff2cc67 fix: pass merchant-provided tokenCollector/escrow to contract calls
8e49004 fix: use CREATE3 canonical addresses for commerce scheme in /supported
758c2de fix: accept CREATE3 canonical addresses on all escrow networks
02c29bb feat: add commerce scheme support (x402r Execution Market integration)
3844bae feat: add Hedera network support (ERC-8004 + x402 feasibility)
```

### Working tree (uncommitted)
Almost all diffs are `cargo fmt` line-break churn (CRLF warnings). **Real changes**:

| File | Real change |
|---|---|
| `src/network.rs` | Tests exclude Hedera from USDC checks (Hedera is ERC-8004-only) + BSC USDC has 18 decimals |
| `src/erc8004/mod.rs` | Tests updated: 16 EVM + 2 SKALE + 2 Hedera + 2 Solana = 22 networks |

Other modified files (`src/chain/evm.rs`, `src/chain/solana.rs`, `crates/x402-reqwest/src/chains/*.rs`, `src/upto/*.rs`, `src/erc8004/solana.rs`, `src/payment_operator/addresses.rs`) are pure formatting — no logic changes.

`docs/handoffs/` is untracked (contains `commerce-scheme-sdk-handoff.md` from 4/05).

`.claude/.irc-nick` was deleted.

### Open SDK handoff
`docs/handoffs/commerce-scheme-sdk-handoff.md` was the last batch — Python and TypeScript SDKs were asked to widen `Scheme` literal to accept `"commerce"`. Status of those SDK PRs is **unknown from this session** — verify before authoring a new handoff for `authCapture`.

---

## 5 OPEN DECISIONS (must answer before coding)

These are blocking implementation. The previous session left them with the user.

### Decision 1: Implement now or wait for PR merge?
Ali is still iterating with `fabrice-cheng` (salt placement, captureAuthorizer naming). Last comment was 1h before the chat snapshot. **Risk of churn** if we implement before merge. **Risk of delay** to Execution Market if we wait.

Recommendation: write the migration plan now (no code), wait for merge to start coding, **but**: drop SKALE and clean working tree are independent of the merge — those can ship immediately.

### Decision 2: Backward compatibility window for `commerce`/`escrow`
Production is live with `"commerce"` and `"escrow"`. Execution Market is using `"commerce"`. Options:
- (a) Accept all three (`escrow`, `commerce`, `authCapture`) indefinitely with `is_escrow_scheme()` covering all
- (b) Deprecate `escrow`+`commerce` with a sunset date
- (c) Hard cut: only `authCapture` after vN+1

Recommendation: (a) initially, deprecate later when SDKs and Execution Market migrate.

### Decision 3: SKALE rollback now or after auth-capture migration?
SKALE drop is **decoupled** from authCapture spec. Can ship today. The release() bug investigation is moot.

Recommendation: drop SKALE now, separate commit, separate version bump.

### Decision 4: Permit2 scope
Existing `src/upto/permit2.rs` is for the `upto` scheme — **different signature path** (`permitWitnessTransferFrom`). The new authCapture permit2 uses `permitTransferFrom` with no witness. Need a new module, probably `src/payment_operator/permit2.rs`, sharing only low-level types.

Recommendation: scope as Phase 2 after EIP-3009 authCapture works.

### Decision 5: CREATE2 addresses dependency on Ali
We don't have the new CREATE2 addresses. Ali has to deploy `AuthCaptureEscrow` + universal token collectors via CREATE2 first.

Recommendation: ask Ali to share addresses + deployment tx hashes as soon as he has them. Block code merge on having real addresses.

### Decision 6: `captureAuthorizer` architecture — Model A vs Model B
See spec section "`captureAuthorizer` is constrained by `onlySender(paymentInfo.operator)`" above. We must choose, per network or globally:

- **Model A (facilitator-as-operator):** simple, fewer contracts to deploy, but facilitator becomes a heavily trusted party with unilateral capture/void/refund power. Goes against x402 trust-minimization principle. phdargen prefers it for simplicity.
- **Model B (smart-contract-operator):** what production v1.43.0 already does for `commerce`. Trust-minimized via x402r `PaymentOperator` contracts. Requires Ali's factory deploy per-merchant or per-config. Extra hop in every settlement.

Cross-cutting consequences:
- Model B requires Ali's factory to be deployed via CREATE2 on the new spec's address layout (not the current CREATE3 layout). Block on Ali confirming.
- Model A makes our facilitator wallet a high-value target — every fund recovery path runs through it. Plus the wallet rotation playbook (`docs/WALLET_ROTATION.md`) becomes more critical.
- Mixing models per-network is technically possible but adds verify/settle branching complexity.

Recommendation: stick with Model B (status quo) for backward compat with Execution Market, but ask Ali if he plans to ship a default `PaymentOperator` per network alongside the canonical CREATE2 escrow — that would make Model B as ergonomic as Model A for new merchants without sacrificing trust-minimization.

---

## Key files to read on resume

In order of importance:

1. **`.unused/ali-x402r-standard.txt`** — full source: Ali's chat message + the entire PR #1425 conversation. Read first to verify nothing in this handoff drifted.
2. `docs/plans/commerce-scheme-master-plan.md` — what we did to get to v1.43.0; useful as template for the next plan.
3. `docs/handoffs/commerce-scheme-sdk-handoff.md` — what the SDKs were asked for; needs an authCapture follow-up.
4. `src/types.rs` — `Scheme` enum, where `Commerce` variant lives.
5. `src/payment_operator/operator.rs` — `is_escrow_scheme()` and `ESCROW_SCHEME`/`COMMERCE_SCHEME` constants.
6. `src/payment_operator/addresses.rs` — `OperatorAddresses::for_network()` + `ESCROW_NETWORKS` constant + `create3` module (will need a `create2` sibling).
7. `src/handlers.rs` — verify/settle handlers, scheme routing.
8. `src/facilitator_local.rs` — `/supported` builds escrow + commerce entries (~line 219).
9. `src/openapi.rs` — Swagger docs for scheme.
10. `abi/PaymentOperator.json` — current ABI (likely needs update once Ali ships new contracts).

For SKALE rollback (Decision 3):
- `src/payment_operator/addresses.rs` — `ESCROW_NETWORKS`, `Network::SkaleBase`/`SkaleBaseSepolia` arms, `create3` module (used only by SKALE today)
- `src/network.rs` — `Network::SkaleBase`/`SkaleBaseSepolia` enum variants (probably keep, just remove from escrow)

---

## Verify before coding (quick commands)

```bash
# Confirm current production state
curl -s https://facilitator.ultravioletadao.xyz/version
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].scheme] | unique'
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[] | select(.scheme == "commerce")] | length'

# Check PR status
gh pr view 1425 --repo x402-foundation/x402 --json state,merged,updatedAt,reviews

# Inspect what will be in the next commit (formatting + Hedera/BSC test changes)
git diff -w --stat

# See what files reference SKALE escrow specifically
grep -rn "SkaleBase" src/payment_operator/ src/erc8004/
```

---

## Suggested order of work on resume

1. Read `.unused/ali-x402r-standard.txt` to confirm context. Check PR #1425 status — if merged, naming/structure is final; if open, watch for new commits since 4/28.
2. Resolve the 5 open decisions with the user.
3. Independent quick wins (no spec dependency):
   - Commit working-tree real changes (Hedera/BSC USDC tests + Hedera test count update). Skip the cargo-fmt-only files or commit them separately as `style:`.
   - Drop SKALE from `ESCROW_NETWORKS` (small PR, version bump, deploy).
4. Once PR #1425 merges, write `docs/plans/authcapture-migration-plan.md` mirroring the structure of `commerce-scheme-master-plan.md`.
5. Implement Phase 1: rename `Scheme::Commerce` → handle `Scheme::AuthCapture` (with backward compat per Decision 2). Update `/supported`, openapi, handlers, operator.
6. Phase 2: rename `extra` fields, deterministic nonce, universal token collector constants. This is the biggest chunk — touches verify/settle.
7. Phase 3: Permit2 path (`assetTransferMethod = "permit2"`).
8. Phase 4: handoff to Python + TypeScript SDKs (mirror `commerce-scheme-sdk-handoff.md`).

---

## Notes & gotchas

- **Don't reuse `src/upto/permit2.rs`** for authCapture's permit2 — different signature primitive.
- **The `salt` field is in flux** — last seen in PR #1425 conversation 1h before chat snapshot. Re-check PR before encoding nonce logic.
- **Universal CREATE2 addresses are not yet known** — block on Ali for those.
- **Execution Market is in production using `commerce`** — coordinate cutover with Ali so EM doesn't break.
- **0.10 USDC stuck on SKALE Base** is acceptable cost; do not invest more debug time.
- **CREATE3 plan is dead** — Coinbase wants CREATE2 with identical bytecode. `docs/plans/create3-full-migration-plan.md` and the SKALE plans become historical.
- **CLAUDE.md: never use emojis in Rust code.** Already covered, but worth flagging for the test/string changes coming.
- **`.unused/` is gitignored and contains secrets in other files** — only `ali-x402r-standard.txt` was read for this handoff. Don't grep blindly.

---

## Pending message to Ali (not sent)

When the migration plan is in place, send Ali a coordination message asking for:
1. CREATE2 addresses for `AuthCaptureEscrow`, `EIP3009_TOKEN_COLLECTOR`, `PERMIT2_TOKEN_COLLECTOR` (with deployment tx hashes).
2. Final ABI for `PaymentOperator` after rename + autoCapture path.
3. Notification on PR #1425 merge (or invite to be a collaborator).
4. Confirmation on salt placement (PaymentRequirements vs PaymentPayload).
5. Plan for Execution Market cutover from `commerce` to `authCapture`.
