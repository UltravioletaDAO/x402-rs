# x402-rs Payment Facilitator — Consolidated Audit Report

**Date:** 2026-05-19
**Auditors:** 13 parallel agents (8 security lanes + HR Manager + Python/Rust/Networking/Cloud specialists)
**Consolidated by:** Ruthless PM
**Target:** `facilitator.ultravioletadao.xyz` (production), commit `686683a` on `main`

## 1. Executive Summary

The x402-rs facilitator is a single Rust process holding hot wallets for seven blockchains and exposing public, unauthenticated HTTP endpoints that can be coerced into signing arbitrary token transfers. Thirteen independent auditors converged on the same root cause from different angles: the service trusts merchant-supplied data (asset addresses, settlement signatures, factory addresses, fee transactions, Move/Soroban payloads) without binding it back to `paymentRequirements`. Combined with no rate limiting, no body limits, no authentication on ERC-8004 endpoints, root-running containers, and all keys colocated in one Fargate task, exploitation is not theoretical — multiple chains have a literal drain-any-wallet primitive today. Top systemic issue: validation-by-assumption.

## 2. Ship Verdict

**SHIP-BLOCKER — do not deploy as-is.** At least six distinct paths drain facilitator funds without privileged access (Algorand fee-tx, Solana settlement-account, NEAR ft_transfer args, Stellar Soroban auth, Sui Move commands, and any-token EVM settlement). The blast radius is "every hot wallet, every chain, in one RCE."

## 3. Top 10 Ship-Blockers

**B1 — Solana settlement-account is a universal drain primitive** [A2-C1, A2-C2]
`src/chain/solana.rs:1043-1234, 1242-1431` + `src/upto/types.rs:498-509`. Client supplies `settleSecretKey` and `settlement_rent_destination`; facilitator verifies USDC delta without binding to `pay_to`'s ATA, then sweeps full balance + harvests rent.
**Fix:** require settlement account ATA owner == `pay_to`, cap sweep to `max_amount_required`, store `(network, tx_signature)` for replay.

**B2 — Algorand fee transaction not bound to facilitator** [A3-C1]
`src/chain/algorand.rs:396-429, 564-568`. `validate_fee_transaction` never enforces `fee_tx.sender == facilitator_address`. Submit `[Payment(facilitator → attacker, 100 ALGO), USDC_xfer]`, facilitator co-signs, ALGO gone.
**Fix:** assert `fee_tx.sender == facilitator` AND `fee_tx.amount <= fee_cap` before sign.

**B3 — NEAR ft_transfer recipient/amount ignored** [A3-C2]
`src/chain/near.rs:482-558`. Inner action args never compared to `requirements.pay_to` / `max_amount_required`.
**Fix:** parse `actions[].FunctionCall.args`, assert `receiver_id == pay_to`, `amount == requirements.max_amount_required`.

**B4 — Stellar Soroban auth-entry not validated** [A3-C3]
`src/chain/stellar.rs:883-974, 999-1014`. `contract_address`, `function_name`, `args` accepted from client. Facilitator signs any swap or non-USDC transfer.
**Fix:** assert `contract == known_usdc`, `function == "transfer"`, decoded args match `(facilitator, pay_to, amount)`.

**B5 — Sui Move commands unparsed** [A3-C4, A12-H1]
`src/chain/sui.rs:207-286`. JSON `to`/`amount` diverges from BCS-decoded `ProgrammableTransaction`; line 498/518 `unwrap_or(0)` lets non-numeric amount make `required_amount = 0`.
**Fix:** decode BCS programmable tx, walk Move commands, match recipient + coin + amount; remove `unwrap_or(0)`.

**B6 — EVM asset has no allowlist** [A1-C1, A2-H4]
`src/chain/evm.rs:1545`. `requirements.asset` accepted as any ERC-20; malicious token returns fake EIP-712 metadata. Same hole on Solana mint side.
**Fix:** `asset ∈ supported_tokens_for_network(network)`; reject otherwise.

