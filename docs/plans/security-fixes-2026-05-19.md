# Security Hardening Plan — 2026-05-19

**Source audit:** `docs/reports/2026-05-19-security-audit.md`
**Upstream correlation:** `docs/reports/2026-05-19-upstream-correlation.md`
**Status:** complete — all PRs 1–11 landed, and the 2026-05-21 follow-up
batch (gaps #1–#4 from handoff §4: B8 rate limit + JSON depth cap, F3 deep
Python fix, F4 idempotency + low-s, B10 SG egress + VPC endpoints) is also
landed. Awaiting user manual build/deploy.
**Handoff report:** `docs/reports/2026-05-19-security-fixes-handoff.md`
(see §10 for the 2026-05-21 batch detail)

## Scope decisions (locked by user 2026-05-19)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Real GitHub issues | NO | Doxing concern — internal tracking doc only |
| B6 EVM/Solana asset allowlist | STRICT | Only tokens defined in `src/network.rs` |
| Primary integrator for risk modeling | Execution Market | Only known active first-party caller |
| B8 64 KiB body limit | YES (after measuring P99) | Bound DoS, but verify legit traffic fits |
| B9 operator `28c23AE8…` | KEEP allow-listed | Likely still in use; no telemetry added |
| B7 ERC-8004 auth | POSTPONED | Will be coordinated via separate IRC channel |
| CORS allowlist (was in B8) | PAUSED | Facilitator is public by design (photo2melee, ExecutionMarket, meshrelay, third parties) |
| Compile/deploy | USER DOES IT MANUALLY | Per project CLAUDE.md — Claude only edits files |

## Execution order (low risk → high risk)

| PR  | Findings              | Files touched (approx)                                                                | Risk | Status |
|-----|-----------------------|---------------------------------------------------------------------------------------|------|--------|
| 1   | F3, F8                | `.gitignore`                                                                          | Trivial | done |
| 2   | F2, F7                | `src/chain/evm.rs`, `src/handlers.rs`, `scripts/migrate_secrets.py`, `terraform/.../validate_secrets.sh` | Low | done |
| 3   | B8 (partial), F4, F6  | `src/main.rs`, `src/handlers.rs`, `src/discovery.rs`, `src/chain/evm.rs`, `Cargo.toml` | Medium | done |
| 4   | B1, F1                | `src/chain/solana.rs`, `src/upto/types.rs`                                            | High | done (aegis-solana) |
| 5   | B2                    | `src/chain/algorand.rs`                                                               | High | done (aegis-algorand) |
| 6   | B3                    | `src/chain/near.rs`                                                                   | High | done (aegis-near) |
| 7   | B4                    | `src/chain/stellar.rs`                                                                | High | done (aegis-stellar) |
| 8   | B5                    | `src/chain/sui.rs`                                                                    | High | done (aegis-sui) |
| 9a  | B6 (EVM + Permit2)    | `src/network.rs`, `src/chain/evm.rs`, `src/upto/permit2.rs`                           | Medium | done |
| 9b  | B6 (Solana)           | `src/chain/solana.rs`                                                                 | Medium | done |
| 10  | B9 (partial), F5      | `src/payment_operator/operator.rs`, `src/upto/permit2.rs`, `src/chain/stellar.rs`, `src/chain/algorand.rs`, `src/nonce_store.rs` | Medium | done |
| 11  | B10 (partial), F9, F10 | `Dockerfile`, `terraform/environments/production/{main,variables,observability}.tf` | Medium (infra) | done |

### 2026-05-21 follow-up batch (gaps from handoff §4)

| PR  | Findings              | Files touched                                                                          | Risk | Status |
|-----|-----------------------|----------------------------------------------------------------------------------------|------|--------|
| F3-deep | F3 deeper Python fix | `scripts/rotate_wallet.py` (writes wallet temp under `/tmp` with 0o600 perms) | Trivial | done |
| 12  | B8 (rate limit + JSON depth) | `Cargo.toml`, `src/main.rs`, `src/handlers.rs`, `src/lib.rs`, `src/types.rs`, `src/types_v2.rs`, `src/upto/types.rs`, **new** `src/json_depth.rs` | Medium | done |
| 13  | F4 (idempotency + low-s) | `src/chain/evm.rs`, `src/handlers.rs`, `src/main.rs`, `src/lib.rs`, `terraform/.../main.tf`, **new** `src/idempotency_store.rs` | High | done |
| 14  | B10 (SG egress + VPC endpoints) | `terraform/environments/production/main.tf` | Medium (infra) | done |

## Explicitly NOT in this rollout

- **B7 ERC-8004 auth** — postponed by user; verify separately whether already implemented
- **CORS allowlist** — facilitator stays public
- **Drop operator `28c23AE8…`** — kept by user decision
- **GitHub issues / PR descriptions referencing security findings** — kept internal
- **Compile, docker build, ECR push, ECS deploy** — user does manually

## PR-1 detail — F3 + F8

