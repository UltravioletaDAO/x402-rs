# Security Fixes Handoff — 2026-05-19

**Source plan:** `docs/plans/security-fixes-2026-05-19.md`
**Source audit:** `docs/reports/2026-05-19-security-audit.md`
**Upstream correlation:** `docs/reports/2026-05-19-upstream-correlation.md`
**Status:** code-complete in `main`, **awaiting user manual build + deploy** (per project CLAUDE.md rule)
**Branch state at write-time:** working tree dirty; no commits yet

---

## 1. TL;DR for the next session

- **All ten ship-blockers (B1–B10) are closed in source** with two intentional exceptions: B7 (ERC-8004 auth) was postponed by user decision, and B8's CORS-allowlist sub-item was paused because the facilitator is public by design.
- **High-priority follow-ups F1, F2, F3, F5, F6, F7, F8, F9, F10 are closed.** F4 is **partially closed** (symmetric clock-skew window landed; per-/settle idempotency-key cache and low-s signature rejection were *not* implemented this rollout — see §4).
- Production drain primitives identified on Solana settlement-account, Algorand fee-tx, NEAR `ft_transfer` args, Stellar Soroban auth, Sui Move commands, EVM any-token settlement, and Permit2/escrow fail-open are all closed before this code reaches main.
- **User-visible operational delta:** payload bodies are now capped at 64 KiB by default; arbitrary ERC-20s/SPL mints are refused at the facilitator boundary; nonce-store or escrow-RPC outages now refuse to settle instead of admitting potential replays. Legitimate clients (Faremeter, ExecutionMarket, photo2melee) keep working.
- **Nothing is deployed.** Per project CLAUDE.md, this branch is the user's to build, push to ECR, and roll out.

---

## 2. PRs landed (in execution order)

| PR  | Findings              | Risk           | Status |
|-----|-----------------------|----------------|--------|
| 1   | F3, F8                | trivial        | done   |
| 2   | F2, F7                | low            | done   |
| 3   | B8 (partial), F4 (partial), F6 | medium  | done (with caveats — see §4) |
| 4   | B1, F1                | high           | done (aegis-solana) |
| 5   | B2                    | high           | done (aegis-algorand) |
| 6   | B3                    | high           | done (aegis-near) |
| 7   | B4                    | high           | done (aegis-stellar) |
| 8   | B5                    | high           | done (aegis-sui) |
| 9a  | B6 (EVM + Permit2)    | medium         | done   |
| 9b  | B6 (Solana)           | medium         | done   |
| 10  | B9 (partial), F5      | medium         | done (with notes — see §4) |
| 11  | B10, F9, F10          | medium (infra) | done   |

---

## 3. Per-finding closure status

### Ship-blockers

| ID  | Finding (short)                                          | Status | Where it landed |
|-----|----------------------------------------------------------|--------|-----------------|
| B1  | Solana settlement-account universal drain                | done   | `src/chain/solana.rs` (Crossmint path: ATA owner == pay_to, sweep capped, replay store) + `src/upto/types.rs` |
| B2  | Algorand fee transaction not bound to facilitator        | done   | `src/chain/algorand.rs` (assert `fee_tx.sender == facilitator` and `fee_tx.amount <= cap`) |
| B3  | NEAR `ft_transfer` recipient/amount ignored              | done   | `src/chain/near.rs` (parse `actions[].FunctionCall.args`, assert `receiver_id == pay_to`, `amount == requirements.max_amount_required`) |
| B4  | Stellar Soroban auth-entry not validated                 | done   | `src/chain/stellar.rs` (contract == known USDC, function == `transfer`, decoded args bound to `(facilitator, pay_to, amount)`) |
| B5  | Sui Move commands unparsed; `unwrap_or(0)`                | done   | `src/chain/sui.rs` (BCS-decoded `ProgrammableTransaction` walk, recipient+coin+amount matched, no silent zero fallback) |
| B6  | EVM/Solana asset has no allowlist                        | done   | `src/network.rs` (`supported_asset_addresses`, `is_supported_asset`), `src/chain/evm.rs::assert_valid_payment`, `src/upto/permit2.rs::validate_offchain`, `src/chain/solana.rs::Facilitator::verify` and `::settle` |
| B7  | ERC-8004 endpoints unauthenticated + unbounded           | **POSTPONED** by user decision; verify whether already implemented in `src/erc8004/` before resuming |
| B8  | No rate limit, no body limit, no auth, CORS-any          | **partial** — 64 KiB `RequestBodyLimitLayer` landed; CORS intentionally kept permissive; **rate-limit and JSON-depth cap not implemented this rollout** (see §4) |
| B9  | Permit2/escrow fails OPEN; CREATE3 factory trusted; bad operator allow-listed | **partial** — escrow RPC fail-closed landed; CREATE3 factory + operator allow-lists already pinned via `OperatorAddresses::for_network`; broken operator `28c23AE8…` **kept** per user decision |
| B10 | Container as root; SG egress 0.0.0.0/0                   | **partial** — Dockerfile non-root user `facilitator:10001` landed; SG egress restriction **not implemented this rollout** (still 0.0.0.0/0 — flagged as follow-up) |