**B7 — ERC-8004 endpoints unauthenticated + unbounded** [A4-C1, A6-C1..C4]
`src/handlers.rs:2129, 2460, 2687, 3924`. `/register`, `/feedback`, `/feedback/revoke`, `/feedback/response` anyone-can-call; facilitator is `msg.sender` so on-chain ACL is meaningless. Anon caller drains gas across Base/Polygon/Optimism/Hedera and smears any agent.
**Fix:** require signed `ProofOfPayment` linking caller → action; per-chain daily gas cap.

**B8 — No rate limit, no body limit, no auth, CORS-any** [A4-C2, A4-C3, A11-C1, A4-H1, A4-H2]
`src/main.rs:281-293`. No `tower-governor`, no `DefaultBodyLimit`, `Bytes` extractor on `/settle` bypasses 2MB default; `extra: serde_json::Value` has no recursion limit. 10GB POST OOMs the 2GB Fargate task; QuickNode quota drains in minutes.
**Fix:** `RequestBodyLimitLayer(64 KiB)`, `tower-governor` per IP+route, restrict CORS allowlist, cap JSON depth.

**B9 — Permit2 / escrow fails OPEN; CREATE3 factory trusted** [A1-C2, A1-C3, A7-C1, A7-C2]
`src/payment_operator/operator.rs:245-253, 693, 700, 949-996`; `src/upto/permit2.rs:182-269`. RPC error on `verify_escrow` returns `isValid: true`; `tokenCollector` and CREATE3 factory taken from client; known-broken operator `28c23AE8…` still allow-listed.
**Fix:** fail closed on RPC error; pin factory + operator allow-lists per chain; drop broken operator.

**B10 — Container runs as root; all keys in one task** [A8-C1, A13-CRITICAL, A11-H5]
`Dockerfile:18-42` no `USER`; `terraform/.../main.tf:187-193` SG egress `0.0.0.0/0`. Single RCE → all wallets (EVM mainnet+testnet, Solana, NEAR, Stellar, Sui, Algorand) exfilled over any port.
**Fix:** non-root user, drop caps, restrict egress to RPC CIDRs, split signer into separate task with KMS or per-chain microservice.

## 4. Top 10 High-Priority Follow-Ups

**F1 — Solana smart-wallet CPI scan accepts wrong transfer** [A2-C3, A2-H1, A2-H3]
`src/chain/solana.rs:590-733, 949-1023, 931-947`. No `stack_height` check, no balance-delta verify, ALT-resolved indices unread. Bind via simulation `accounts` post-balances and reject v0 txs touching facilitator ATA.

**F2 — Secrets leaked to stream/logs** [A5-C1, A5-H1, A10-H4]
`terraform/environments/production/validate_secrets.sh:85,112,139,166` and `src/chain/evm.rs:264` print key prefixes / full RPC URLs (with API keys) to stdout and CloudWatch. `scripts/migrate_secrets.py:72` prints `evm_private_key[:20]`. Mask all key material; redact RPC URLs.

**F3 — `.facilitator_wallet_temp.json` not gitignored** [A10-C1]
`scripts/rotate_wallet.py:331-335`. Freshly generated key written to repo root; one `git add -A` ships it. Add to `.gitignore`, write to `/tmp` with `0600`, delete on success.

**F4 — Settle is not idempotent + clock skew + sig malleability** [A4-C4, A1-H2, A1-H4]
`src/chain/evm.rs:1202-1221, 1638-1656`, `src/handlers.rs:1809`. Asymmetric 6s past tolerance, no low-s enforcement, no idempotency key. Add `Idempotency-Key`, enforce low-s, symmetric clock window.

**F5 — Nonce store fails OPEN** [A3-H3]
`src/chain/stellar.rs:847-851, 875-879`; `src/chain/algorand.rs:310-315`. DynamoDB error → allow. Fail closed; degrade to 503.

**F6 — `/discovery/register` SSRF** [A6-H1]
`src/handlers.rs:292-319` + `src/discovery.rs:596-622`. No URL allowlist; can register `169.254.169.254`. Block link-local + RFC1918; resolve before fetch.

**F7 — Error responses leak RPC URLs + revert reasons** [A4-H6]
`src/handlers.rs:1893, 1937-1944, 3354, 3823, 944, 952`. Wrap errors in opaque IDs, log internal detail server-side.