**F3 — `.facilitator_wallet_temp.json` could land in repo via `git add -A`**
- Audit ref: `scripts/rotate_wallet.py:331-335`
- Fix: add `.facilitator_wallet_temp.json` and `**/.facilitator_wallet_temp.json` to `.gitignore`
- Verified never committed (git log + git ls-files clean)
- **Done in this PR.** The deeper fix (write to `/tmp` with `0600`, delete on success) needs a Python edit in `scripts/rotate_wallet.py`; deferred to a follow-up if user wants.

**F8 — Vulnerable `rustls 0.20.9 / 0.21.12`**
- Audit ref: `Cargo.lock`, RUSTSEC-2024-0336 close_notify DoS
- **Verified non-issue.** RUSTSEC-2024-0336 fix versions:
  - 0.20.x: fixed in **0.20.9** (we have 0.20.9)
  - 0.21.x: fixed in **0.21.11** (we have 0.21.12)
  - 0.22.x: fixed in 0.22.4
  - 0.23.x: fixed in 0.23.5 (we have 0.23.37)
- Sources of legacy rustls in our tree:
  - `rustls 0.20.9` ← `hyper-rustls 0.23.2`, `tokio-rustls 0.23.4`
  - `rustls 0.21.12` ← `aws-smithy-http-client 1.1.12`, `hyper-rustls 0.24.2`, `tokio-rustls 0.24.1`, `tokio-tungstenite 0.20.1`, `tungstenite 0.20.1`
- Pinning via `[patch.crates-io]` to 0.23.37 would break API consumers across major versions.
- **No code change in PR-1.** Tracked here so we don't re-litigate.
- Follow-up watchlist: bump `aws-smithy-http-client`, `solana-client`, and any NEAR SDK crates that still hold 0.21.x at next dependency review.

## PR-2 detail — F2 + F7

**F2 — Secrets leaked to stream/logs**
- Audit refs:
  - `terraform/environments/production/validate_secrets.sh:85,112,139,166` — prints key prefixes
  - `src/chain/evm.rs:264` — RPC URL with API key in trace
  - `scripts/migrate_secrets.py:72` — prints `evm_private_key[:20]`
- Fix:
  - Mask all key material in shell scripts (`***` instead of prefixes)
  - Redact RPC URLs in tracing (strip path after host)
  - Drop `[:20]` slicing in Python script
- Stream-safety check: per CLAUDE.md global, user is always on stream. NO partial-key fingerprints either.

**F7 — Error responses leak RPC URLs + revert reasons**
- Audit refs: `src/handlers.rs:1893,1937-1944,3354,3823,944,952`
- Fix:
  - Wrap external-facing errors in opaque IDs (e.g., `correlation_id = uuid::Uuid::new_v4()`)
  - Log full detail server-side via `tracing::error!(%correlation_id, error=?e)`
  - Return `{"error":"internal","correlation_id":"…"}` to client

## PR-3 detail — B8 + F4 + F6

**B8 (scoped) — No body limit, no rate limit, no JSON depth cap**
- Audit ref: `src/main.rs:281-293`
- Fix:
  - `tower-http` add `"limit"` feature, then `RequestBodyLimitLayer::new(64 * 1024)` on the Axum router
  - `tower-governor` per IP+route (e.g., `30 req/min/IP` on `/verify` and `/settle`)
  - JSON recursion limit via custom `serde_json::Deserializer` builder where `extra` is parsed
- CORS unchanged — public service.

**F4 — Settle not idempotent, clock skew asymmetric, sig malleability**
- Audit refs: `src/chain/evm.rs:1202-1221,1638-1656`, `src/handlers.rs:1809`
- Fix:
  - Idempotency: optional `Idempotency-Key` header → if present, cache `(key, tx_signature)` in DynamoDB; replay returns same signature.
  - Clock skew: symmetric ±60s window for both `validAfter` and `validBefore`.
  - Low-s: reject signatures with `s > secp256k1_n / 2`.

**F6 — `/discovery/register` SSRF**
- Audit refs: `src/handlers.rs:292-319`, `src/discovery.rs:596-622`
- Fix:
  - Resolve URL to IP **before** fetch (`tokio::net::lookup_host`)
  - Reject if any resolved IP is in: RFC1918 (10/8, 172.16/12, 192.168/16), link-local (169.254/16), loopback (127/8), unique-local (fc00/7), benchmark (198.18/15), AWS metadata IPs.

## PR-4 to PR-8 — Per-chain hardening

These are downstream-only fixes (no upstream pattern to port). Each runs in its own agent.

| PR | Chain | Files | Key invariant to enforce |
|----|-------|-------|--------------------------|
| 4  | Solana | `src/chain/solana.rs`, `src/upto/types.rs` | settlement_account ATA owner == pay_to; sweep ≤ max_amount; replay store |
| 5  | Algorand | `src/chain/algorand.rs` | fee_tx.sender == facilitator AND fee_tx.amount ≤ fee_cap |
| 6  | NEAR | `src/chain/near.rs` | actions[].FunctionCall.args: receiver_id == pay_to AND amount == max_amount_required |
| 7  | Stellar | `src/chain/stellar.rs` | Soroban auth: contract == known_usdc, function == "transfer", args == (facilitator, pay_to, amount) |
| 8  | Sui | `src/chain/sui.rs` | BCS-decoded ProgrammableTransaction Move command matches (recipient, coin, amount); no `unwrap_or(0)` |