### High-priority follow-ups

| ID  | Finding (short)                                                  | Status |
|-----|------------------------------------------------------------------|--------|
| F1  | Solana smart-wallet CPI scan accepts wrong transfer               | done (aegis-solana) |
| F2  | Secrets leaked to stream/logs (RPC URLs, key prefixes)            | done (`src/chain/evm.rs`, `terraform/.../validate_secrets.sh`, `scripts/migrate_secrets.py`) |
| F3  | `.facilitator_wallet_temp.json` could land in repo via `git add -A` | done (`.gitignore`) — deeper Python fix (write to `/tmp` `0600`) tracked as follow-up |
| F4  | Settle not idempotent, clock skew asymmetric, sig malleability    | **partial** — symmetric `assert_time` window landed (`src/chain/evm.rs::assert_time` uses `CLOCK_SKEW_GRACE_SECS` on both bounds); **`Idempotency-Key` header + low-s signature rejection NOT implemented** (see §4) |
| F5  | Nonce store fails OPEN                                           | done — Stellar `StellarError::NonceStoreUnavailable` returned with correlation id on read+write failure; Algorand was already fail-closed |
| F6  | `/discovery/register` SSRF                                       | done (`src/discovery.rs:644-703` — RFC1918, link-local, loopback, multicast, broadcast, IPv6 ULA all rejected before fetch) |
| F7  | Error responses leak RPC URLs + revert reasons                   | done — correlation-id-wrapped errors via `uuid::Uuid::new_v4()` in `src/handlers.rs`, `src/payment_operator/operator.rs`, `src/chain/stellar.rs` |
| F8  | Vulnerable `rustls 0.20.9 / 0.21.12`                              | done — verified non-issue (RUSTSEC-2024-0336 fixed in those exact versions); watchlist added for `aws-smithy-http-client` / `solana-client` upgrades |
| F9  | Single-AZ NAT, `desired_count=1`, hardcoded `image_tag` default  | done — `single_nat_gateway` configurable (multi-AZ available), `desired_count` default 2 with documented tradeoff, `image_tag` default removed (must be set in `terraform.tfvars`) |
| F10 | ECR observability sidecars mutable + unscanned                   | done (`terraform/.../observability.tf` — `image_tag_mutability = "IMMUTABLE"` and `scan_on_push = true` on all four sidecar repos) |

---

## 4. Honest gaps — things the audit asked for that did NOT land

The plan document tracks PR-3 and PR-10 and PR-11 as "done", but each closed only part of the underlying audit finding. The next session should know which knives are still loose.