**F8 — Vulnerable `rustls 0.20.9 / 0.21.12`** [A12-H3]
`Cargo.lock`. RUSTSEC-2024-0336 close_notify DoS. Bump `reqwest` to a version on `rustls 0.23.5+` or pin override.

**F9 — Single AZ NAT, `desired_count = 1`, hardcoded image_tag** [A11-H4, A13-HIGH, A8-M4]
`terraform/.../main.tf:81-90, variables.tf:150`. Single-AZ outage kills every settle; `terraform apply` without `-var` rolls back to v1.24.0. Multi-AZ NAT or NAT instances per AZ; remove default `image_tag`; min 2 tasks.

**F10 — Observability ECR mutable + unscanned** [A8-C2, A13-HIGH]
`terraform/.../observability.tf:73, 86, 99, 112`. `image_tag_mutability = "MUTABLE"`, `scan_on_push = false` — sidecars share task netns with facilitator. Flip both.

## 5. Systemic Patterns

- **Trust-by-default of merchant-supplied data.** Same defect, six chains: Algorand fee-tx sender, NEAR ft_transfer args, Stellar auth-entry, Sui Move commands, EVM asset/operator/factory, Solana settleSecretKey. The architecture treats `paymentRequirements` as a hint, not a binding contract.
- **Fail-open under pressure.** Escrow verify, Stellar/Algorand nonce store, OFAC screening (Solana), Permit2 RPC error — all degrade to allow. The opposite of how a custody service should behave.
- **One process, all keys, no isolation.** No KMS, no signer microservice, no per-chain gas cap, no velocity limit, container runs as root with egress-anywhere.
- **HTTP layer is naïve.** No rate limit, no body limit, CORS-any, unauthenticated mint endpoints, leaky error strings — the basics of a public Axum service are missing.
- **Documentation drift from reality.** NAT gateway vs documented NAT instance; rotation runbook covers only 2 of 7 chains; `lambda/balances/handler.py` carries parallel wallet addresses; default `image_tag` lies about deployed version.

## 6. Controversial Calls

- **Dropped A8-M2 (no ALB access logs) and A8-H3 (7-day log retention) from blockers.** Real defects but cannot themselves drain funds; move to backlog after B1-B10 ship.
- **Downgraded A12-M1 (`unsafe env::set_var` in tests).** Test-only race, not a production attack surface.
- **Kept A6 architectural ("facilitator is msg.sender for all ERC-8004 ops") as Top-10 (B7) despite Auditor 6 framing it as design, not bug.** When `msg.sender` is universal, the spec's ACL is decorative — that's a security defect, not a design choice.
- **Did NOT downgrade "no rate limit" despite appearing in 4 reports.** Repetition is signal: it gates B7, B8, and the QuickNode bill.
- **Demoted A9 to context only.** HR-Manager finding contained no exploitable defect.

## 7. Three Questions Before Next Deploy

1. **Where is the documented, tested offline backup for each of the seven hot wallets (EVM mainnet, EVM testnet, Solana, NEAR, Stellar, Sui, Algorand), what is the rotation runbook for the five chains not covered in `docs/WALLET_ROTATION.md`, and when was each wallet last rotated?**
2. **What is the maximum value a single unauthenticated POST to `/settle`, `/feedback`, `/register`, or `/accepts` can move or burn — denominated in USD — given current code on main, and what is the per-minute aggregate cap enforced by the process?**
3. **Which subset of B1-B10 is the team committing to fix before the next ECR push, who owns each, and what is the rollback plan if the fix itself breaks a live merchant integration (Crossmint, Faremeter, Execution Market)?**

## 8. What the Team Did Right

- **Separate mainnet/testnet wallet env vars** (`src/from_env.rs:174-261`) — fixes the v1.3.0 incident pattern, even if legacy fallback still exists.
- **TLS 1.2+ enforced on ALB** (confirmed A11-L1) and AWS Secrets Manager references for RPC URLs documented in `CLAUDE.md` — the secrets-handling intent is right where it's followed.
- **EIP-712 domain resolution priority chain** (`src/chain/evm.rs:assert_domain`) handles the EURC-on-Base / USDC-on-Celo naming chaos correctly; the audit only found a bypass when clients are allowed to supply unknown tokens (fixed by B6).