## PR-9 detail — B6 asset strict allowlist

- Audit refs: `src/chain/evm.rs:1545`, Solana mint side (`src/chain/solana.rs` verify path)
- Plan vs reality:
  - `src/network.rs`: added `supported_asset_addresses(network) -> Vec<MixedAddress>` and `is_supported_asset(network, &MixedAddress) -> bool` that aggregate all 5 stablecoin `by_network` lookups (USDC, EURC, AUSD, PYUSD, USDT). — **DONE**.
  - `src/chain/evm.rs`: `assert_valid_payment` now calls `is_supported_asset(chain.network, &requirements.asset)` immediately after the time check and before any RPC call. Rejects with `FacilitatorLocalError::Other("unsupported_asset: network=..., asset=...")`. Covers verify and settle (both go through `assert_valid_payment`). — **DONE**.
  - `src/upto/permit2.rs` (`validate_offchain`): added the same strict allow-list check after the existing asset-equality check. Resolves `accepted.network` via `Network::from_caip2` and rejects with `UptoError::InvalidPayload("unsupported_asset: ...")`. — **DONE**.
  - **PR-9b — Solana strict allowlist**: `src/chain/solana.rs` `Facilitator::verify` and `Facilitator::settle` now both call `is_supported_asset(self.network(), &request.payment_requirements.asset)` as the first check before any other validation. Two checks (not one) because the settlement-account branch in `settle` bypasses `verify_transfer`, so a single check on verify would leave settle untrusted. Rejects with `FacilitatorLocalError::Other("unsupported_asset: network=..., asset=...")`. Covers SPL Token, spl_token_2022, and Crossmint settlement-account flows. — **DONE** (landed after aegis-solana / PR-4 released `src/chain/solana.rs`).

## PR-10 detail — B9 + F5

**B9 (scoped) — Permit2 / escrow fails OPEN; CREATE3 factory trusted**
- Audit refs: `src/payment_operator/operator.rs:245-253,693,700,949-996`, `src/upto/permit2.rs:182-269`
- Plan vs reality:
  - `verify_escrow` RPC error → return `isValid: false` with `verification_unavailable (ref: <uuid>)` — **DONE** in `operator.rs:245-253`.
  - Pin CREATE3 factory + operator allow-lists per chain — **already pinned** in `src/payment_operator/addresses.rs` (`OperatorAddresses::for_network` + `validate_addresses`). No new code needed.
  - `28c23AE8…` kept in SkaleBase allow-list per user decision.
  - `src/upto/permit2.rs` audit: `validate_offchain` is pure off-chain validation (no RPC, cannot fail-open). `verify_upto` propagates RPC errors via `?`. No fail-open path. **No change required.**

**F5 — Nonce store fails OPEN**
- Audit refs: `src/chain/stellar.rs:865-880,884-908`, `src/chain/algorand.rs` group nonce path
- Plan vs reality:
  - Stellar: added `StellarError::NonceStoreUnavailable(String)` variant. `check_nonce_unused` and `check_and_mark_nonce_used` now log with correlation_id and return `Err(NonceStoreUnavailable(...))` on storage error instead of `Ok(())`. — **DONE**.
  - Algorand: `check_and_mark_group_used` was **already fail-closed** (maps `NonceStoreError` → `AlgorandError::RpcError`). No change required.
  - `src/nonce_store.rs` distinct error variants (`NonceAlreadyUsed`, `ConnectionFailed`, `ReadError`, `WriteError`) preserved so callers can distinguish replay from unavailability.

## PR-11 detail — B10 + F9 + F10

**B10 — Container runs as root; SG egress 0/0**
- Audit refs: `Dockerfile:18-42`, `terraform/.../main.tf:187-193`
- Fix:
  - Dockerfile: add `USER 10001:10001` (non-root); ensure runtime files writable
  - Terraform SG egress: restrict to RPC CIDRs (or VPC endpoints) + DynamoDB + Secrets Manager

**F9 — Single-AZ NAT, desired_count=1, hardcoded image_tag default**
- Audit refs: `terraform/.../main.tf:81-90`, `terraform/.../variables.tf:150`
- Fix:
  - Multi-AZ NAT (gateway or per-AZ instances)
  - `desired_count` minimum 2 (or autoscaling 1-5 with min 2 for prod)
  - Drop the `image_tag` default — force explicit `-var image_tag=vX.Y.Z`

**F10 — ECR observability sidecars mutable + unscanned**
- Audit refs: `terraform/.../observability.tf:73,86,99,112`
- Fix: `image_tag_mutability = "IMMUTABLE"`, `scan_on_push = true`

## Verification per PR (no deploy, only static check)

After each edit:
```bash
# Format + clippy (offline)
cargo fmt --check
cargo clippy --all-targets --no-deps -- -D warnings

# Smoke test for HTTP layer (PR-3 only)
cargo test --no-run --test '*'
```

Final handoff report: `docs/reports/2026-05-19-security-fixes-handoff.md` (created after PR-11).
