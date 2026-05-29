# Upstream Correlation Report — Security Audit Findings vs `x402-rs/x402-rs`

**Date:** 2026-05-19
**Our fork HEAD:** `686683a` on `main`
**Upstream HEAD:** `980ad88` on `upstream/main` (v1.4.10)
**Source audit:** `docs/reports/2026-05-19-security-audit.md`
**Method:** `git fetch upstream` + file/commit search across upstream tree.

## TL;DR — Headcount

| Status | Count | Meaning |
|---|---|---|
| Downstream-only | **12** | Feature/chain doesn't exist upstream; we own the fix entirely |
| Not addressed upstream | **5** | Upstream has the same defect (or equivalent untouched code) |
| Partial (pattern portable) | **3** | Upstream has scaffolding we can port to harden ours |

**Hard truth:** upstream cannot help us with the majority of our blockers. The reason isn't that upstream is better hardened — it's that **upstream doesn't have NEAR, Stellar, Sui, Algorand, ERC-8004, settlement-account, discovery, commerce, or any AWS infrastructure**. The drain primitives B1-B5 + B7 + F1 + F3 + F5 + F6 + F9 + F10 exist in code we wrote, not code we inherited. Where upstream and our fork share defects (CORS-any, no rate limit, root container — B8 + B10), upstream is **just as broken**.

## 20-Row Correlation Table

| ID | Finding (one-line) | Upstream status | Upstream ref | Recommendation |
|---|---|---|---|---|
| **B1** | Solana settlement-account is a universal drain primitive | downstream-only | settlement-account flow not in upstream `crates/chains/x402-chain-solana/` | Fix ourselves: bind ATA owner == `pay_to`, cap sweep, replay store |
| **B2** | Algorand fee tx not bound to facilitator | downstream-only | Algorand chain not implemented upstream | Fix ourselves in `src/chain/algorand.rs:396-429` |
| **B3** | NEAR `ft_transfer` recipient/amount ignored | downstream-only | NEAR chain not implemented upstream | Fix ourselves in `src/chain/near.rs:482-558` |
| **B4** | Stellar Soroban auth-entry not validated | downstream-only | Stellar chain not implemented upstream | Fix ourselves in `src/chain/stellar.rs:883-974` |
| **B5** | Sui Move commands unparsed + `unwrap_or(0)` on amount | downstream-only | Sui chain not implemented upstream | Fix ourselves in `src/chain/sui.rs:207-286, 498, 518` |
| **B6** | EVM/Solana asset has no allowlist | partial | `crates/chains/x402-chain-eip155/src/v2_eip155_exact/facilitator/permit2.rs` + commit `7c7ec6d` ("validate required contract addresses during provider initialization") | Port `assert_contracts_exists` pattern for our init; **still need our own per-payment asset allowlist** (upstream uses scheme-level dispatch rather than per-asset checks) |
| **B7** | ERC-8004 endpoints `/register`, `/feedback`, `/feedback/revoke`, `/feedback/response` unauthenticated | downstream-only | ERC-8004 not implemented upstream | Fix ourselves: signed `ProofOfPayment` + per-chain gas cap |
| **B8** | No rate limit, no body limit, CORS-any | **not addressed** | `facilitator/src/run.rs:` `CorsLayer::new().allow_origin(cors::Any).allow_headers(cors::Any)` — identical bug | Fix ourselves; upstream offers no template. Add `tower-governor` + `RequestBodyLimitLayer(64 KiB)` + CORS allowlist downstream |
| **B9** | Permit2/escrow fails OPEN; CREATE3 factory trusted; broken operator `28c23AE8` allow-listed | partial | `crates/chains/x402-chain-eip155/src/v2_eip155_upto/facilitator/permit2.rs` uses `assert_onchain_allowance` + `assert_onchain_balance` (commits `067d3fd`, `8b00962`) | Port `assert_onchain_*` to our `src/payment_operator/operator.rs`; **upstream has no CREATE3 factory trust issue because upstream doesn't accept client-supplied factories** — fix that ourselves |
| **B10** | Container runs as root; SG egress `0.0.0.0/0` | **not addressed** | `Dockerfile:` no `USER` directive; FROM `debian:trixie-slim`. No IaC upstream | Fix ourselves end-to-end (non-root `USER`, drop caps, egress allowlist, signer split) |
| **F1** | Solana smart-wallet CPI scan accepts wrong transfer | downstream-only | Smart-wallet CPI scanning not in upstream (upstream parses top-level instructions only) | Fix ourselves: simulation balance-delta + `stack_height` + ALT inspection |
| **F2** | Secrets / RPC URLs leaked to logs | partial | Commit `eb44a4d` ("preserve env var name in LiteralOrEnv for round-trip display") + `b37c129` ("env var references for RPC endpoint URLs") | Port `LiteralOrEnv<T>` pattern so logs show `$VAR_NAME` instead of resolved secret; audit `tracing::info!(rpc=...)` calls in our `src/chain/evm.rs:264` |
| **F3** | `.facilitator_wallet_temp.json` not gitignored | downstream-only | Our script only | Trivial: add to `.gitignore`, write to `/tmp` with `0600` |
| **F4** | Settle not idempotent + asymmetric clock skew + no low-s | not addressed | Upstream has no idempotency key, no clock-skew tolerance, no low-s enforcement. Commit `ef33047` ("Revert nonce updates on transaction errors") partially helps with retry safety but is a different defect | Fix ourselves; consider porting `ef33047`'s nonce-revert pattern as a F4-adjacent improvement |
| **F5** | Stellar/Algorand nonce store fails OPEN on DynamoDB error | downstream-only | No Stellar/Algorand upstream; upstream EVM uses in-memory `PendingNonceManager` | Fix ourselves: fail closed, degrade to 503 |
| **F6** | `/discovery/register` SSRF | downstream-only | `/discovery/*` endpoints not in upstream | Fix ourselves: hostname allowlist, reject link-local/RFC1918/`169.254.169.254`, DNS-resolve-then-pin |
| **F7** | Error responses leak RPC URLs + revert reasons | not addressed | Upstream `crates/x402-facilitator-local/src/handlers.rs` returns raw error strings the same way | Fix ourselves: opaque error IDs, log internal detail server-side only |
| **F8** | `rustls 0.20.9 / 0.21.12` (RUSTSEC-2024-0336) | **partial — upstream avoids it accidentally** | Upstream `Cargo.lock` has ONLY `rustls 0.23.37`. The vulnerable versions enter our tree via NEAR/Stellar/Sui/Algorand SDKs — chains upstream doesn't ship | Pin rustls override in our `Cargo.toml`, or audit each chain SDK and prod-pin them. Upstream can't bump these because they aren't there. |
| **F9** | Single-AZ NAT, `desired_count=1`, hardcoded image_tag | downstream-only | No Terraform/AWS in upstream | Fix ourselves: multi-AZ NAT, `desired_count ≥ 2`, drop `image_tag` default |
| **F10** | Observability ECR `image_tag_mutability=MUTABLE` + `scan_on_push=false` | downstream-only | No ECR/IaC upstream | Fix ourselves: flip to IMMUTABLE + scan-on-push |