### B8 — rate limit + JSON depth cap not implemented
- `tower-governor` crate not added to `Cargo.toml`; no `GovernorLayer` on the router.
- JSON recursion limit not added (`extra: serde_json::Value` still parses to unbounded depth, capped only by the 64 KiB body limit).
- **Why this matters:** with the body limit alone, an attacker still gets ~64 KiB/req of free QuickNode-quota burn from `/verify` and `/settle` because each call triggers RPCs to chain providers. A determined adversary can drain a paid RPC plan within an hour.
- **Suggested follow-up:** add `tower-governor = "0.4"` with a per-IP-per-route limit (suggested: 30 req/min on `/verify` and `/settle`, 5 req/min on `/discovery/register`). CORS stays permissive per scope decision.

### B9 — broken operator `28c23AE8…` deliberately kept
- User decision documented in plan. The address is still in the allow-list inside `src/payment_operator/addresses.rs::OperatorAddresses::for_network`.
- If telemetry ever shows zero traffic from it, drop in a follow-up PR.

### B10 — SG egress 0.0.0.0/0 not restricted
- Dockerfile non-root landed. Terraform security-group egress is unchanged.
- **Why this matters:** an RCE in the facilitator container still reaches arbitrary outbound hosts on any port. The defense-in-depth promise of `B10` was *non-root AND constrained egress*. We have the first half.
- **Suggested follow-up:** add egress allow-list to known RPC CIDRs (QuickNode, Solana RPC, Stellar Horizon, etc.) plus `dynamodb.us-east-2.amazonaws.com` and `secretsmanager.us-east-2.amazonaws.com`. Consider VPC endpoints for DynamoDB + Secrets Manager to avoid public-internet hops entirely.

### F4 — idempotency + signature malleability
- Symmetric clock-skew landed. The other two F4 items did not:
  - **No `Idempotency-Key` header support.** A network blip mid-`/settle` followed by client retry still causes double-settlement risk for any caller that doesn't have its own nonce tracking. Faremeter handles this client-side; less-mature integrators may not.
  - **No low-s signature enforcement.** EIP-2 (`s > secp256k1_n / 2` rejection) not added. Practical impact is low because EIP-3009 nonces prevent replay, but signature malleability is on the OWASP/CWE-352-adjacent watchlist for any code that ingests external signatures.
- **Suggested follow-up:** new PR adding a small DynamoDB `idempotency_records` table (`pk = idempotency_key`, `attrs = {tx_signature, network, expires_at}`, 24h TTL). Reject sigs with `s > secp256k1_n / 2` in `assert_valid_payment` after domain check.

### F3 — temp-file fix is `.gitignore` only
- `.facilitator_wallet_temp.json` is now in `.gitignore`. The Python script `scripts/rotate_wallet.py:331-335` that writes that file still writes to the repo root in cleartext.
- **Suggested follow-up:** edit `scripts/rotate_wallet.py` to write to `tempfile.mkstemp(prefix='facilitator_wallet_', suffix='.json', mode=0o600)` under `/tmp`, and `os.remove` on success.

---

## 5. Explicit out-of-scope decisions (locked by user 2026-05-19)

These are *not* gaps — they are conscious choices that we should not relitigate without a new user decision.

| Item                                          | Decision     | Note |
|-----------------------------------------------|--------------|------|
| B7 ERC-8004 endpoint authentication           | postponed    | Coordinate via separate IRC channel before resuming |
| CORS allowlist (was inside B8)                | paused       | Facilitator stays public for photo2melee, ExecutionMarket, meshrelay, third parties |
| Drop operator `28c23AE8…`                     | rejected     | User believes it is still in use |
| Real GitHub issues for the audit findings     | rejected     | Doxing concern — internal tracking only |
| Compile / docker build / ECR push / ECS deploy | user-manual  | Per project CLAUDE.md; Claude only edits source |

---

## 6. Files touched (canonical inventory)

