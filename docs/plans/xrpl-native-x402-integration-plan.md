# XRPL Native x402 Integration Plan — Mainnet #20

**Date:** 2026-05-29
**Status:** DECISIONS LOCKED - implementation drafted (UNCOMPILED, pending user `cargo build --features xrpl`)
**Goal:** Add XRP Ledger (XRPL) as the 20th supported **mainnet** in the x402-rs facilitator.
**Deliverable of this session:** research + plan + drafted (uncompiled) implementation.

---

## TL;DR / Decision

| Question | Answer |
|---|---|
| **Which XRP?** | **XRPL native** (the XRP Ledger L1), NOT XRPL EVM sidechain. |
| **Is it feasible now?** | **YES.** The Dec-2025 "not feasible, needs Hooks" verdict is **obsolete**. |
| **What changed?** | t54 Labs proved the **presigned-`Payment` scheme** works on XRPL mainnet — no Hooks, no smart contracts, no amendments. |
| **Why not XRPL EVM?** | No EIP-3009 stablecoin exists on XRPL EVM. Our standard flow is hard-blocked there. |
| **Build vs proxy?** | **Build our own native Rust module** following t54's published `xrpl-scheme` for client/SDK interoperability. |
| **Effort** | ~1,600–2,150 LoC (new `NetworkFamily::Xrpl` + `src/chain/xrpl.rs`) + integration boilerplate. Main risk: `xrpl-rust` crate maturity. |

---

## 0. Provisioned (2026-05-29)

State already provisioned outside the code, captured here so it is not re-derived or re-invented.

### Facilitator XRPL addresses (PUBLIC — r-addresses, safe to record)

| Environment | r-address | AWS secret (us-east-2) |
|---|---|---|
| Mainnet | `rfADKkVXBNqK3z72tVSS3LVzAR3psYkonp` | `facilitator-xrpl-keypair-mainnet` |
| Testnet | `rGhTioKAFHe75KgVnQtacRiKFuPv28Wbwk` | `facilitator-xrpl-keypair-testnet` |

- Both r-addresses are PUBLIC identifiers only; the keypairs themselves live in AWS Secrets Manager (region **us-east-2**) and are never committed.
- Addresses corroborated by authoritative in-repo sources: `lambda/balances/handler.py` (canonical wallet source), `config/supported_tokens.json`, and `docs/handoffs/2026-05-29-moonpay-cli-xrp-contribution.md`.
- AWS secrets `facilitator-xrpl-keypair-mainnet` and `facilitator-xrpl-keypair-testnet` have been **created** in us-east-2 (matches the `*-keypair-mainnet`/`*-keypair-testnet` naming used for Solana/Sui). Loader wiring follows the existing per-network AWS Secrets Manager pattern.

### 4 locked decisions

1. **Assets:** ship **RLUSD + USDC + XRP** on XRPL (not XRP-only). RLUSD/USDC are XRPL issued tokens; XRP is native. (Resolves §8 Q1 — supersedes the earlier "XRP-first" recommendation.)
2. **Keys in AWS:** facilitator XRPL keypairs are stored in **AWS Secrets Manager** (us-east-2), not in `.env` or any committed file. (Resolves §8 Q2 — a funded facilitator r-address is provisioned for the settlement-account/RLUSD-trust-line path, not just pure relay.)
3. **Scheme:** adopt **t54's `xrpl:0` CAIP-2 + `exact` scheme** so existing `x402-xrpl` clients interoperate with our facilitator. (Resolves §8 Q3.)
4. **XRPL EVM stub removed:** the dead `XrplEvm` stub (chain ID 1440002, unsettleable token) is **removed** rather than re-pointed. (Resolves §8 Q4.)

> **Status of these decisions:** locked. The corresponding implementation is **drafted but UNCOMPILED** — do not assume it builds until the user runs `cargo build --features xrpl`.

---

## 1. The two paths investigated

The user asked us to evaluate **both** XRPL native and XRPL EVM and recommend which becomes #20.

### Path A — XRPL EVM sidechain (REJECTED for now)

The repo already has a stub `Network::XrplEvm` but it is **broken and blocked**:

- **Wrong chain ID.** Our stub uses **1440002**, which is the *dead legacy devnet* of the old architecture. The current production chain is:
  - Mainnet: **1440000** (`https://rpc.xrplevm.org`, `https://explorer.xrplevm.org`)
  - Testnet: **1449000**
  - Devnet: **1449900**
  - The stub at `0xDaF4556169c4F3f2231d8ab7BC8772Ddb7D4c84C` / 1440002 should be treated as dead. (`src/network.rs`, `src/chain/evm.rs:128-131`)
- **No EIP-3009 stablecoin exists on XRPL EVM mainnet.**
  - **RLUSD** on EVM supports only **EIP-2612 `permit`**, NOT EIP-3009 `transferWithAuthorization`. (Ripple's own `RLUSD-Implementation` design doc.)
  - USDC/USDT present only as **Axelar/Squid bridged wrappers** (axlUSDC, bridged USDT) — plain ERC-20s, no EIP-3009.
  - Circle deployed **native USDC on XRPL L1** (June 2025), NOT on the EVM sidechain. XRPL EVM is not on CCTP's chain list.
- **Conclusion:** Our standard `transferWithAuthorization` settlement primitive has no eligible token here. Integrating would require a *custom* EIP-2612-permit + relayed-`transferFrom` code path (Permit2-style) — a separate project, and it would not be "real XRP."

**Cleanup item regardless of this plan:** the existing `XrplEvm` stub at chain ID 1440002 is on a dead network and advertises a USDC that has no EIP-3009 support. It should either be re-pointed to 1440000 *with* a working token path or removed from `/supported` to avoid clients attempting (and failing) settlements.

### Path B — XRPL native via presigned `Payment` (RECOMMENDED — this is #20)

This is what `xrpl-x402.t54.ai` does and what the user's lead pointed at.

**The model (no Hooks required):**
1. Resource server returns `402` with x402 v2 `PAYMENT-REQUIRED` challenge (network `xrpl:0`, scheme `exact`, amount, `payTo`, `invoiceId`).
2. Buyer's wallet **signs a complete, standard XRPL `Payment` transaction** off-ledger (specific amount, destination = `payTo`, `LastLedgerSequence` for expiry, invoice binding via `Memos`/`InvoiceID`), and sends the **signed tx blob** in the x402 payload.
3. Facilitator `/verify` decodes the signed blob and runs the checks (see §4).
4. Facilitator `/settle` submits the already-signed blob to a `rippled` node and returns the validated tx hash.

**Why this sidesteps the old blocker:** the buyer signs a *whole transaction*, not an "authorization to be wrapped later." No smart contract, no Hooks amendment, no EIP-3009 equivalent needed. The signature already on the tx authorizes the transfer; any account (the facilitator) can submit it.

**Fee model note (important for wallet funding):** the tx `Fee` is drawn from the **buyer's** XRP balance, set at signing time. In the pure presigned-`Payment`-to-merchant flow the facilitator is just a relay — **it does NOT need a funded XRPL wallet to settle**. (Contrast with EVM where the facilitator pays gas.) A facilitator XRPL r-address may still be wanted for a settlement-account/sweep variant or for RLUSD trust-line setup, but the basic flow needs none.

---

## 2. What t54 built (our interop reference)

`xrpl-x402.t54.ai` is a **hosted, third-party** x402 facilitator by **t54 Labs** (not Ripple). Key verified facts:

- Hosted endpoint: `https://xrpl-facilitator-mainnet.t54.ai`, **mainnet-live**.
- Standard x402 interface: `/supported`, `/verify`, `/settle`; x402 v2 headers (`PAYMENT-REQUIRED`, `PAYMENT-SIGNATURE`, `PAYMENT-RESPONSE`).
- Scheme: `"exact"`, `x402Version = 2`.
- **CAIP-2 namespace `xrpl:{network_id}`**: mainnet `xrpl:0`, testnet `xrpl:1`, devnet `xrpl:2`. (This is a t54 convention aligned with the registered `xrpl` ChainAgnostic namespace; it is NOT in the upstream Coinbase x402 spec — XRPL is an independent extension.)
- Assets: **XRP, RLUSD, USDC** (the latter two as XRPL issued/IOU tokens).
- Client SDKs published: npm `x402-xrpl` (Express + client), pip `x402-xrpl` (with `xrpl-py`). The **facilitator server code itself is NOT open source** — only the hosted endpoint.
- Settlement response shape:
  ```json
  { "success": true, "transaction": "<txhash>", "network": "xrpl:1", "payer": "r..." }
  ```

**Strategic choice for us:** adopt t54's `xrpl-scheme` envelope (CAIP-2 `xrpl:0`, scheme `exact`, the verification checks below) so any `x402-xrpl` client/SDK can point at OUR facilitator interchangeably. We become a true settlement intermediary and a genuine #20 mainnet rather than proxying t54.

---

## 3. Verification checks (`/verify`) — port of t54's 9-step scheme

For each incoming signed blob, verify:

1. **Envelope** — `x402Version == 2`, `scheme == "exact"`, network supported (`xrpl:0`).
2. **Decode** — hex `signedTxBlob` → XRPL binary codec → tx object.
3. **Type** — must be `TransactionType == Payment`.
4. **Destination** — `tx.Destination == payTo`.
5. **Network binding** — `tx.NetworkID` matches the CAIP-2 network (mainnet rules).
6. **Amount** — `DeliverMax`/`Amount` matches required amount (drops for XRP; issued-token amount object for RLUSD/USDC).
7. **Expiry** — `LastLedgerSequence` present and within limits (native self-expiry).
8. **Invoice binding** — `Memos` (`MemoData = HEX(UTF-8(invoiceId))`) or `InvoiceID` (`SHA256(invoiceId)`) binds to the invoice. **Required** — without it a valid payment can be replayed against multiple invoices.
9. **Policy** — fee limits, no partial payments (`tfPartialPayment` rejected), no cross-currency.
10. **Signature** — verify `TxnSignature` against `SigningPubKey` (ed25519 `0xED…` or secp256k1), single-sign prefix `0x53545800`.

`/settle` = submit blob via `submit`/`submit_and_wait`, poll `tx` for validation, return `{ success, transaction: <hash>, network, payer }`.

---

## 4. Assets to support

| Asset | Form | Decimals / amount format | Identifier |
|---|---|---|---|
| **XRP** | native | 6 dp — integer **drops** as string (1 drop = 1e-6 XRP) | — |
| **RLUSD** | XRPL issued token | issued-token format, up to 15 sig digits, **decimal strings** (NOT fixed 6dp) | issuer `rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De`, currency hex `524C555344000000000000000000000000000000` |
| **USDC** | XRPL issued token (Circle native on L1, June 2025) | decimal-string issued-token format | issuer `rGm7WCVp9gb4jZHWTEtGUr4dd74z2XuWhE` (verify on livenet before use) |

**Trust-line caveat:** issued tokens (RLUSD/USDC) require the *recipient* (`payTo`) to hold a `TrustSet` trust line to the issuer, each consuming owner reserve. Verification should account for trust-line / `RequireAuth` failures. Native XRP needs no trust line. **MVP recommendation: ship XRP first, add RLUSD/USDC issued-token support in a second pass** (issued-token amount serialization + trust-line handling is the fiddly part).

---

## 5. Rust tooling & risk

- **`xrpl-rust` (XRPLF official, v1.1.0, ~Apr 2026)** — 100% Rust, ed25519 + secp256k1 signing, binary codec, JSON-RPC + WebSocket, `submit`/`submit_and_wait`. **~41 GitHub stars — small/young.**
- **Risks to de-risk before committing:**
  1. Confirm the crate can **decode an externally-signed tx blob** and **verify the embedded signature** (our flow never signs — it validates buyer signatures). This is the load-bearing capability.
  2. Validate binary serialization of **issued-token amounts**, X-addresses, and `Memos` against `xrpl-py`/`xrpl.js` golden vectors before trusting settlement.
  3. Check `no_std`/async assumptions vs our tokio stack.
- Alternatives if `xrpl-rust` falls short: `xrpl_http_client` (lighter JSON-RPC), or implement blob decode + signature verify manually against the binary-format spec.
- **Gate the module behind a Cargo feature** (`--features xrpl`), like `algorand`/`sui`.

---

## 6. Integration surface (files to touch)

Modeled on the Stellar/NEAR/Algorand non-EVM family pattern.

**Core:**
- `src/network.rs`
  - Add `Network::Xrpl` + `Network::XrplTestnet` enum variants.
  - Add `NetworkFamily::Xrpl` (feature-gated) and the `From<Network>` arms (see `src/network.rs:262-324`).
  - Add to all `variants()` arrays, `is_testnet()`, and CAIP-2 mapping (`xrpl:0` / `xrpl:1`).
  - Add XRP/RLUSD/USDC token deployment statics + getter arms.
- `src/chain/mod.rs` — add `XrplProvider` to `NetworkProvider` enum + the 3 dispatch impls (`FromEnvByNetworkBuild`, `NetworkProviderOps`, `Facilitator`).
- `src/chain/xrpl.rs` — **new file (~1,500–2,000 LoC)**: `XrplChain`, `XrplProvider`, `verify()`, `settle()`, `supported()`, blob decode + signature verify, rippled JSON-RPC/WS client.
- `src/from_env.rs` — `ENV_RPC_XRPL` / `ENV_RPC_XRPL_TESTNET` (+ optional `XRPL_PRIVATE_KEY*` only if a settlement-account variant is built; basic relay flow needs none).
- `src/types.rs` — add `MixedAddress::Xrpl(String)` for r-addresses.
- `Cargo.toml` — optional `xrpl-rust` dep behind `xrpl` feature.

**Peripheral (per CLAUDE.md "Adding a New Network" checklist):**
- `src/handlers.rs` + `static/xrpl.png` logo + handler.
- `static/index.html` network card + i18n (EN/ES).
- `config/supported_tokens.json` — add XRPL chain + tokens + facilitator address (copy from authoritative source, never type from memory).
- `lambda/balances/handler.py` — add XRPL balance config.
- `README.md` — bump network count to 20, update stablecoin matrix (`python scripts/stablecoin_matrix.py`).
- SDKs: `uvd-x402-sdk-python`, `uvd-x402-sdk-typescript` — add `xrpl:0` network.
- `src/openapi.rs` — only if new endpoints (none expected).

---

## 7. Phased rollout

1. **Phase 0 — De-risk tooling (½–1 day).** Spike: decode a real mainnet signed `Payment` blob with `xrpl-rust`, verify its signature, confirm checks 2/3/10 work. Go/no-go on the crate.
2. **Phase 1 — XRP-only native module.** New `NetworkFamily::Xrpl`, `src/chain/xrpl.rs`, verify/settle for native XRP using presigned `Payment`. Testnet (`xrpl:1`) first.
3. **Phase 2 — Issued tokens.** Add RLUSD + USDC (issued-token amount serialization, trust-line handling).
4. **Phase 3 — Interop + frontend.** CAIP-2, `/supported`, landing page card, `config/supported_tokens.json`, SDKs. Confirm an `x402-xrpl` client can pay through our endpoint.
5. **Phase 4 — Mainnet (#20).** Switch to `xrpl:0`, deploy, verify in `/supported`, update README count to 20.

**Definition of done:** an `x402-xrpl` (or our SDK) client completes a real XRP payment on XRPL mainnet through `facilitator.ultravioletadao.xyz`, `/supported` lists `xrpl:0`, and `[.kinds[].network] | unique | length` of mainnets == 20.

---

## 8. Open questions for the user

1. **MVP asset:** XRP-only first (simplest), or block on RLUSD/USDC too? (Recommend XRP-first.)
2. **Facilitator wallet:** the pure relay flow needs no XRPL wallet. Do we want a settlement-account/sweep variant (requires a funded r-address + trust lines)?
3. **Interop posture:** confirm we adopt t54's `xrpl:0` + scheme so existing `x402-xrpl` clients work against us (recommended), vs. inventing our own envelope.
4. **XRPL EVM stub:** fix (re-point 1440002 → 1440000) or remove from `/supported`? It currently advertises an unsettleable token.

---

## Sources

- t54 scheme: `https://xrpl-x402.t54.ai/docs/xrpl-scheme`, hosted `https://xrpl-facilitator-mainnet.t54.ai`
- XRPL CAIP-2: `https://namespaces.chainagnostic.org/xrpl/caip2`
- RLUSD: `https://docs.ripple.com/stablecoin/developer-resources/rlusd-on-the-xrpl/` (issuer `rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De`)
- `xrpl-rust`: `https://github.com/XRPLF/xrpl-rust`, `https://docs.rs/xrpl-rust`
- XRPL EVM mainnet: `https://docs.xrplevm.org/pages/operators/resources/networks` (1440000), RLUSD EVM design `https://github.com/ripple/RLUSD-Implementation`
- Prior internal research: `docs/NON_EVM_CHAIN_RESEARCH.md` (Dec 2025 — now superseded re: feasibility)