## What's Portable from Upstream

Concrete cherry-pick / port candidates. Each is described as `upstream-commit → downstream-target`.

1. **`7c7ec6d` — `assert_contracts_exists` at provider init** → port to our `src/provider_cache.rs` so we fail fast if `PERMIT2_ADDRESS`, validator, or UpTo proxy isn't deployed on a configured chain. Helps B6 obliquely (init-time sanity) but **does not replace** our need for a per-payment asset allowlist. Risk: low; init-time only.

2. **`067d3fd` + `8b00962` — Permit2 `assert_onchain_allowance` / `assert_onchain_balance`** → port to our `src/upto/permit2.rs` and `src/payment_operator/operator.rs`. Direct hardening for B9 (verify-replay budget burn). The patterns are clean Rust over `alloy_provider::MulticallItem`. Risk: medium — our Permit2 implementation has Ultravioleta extensions (escrow, refund, commerce scheme) so the import isn't a straight copy.

3. **`eb44a4d` + `b37c129` — `LiteralOrEnv<T>` config wrapper** → adopt for our `src/from_env.rs`. Whenever our config struct holds a secret/RPC URL, store the env var name and display `$VAR_NAME` instead of the resolved value. Closes F2's `tracing::info!(rpc=rpc_url, ...)` leak at `src/chain/evm.rs:264`. Risk: low; mechanical refactor with high payoff.

4. **`ef33047` — Revert nonce updates on tx errors (`PendingNonceManager`)** → port to our EVM nonce handling. Doesn't fix F4 (idempotency) directly but improves the retry-safety story and reduces double-spend windows during RPC errors. Risk: low; isolated change in EVM settle path.