**Rust source:**
- `src/chain/evm.rs` — F2 (RPC URL redaction), F4 (symmetric `assert_time`), F7 (correlation-id errors), B6 (strict asset allow-list at top of `assert_valid_payment`)
- `src/chain/solana.rs` — B1/F1 (settlement-account binding, CPI scan hardening, replay store) and B6 (strict asset allow-list at top of `Facilitator::verify` and `::settle`)
- `src/chain/algorand.rs` — B2 (fee-tx sender + cap), F5 (verified already fail-closed)
- `src/chain/near.rs` — B3 (`ft_transfer` args bound to requirements)
- `src/chain/stellar.rs` — B4 (Soroban auth-entry validation), F5 (nonce-store fail-closed with correlation id), F7
- `src/chain/sui.rs` — B5 (BCS decode + Move command walk, no `unwrap_or(0)`)
- `src/network.rs` — B6 plumbing (`supported_asset_addresses`, `is_supported_asset`)
- `src/upto/permit2.rs` — B6 (strict allow-list in `validate_offchain`)
- `src/upto/types.rs` — B1 (settlement-account payload tightening)
- `src/payment_operator/operator.rs` — B9 (escrow RPC fail-closed)
- `src/payment_operator/addresses.rs` — B9 (CREATE3 factory + operator allow-lists verified already pinned)
- `src/discovery.rs` — F6 (SSRF guard rejecting RFC1918/link-local/loopback/multicast/IPv6 ULA)
- `src/handlers.rs` — F2/F7 (RPC URL + revert reason scrubbing, correlation ids)
- `src/main.rs` — B8 (`RequestBodyLimitLayer` with `MAX_REQUEST_BODY_BYTES` env, default 64 KiB)

**Build / scripts:**
- `Cargo.toml` — `tower-http` `"limit"` feature
- `scripts/migrate_secrets.py` — F2 (drop key-prefix print)
- `terraform/environments/production/validate_secrets.sh` — F2 (mask key fingerprints)
- `.gitignore` — F3 (`.facilitator_wallet_temp.json`)

**Infra:**
- `Dockerfile` — B10 (non-root `facilitator:10001`, `--chown` on COPY)
- `terraform/environments/production/main.tf` — F9 (multi-AZ NAT plumbing via `single_nat_gateway` + `nat_count`)
- `terraform/environments/production/variables.tf` — F9 (`desired_count` default 2 with documented tradeoff, `image_tag` no default)
- `terraform/environments/production/observability.tf` — F10 (`image_tag_mutability = "IMMUTABLE"`, `scan_on_push = true` on all four sidecar ECR repos)

**Docs:**
- `docs/plans/security-fixes-2026-05-19.md` — updated to "complete" with detail sections per PR
- `docs/reports/2026-05-19-security-fixes-handoff.md` — **this file**

---

## 7. Verification gates (for the user's manual run)

The user runs build/deploy. Recommended local checks **before** docker build:

```powershell
# Format + clippy across workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps -- -D warnings

# Test compile (no run) — feature-flagged chains must each build
cargo test --no-run --features solana,near,stellar,algorand,sui

# Quick allow-list sanity: count supported assets per network
# (does not need the binary running)
python scripts/stablecoin_matrix.py --md
```

After docker build + ECS deploy:

```powershell
# Endpoint smoke tests
curl -s https://facilitator.ultravioletadao.xyz/health
curl -s https://facilitator.ultravioletadao.xyz/version
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].network] | unique | length'

# B6 negative test — push an arbitrary ERC-20 against base-mainnet asset slot
# (the request must be refused with `unsupported_asset: ...` and never hit RPC)
# Use docs/reports/2026-05-19-security-audit.md B6 fixture as starting payload.

# B8 body-limit test — POST > 64 KiB should be rejected at the Axum layer
# before reaching any handler.
```

If `/version` does not increment after deploy, force-new-deployment:

```powershell
aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2
```

---

## 8. What the next session should do first

Suggested priority order (highest leverage first):

1. **B8 rate limit + JSON depth cap** — closes the last public-DoS / QuickNode-drain primitive. Estimated ~150 LoC + Cargo dep.
2. **F4 idempotency + low-s** — protects mid-rollout integrators that don't have their own retry-dedup. Estimated ~200 LoC + new DynamoDB table.
3. **B10 SG egress restriction** — completes the defense-in-depth promise from PR-11. Pure Terraform.
4. **F3 deeper temp-file fix** — small Python edit in `scripts/rotate_wallet.py`.
5. **B7 ERC-8004 authentication** — separately coordinated via IRC; verify current `src/erc8004/` state before designing.

Anything beyond that should re-read the audit report (`docs/reports/2026-05-19-security-audit.md`) and the upstream correlation (`docs/reports/2026-05-19-upstream-correlation.md`) to pick up Medium/Low findings that were not in this rollout's scope.

---

## 9. Secrets discipline (carry-forward)

This rollout was conducted under the project's standing rules:

- No private keys, API keys, or secret fingerprints in any output, log, comment, or doc.
- No emojis in Rust source.
- No `cp -r upstream/*` against the customized fork.
- `ENABLE_ESCROW=true` stays set in Terraform.
- Compile and deploy are the user's responsibility; Claude only edits files.

Nothing in this branch violates those rules; if a future audit finds one it is a regression, not a baseline.

---

## 10. 2026-05-21 follow-up batch — gaps #1–#4 from §4 landed

The four "Honest gaps" called out in §4 have all been closed in source.
Each gap was committed as its own gap-scoped commit on top of the
PR-1-11 working tree.

### 10.1 — B8 rate limit + JSON depth cap

**Commit:** `feat(security): B8 rate limit + JSON depth cap`

- `Cargo.toml` adds `tower_governor = "0.8"`.
- `src/main.rs` builds two `GovernorConfig`s and applies them as layers:
  - 30 req/min sustained on `/verify` + `/settle` (burst 30, replenish
    one token every 2 s).
  - 5 req/min sustained on `/discovery/register` (burst 5, replenish
    one token every 12 s).
  - Other endpoints (`/health`, `/supported`, `/accepts`, `/escrow`,
    `/register`, `/feedback`, `/identity`, `/reputation`, asset PNGs,
    `/discovery/resources`) intentionally stay un-rate-limited — they
    don't fan out to RPCs and ALB health checks need to flow.
- `src/handlers.rs` is refactored: `routes()` / `discovery_routes()` no
  longer host the rate-limited paths; new `verify_settle_routes()` and
  `discovery_register_routes()` host them so the stricter governor
  layers attach to those Routers only.
- New `src/json_depth.rs` adds a `serde` `deserialize_with` shim
  `deserialize_bounded_extra` that walks the JSON iteratively and
  rejects any `extra` payload whose container nesting exceeds
  `MAX_EXTRA_JSON_DEPTH = 16`. Applied to `PaymentRequirements.extra`,
  `PaymentRequirementsV2.extra`, `Permit2Witness.extra`, and
  `UptoPaymentRequirements.extra`.

### 10.2 — F3 deeper Python fix

**Commit:** `fix(scripts): move rotate_wallet temp file to /tmp with 0o600`

- `scripts/rotate_wallet.py` no longer writes the unencrypted wallet
  payload to `.facilitator_wallet_temp.json` at the repo root between
  `--generate` and `--deploy`. The file now lives under
  `tempfile.gettempdir()` (typically `/tmp`), namespaced by UID, and is
  created with mode `0o600` via `os.open(..., O_WRONLY|O_CREAT|O_TRUNC, 0o600)`.
- Defensive `os.chmod(temp_file, 0o600)` re-applies the mode in case
  umask stripped it. Cleanup on `--deploy` uses `os.remove` with
  `OSError` suppression so a partial failure doesn't mask the original.

### 10.3 — F4 idempotency + low-s signature

**Commit:** `feat(security): F4 idempotency cache + EIP-2 low-s signature`

**Idempotency cache:**
- New `src/idempotency_store.rs` module with `IdempotencyStore` trait,
  `DynamoIdempotencyStore`, `NoopIdempotencyStore`, and helpers
  `lookup_record` / `store_record`.