5. **`8f66461` — Validate `SettleResponse.success` before serving resources** → this is upstream's *paygate* fix (client-side), not ours. **Worth back-porting to `crates/x402-axum`** in our workspace so any downstream consumer of our middleware fails-safe when our facilitator returns `{"success": false}` despite HTTP 200. Risk: minimal; mostly relevant for our x402-axum users.

## What Upstream Can't Help With

1. **Every chain we added (NEAR, Stellar, Sui, Algorand) — B2, B3, B4, B5, F5 plus all their RPC/error/network surface.** Upstream ships EIP-155, Solana, Aptos. Our four extra chains and their drain primitives are entirely ours to fix. These are **6 of 10 ship-blockers**.

2. **Every feature unique to Ultravioleta (ERC-8004, smart-wallet settlement-account, smart-wallet CPI scan, /discovery, custom networks, commerce scheme) — B1, B7, F1, F6.** Upstream has none of this. The reputation system, the Crossmint-compatible settlement account, the smart-wallet CPI scanning, and the discovery handler all need their own auth/validation work.

3. **All AWS infrastructure — B10 (partial), F9, F10.** Upstream is a library with a stock Dockerfile. No Terraform, no ECS task definitions, no IAM. The container hardening (non-root `USER`, dropped caps, egress allowlist) and the multi-AZ / immutable-ECR / scan-on-push work is downstream-only.

## Where Upstream Is Worse Than We Thought

Two findings the audit flagged as our defects are **also defects in upstream**, which is worth knowing:

- **B8 (CORS-any + no rate limit + no body limit)** — `facilitator/src/run.rs` on upstream HEAD has `CorsLayer::new().allow_origin(cors::Any).allow_headers(cors::Any)` and no `tower-governor` / `DefaultBodyLimit`. Not our regression. Still ours to fix because we run a public endpoint with hot wallets and upstream doesn't.
- **B10 (root container)** — upstream `Dockerfile` lacks any `USER` directive and ships as root from `debian:trixie-slim`. Identical to ours. Worth a PR upstream if we fix it well; for now, fix ours.

## Recommended Cherry-Pick / Port Order

Sequenced by `value × low_risk × small_diff`:

1. **First (today):** F3 `.gitignore` line. Trivial, downstream-only.
2. **Same PR:** F8 — pin `rustls = "0.23.37"` override in our root `Cargo.toml` `[patch.crates-io]` or `[workspace.dependencies]`, then `cargo update`. Verify TLS still works against QuickNode/Alchemy.
3. **Next PR:** Port `eb44a4d` `LiteralOrEnv<T>` pattern → `src/from_env.rs`. Audit every `tracing::info!`/`error!` that includes RPC URL or key. Closes F2 + parts of F7.
4. **Hardening PR:** Add `tower-http::limit::RequestBodyLimitLayer(64 * 1024)` + `tower-governor` + CORS allowlist in `src/main.rs`. Closes B8. No upstream code to copy — write from `tower-http` docs.
5. **Big-rock PR (multi-step):** Port `assert_onchain_allowance`/`assert_onchain_balance` (commits `067d3fd`, `8b00962`) into `src/payment_operator/operator.rs` AND change `verify_escrow` to fail closed on RPC error. Closes B9.
6. **Per-chain PRs (B1, B2, B3, B4, B5, F1, F5):** No upstream help. Each chain owner audits the validation gap, adds the bind-to-`paymentRequirements` checks, and updates fuzz tests. **These are the highest-risk fixes.**
7. **Infra PR:** Dockerfile `USER`, SG egress allowlist, multi-AZ NAT, ECR immutable, `desired_count ≥ 2`. Closes B10 + F9 + F10.
8. **Auth PR:** ERC-8004 endpoints get signed `ProofOfPayment` + per-chain daily gas cap. Closes B7.

## Three Bottom Lines

1. **Upstream is not a parachute.** Three patterns are portable (`assert_contracts_exists`, `assert_onchain_*`, `LiteralOrEnv`). Everything else is on us.
2. **The downstream surface area is the reason for the audit findings.** We added 4 chains, an ERC-8004 reputation system, smart-wallet support, a discovery service, and AWS infra — each is a fresh attack surface upstream never tested.
3. **B8 and B10 are inherited.** Worth a PR upstream after we fix our own. Builds reputation and reduces our merge drift on future syncs.