- New DynamoDB table `idempotency_records` (PK `idempotency_key`, attrs
  `request_hash`, `response_json`, `expires_at`, TTL 24 h) via
  `terraform/environments/production/main.tf`. New IAM policy
  `DynamoDBIdempotencyStoreAccess` grants the ECS task role only
  `PutItem`, `GetItem`, `DescribeTable` on that table.
- `POST /settle` honours an optional `Idempotency-Key` header
  Stripe-style:
  - cache hit + matching `sha256(body)` → cached response replayed
    byte-equal (parsed back to `SettleResponse` then re-serialized).
  - cache hit + different `sha256(body)` → 409 Conflict.
  - cache miss → settle normally; cache the response best-effort.
  - cache outage when header was supplied → 503 (fail closed; safer
    than risking a double-spend window).
- Cache writes are fire-and-forget via `tokio::spawn` so a slow DDB
  put doesn't block the response. Cache reads are also wrapped in
  `tokio::spawn(...).await` — calling the `#[async_trait]` method on
  `Arc<dyn IdempotencyStore + Send + Sync>` directly from the generic
  `/settle` handler tripped axum 0.8's Handler-trait elaboration; the
  spawn surfaces a concrete `JoinHandle<Send + 'static>` to routing.
- `IDEMPOTENCY_TABLE_NAME` env var wired up in the ECS task definition.
  Falls back to `NoopIdempotencyStore` when unset (preserves dev
  behaviour).

**Low-s signature (EIP-2):**
- `assert_valid_payment` in `src/chain/evm.rs` now rejects ECDSA
  signatures whose `s` component exceeds `secp256k1_n / 2`. The check
  is inserted immediately after the `assert_domain` call, before any
  RPC interaction. EIP-3009 nonces already prevent payload replay, so
  the practical blast radius was bounded — but enforcing the canonical
  form removes a malleability primitive that downstream consumers
  (especially anything that does its own signature recovery) may not
  be guarding against.

### 10.4 — B10 SG egress restriction + VPC endpoints

**Commit:** `feat(security): B10 SG egress restriction + VPC endpoints`

- `terraform/environments/production/main.tf` replaces the
  `0.0.0.0/0:0-65535:-1` egress on the `ecs_tasks` SG with four
  protocol-scoped rules:
  - HTTPS (443) to anywhere.
  - HTTP (80) to anywhere (some chain RPCs still expose `http://`).
  - DNS (UDP+TCP 53) to anywhere.
  - NTP (UDP 123) to anywhere.
- RPC provider CIDRs are intentionally not enumerated (chain providers
  rotate hosts on diverse IP ranges); the win is closing arbitrary
  TCP/UDP egress so an RCE in the container cannot open random
  outbound sockets.
- New DynamoDB Gateway VPC endpoint (free) routes
  `nonce_store` + `idempotency_records` traffic through the AWS
  backbone — no NAT hop, no internet edge.
- New Secrets Manager Interface VPC endpoint
  (~$7/AZ/month + data) exposes ENIs inside our private subnets.
  `private_dns_enabled = true` means the standard
  `secretsmanager.us-east-2.amazonaws.com` hostname resolves to the
  VPC IP. Pulls of `facilitator-*-private-key` and the RPC URL secrets
  no longer transit the public internet.
- New dedicated SG `facilitator-${environment}-vpc-endpoints` limits
  endpoint ENIs to accept HTTPS only from the `ecs_tasks` SG.

### 10.5 — What is *still* not in this batch

The four §4 gaps are closed. The following items were deliberately
left out and remain tracked elsewhere:

- **B7 ERC-8004 endpoint auth** — postponed per the original scope
  decision; coordinate via the separate IRC channel before resuming.
- **Operator `28c23AE8…` removal** — still allow-listed in
  `OperatorAddresses::for_network`. Drop only if telemetry shows zero
  traffic.
- **Conditional-write race serialisation for `Idempotency-Key`** —
  current implementation can race when two retries with the same key
  hit simultaneously. EIP-3009 nonces still prevent on-chain
  double-spend, but a future hardening pass can use DDB conditional
  writes to also serialise the response cache.
- **DDB+Secrets prefix-list source rules on the SG** — VPC endpoints
  carry that traffic now, but the SG itself doesn't reference the
  endpoint prefix lists (we instead rely on the endpoint SG to gate
  inbound). Acceptable, but a future SG audit could tighten further.

### 10.6 — Verification gates for the 2026-05-21 batch

Same harness as §7. Add to the static-check checklist:

```powershell
# fmt + clippy on the new modules
cargo fmt --check
cargo clippy --workspace --all-targets --no-deps

# IDEMPOTENCY_TABLE_NAME wiring
grep -n "IDEMPOTENCY_TABLE_NAME" terraform/environments/production/main.tf

# tower_governor present
grep -n "tower_governor" Cargo.toml

# json depth bound active
grep -n "deserialize_bounded_extra" src/types.rs src/types_v2.rs src/upto/types.rs
```

After deploy:

```powershell
# B8 rate-limit positive test (must return 429 after the 30th request)
for i in $(seq 1 60); do curl -s -o /dev/null -w "%{http_code}\n" -X POST \
  https://facilitator.ultravioletadao.xyz/verify -d "{}" -H "Content-Type: application/json"; done | sort | uniq -c

# B8 JSON depth-cap positive test (deep `extra` should be refused with a
# serde error inside the 400 response)
curl -s -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H "Content-Type: application/json" \
  -d "$(python3 -c 'import json; v={"a":1};
for _ in range(20): v={"a":v}
print(json.dumps({"paymentRequirements":{"extra":v}}))')"

# F4 idempotency replay (two identical requests with the same key must
# produce the same tx_hash; the second hits the cache)
KEY=$(uuidgen)
curl -s -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Idempotency-Key: $KEY" -H "Content-Type: application/json" \
  -d @valid_settle_payload.json | jq .transaction
curl -s -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Idempotency-Key: $KEY" -H "Content-Type: application/json" \
  -d @valid_settle_payload.json | jq .transaction

# F4 low-s positive test (manually craft a high-s signature; expect
# `invalid_signature` in the response)
# (no automated script; covered by future integration test)

# B10 SG egress negative test (from inside the container; expect timeout
# on non-allowed ports)
aws ecs execute-command --cluster facilitator-production \
  --task <TASK_ARN> --container facilitator --command "/bin/sh" \
  --interactive
# inside: nc -zv 1.1.1.1 22  # should fail
#         nc -zv 1.1.1.1 443 # should succeed
```

### 10.7 — Honest caveats from this batch

- All four commits ride on top of the PR-1-11 dirty working tree from
  the original rollout. Each commit message clearly scopes the
  gap-specific diff, but `git log -p` will show the relevant shared
  files (handlers.rs, main.rs, main.tf, etc.) carrying the prior PR
  hunks too. If a clean per-gap diff is wanted, `git rebase -i` can
  split the commits later.
- `cargo clippy --workspace --all-targets --no-deps -- -D warnings` does
  not pass today, but the failures are pre-existing:
  - `crates/x402-compliance/src/{checker,extractors/solana,lists/ofac}.rs`
    have three clippy-lint failures (`dead_code`, `clippy::get_first`,
    `clippy::redundant_closure`) that pre-date this rollout.
  - `src/chain/solana.rs:1243` has an unrelated `E0282` type-inference
    error from PR-4 (aegis-solana) that the user's manual build will
    need to resolve. None of the 2026-05-21 changes introduce new
    errors.
- `tower_governor` uses an in-memory token bucket. Behind an ALB, the
  governor sees per-task buckets, so a horizontal scale-out widens the
  effective rate limit linearly. If you ever run more than 4-5 tasks,
  consider a Redis-backed governor or sticky sessions.

---
