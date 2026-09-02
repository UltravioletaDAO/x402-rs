# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## File Organization

This repository maintains a strict file organization structure. The root directory contains ONLY:

**Essential files in root:**
- `README.md` - Project overview
- `CLAUDE.md` - This file (Claude Code instructions)
- Release marker: `VERSION` - the release version lives here, NOT in `Cargo.toml`. Do not move or delete it.
- Build/tooling: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `justfile`, `Dockerfile`, `docker-compose.yml`, `docker-compose.observability.yml`, `LICENSE`, dotfiles (`.env.example`, `.gitignore`, `.dockerignore`, `.cargoignore`, `.editorconfig`)

**All other files organized in directories:**
- `docs/` - ALL documentation (CHANGELOG.md, CUSTOMIZATIONS.md, DEPLOYMENT.md, etc.)
- `static/` - Landing page HTML, images, assets
- `scripts/` - Python scripts, shell scripts, deployment tools
- `config/` - Configuration files (blacklist.json, prometheus.yml, etc.)
- `terraform/` - Infrastructure as code, task definitions
- `src/` - Rust source code
- `crates/` - Workspace crates
- `tests/` - Test suites
- `guides/` - Operator guides (ADDING_NEW_CHAINS.md)
- `lambda/` - AWS Lambda sources (`balances/handler.py` = authoritative wallet addresses)
- `examples/` - Workspace example crates (x402-axum-example, x402-reqwest-example)
- `abi/`, `contracts/` - Solidity ABIs and contract sources
- `docker/`, `grafana/`, `conductor/` - Container and observability assets
- `.github/` - CI/CD (`workflows/ci.yaml` - push to main deploys to production)
- `.unused/` - **IGNORED** (contains secrets, never commit!)

## Project Overview

This is the **x402-rs Payment Facilitator** - a standalone Rust-based service enabling gasless micropayments across multiple blockchain networks using the HTTP 402 Payment Required protocol. The facilitator acts as a settlement intermediary, verifying EIP-3009 payment authorizations and submitting them on-chain.

**Key characteristics**:
- Production-ready service deployed on AWS ECS
- Multi-chain support (EVM + SVM/Solana incl. Fogo + NEAR + Stellar + Algorand + Sui + XRPL)
- Custom Ultravioleta DAO branding (landing page, logos)
- Forked from upstream [x402-rs/x402-rs](https://github.com/x402-rs/x402-rs)

**IMPORTANT: Network and Stablecoin Verification**

Network counts and stablecoin coverage in documentation get outdated quickly. **ALWAYS verify from source:**

**To verify network count:**
```bash
# NOTE: /supported lists every chain TWICE (v1 name + CAIP-2 alias), so this counts
# identifier STRINGS, not networks. It returned 78 on 2026-08-10.
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].network] | unique | length'

# v1 names only - closer to a real chain count
curl -s https://facilitator.ultravioletadao.xyz/supported | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l

# Canonical mainnet payment-network count (21). The old jq substring filter does NOT
# work - CAIP-2 ids carry no "sepolia"/"testnet" substring, so it returned 55.
python scripts/verify_landing_canonical.py
```

**To verify stablecoin support matrix (GOLDEN SOURCE):**
```bash
# Run the stablecoin matrix script - this parses src/network.rs directly
python scripts/stablecoin_matrix.py

# Output as JSON for programmatic use
python scripts/stablecoin_matrix.py --json

# Output as Markdown table
python scripts/stablecoin_matrix.py --md
```

**NEVER assume stablecoin coverage** - always run `python scripts/stablecoin_matrix.py` to get the real matrix.

**When adding a new network or stablecoin:**
1. Add the deployment to `src/network.rs`
2. Run `python scripts/stablecoin_matrix.py` to verify
3. Update README.md with the new counts and tables

## Development Commands

### Build and Run
```bash
# Build (release mode)
cargo build --release

# Run locally (requires .env configuration)
cargo run --release

# Run with debug logging
RUST_LOG=debug cargo run

# Build all workspace members
just build-all
```

### Testing
```bash
# Integration tests (requires running facilitator)
cd tests/integration
python test_usdc_payment.py   # NO flags exist: hardcoded to the PRODUCTION facilitator + Base MAINNET USDC

# Test all endpoints
python test_endpoints.py

# Quick payment test
python test_quick_payment.py

# Full x402 protocol test
python test_x402_integration.py
```

### Linting and Formatting
```bash
# Format all code
just format-all  # or just fmt-all

# Run clippy linter
just clippy-all

# Format single workspace member
cd crates/x402-axum && cargo fmt
```

**Gotcha**: the `*-all` recipes predate `crates/x402-compliance` and skip it, but CI tests it
(`.github/workflows/ci.yaml`). Before pushing also run:
`cargo clippy -p x402-compliance && cargo test -p x402-compliance -- --test-threads=1`

### Docker
```bash
# Build and run with Docker Compose
docker-compose up -d

# View logs
docker-compose logs -f facilitator

# FAST BUILD (5-10x faster on WSL2) - ALWAYS USE THIS
./scripts/fast-build.sh v1.32.1           # Build only
./scripts/fast-build.sh v1.32.1 --push    # Build + push to ECR

# Legacy build (slow on WSL2, only use if fast-build fails)
./scripts/build-and-push.sh v1.0.0
```

**Why fast-build?** The repo lives on `/mnt/z/` (Windows NTFS via WSL2 9P). Docker builds
cross the WSL2-Windows boundary for every file operation, making builds 5-10x slower.
`fast-build.sh` rsyncs to `~/x402-rs-build` (native ext4) first, then builds there.
First run ~3min, subsequent runs ~35s.

**Caveat since the VERSION-file move:** `fast-build.sh` does NOT pass
`--build-arg FACILITATOR_VERSION`, so an image it builds answers `/version` with `dev`
(`Dockerfile:88`). CI passes it, and so do `build-and-push.sh` / `deploy-to-ecs.sh`.
Use fast-build for local iteration; for a hand-shipped release image use
`build-and-push.sh` or add the build-arg yourself.

### Diagnostics
```bash
# Check configuration
python scripts/check_config.py

# Diagnose payment issues (Base mainnet only - no --network flag exists)
python scripts/diagnose_payment.py

# Verify full stack
python scripts/verify_full_stack.py

# Compare USDC contract addresses
python scripts/compare_usdc_contracts.py
```

## Architecture

### Core Components

**src/main.rs**: HTTP server entrypoint
- Axum-based router with x402 protocol endpoints
- OpenTelemetry tracing integration
- CORS support for cross-origin clients
- Serves custom Ultravioleta DAO landing page and static assets

**src/network.rs**: Network definitions (42 `Network` enum variants; 39 served in prod - Sei, SeiTestnet and XdcMainnet are enum-only). Verify: `curl -s https://facilitator.ultravioletadao.xyz/supported | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l`
- `Network` enum with chain IDs and display names
- `NetworkFamily` (Evm, Solana, Near, Stellar, Xrpl, Algorand, Sui - 7 variants, `src/network.rs:275`) for multi-family dispatch
- Static USDC/token deployment addresses per network
- **CRITICAL**: nearly everything beyond Base/Avalanche/Solana is ours - HyperEVM, Polygon, Optimism, Celo, Ethereum, Arbitrum, Unichain, Monad, BSC, Scroll, SKALE, Robinhood, XDC, Sei, NEAR, Stellar, XRPL, Fogo, Algorand, Sui. Preserve ALL of them on upstream merges.

**src/handlers.rs**: HTTP request handlers
- `get_root()` (`src/handlers.rs:1248`) - **Custom handler** serving the Ultravioleta DAO landing page via `include_str!("../static/index.html")`. `get_index()` (`:1289`) is a thin alias - the `include_str!` is NOT inside it.
- Asset handlers for logos (favicon, network logos)
- `/verify` - Verify payment authorization structure
- `/settle` - Submit payment on-chain
- `/supported` - List available networks/schemes
- `/health` - Health check endpoint
- `/accepts` - Negotiate payment requirements (Faremeter middleware compatibility)
- Plus most of the live surface, registered in router builders in handlers.rs and merged in `src/main.rs`: discovery/bazaar, `/blacklist`, `/escrow/state`, ERC-8004 (`/register`, `/identity`, `/reputation`, `/feedback`), `/events`, `/stats`, `/api/stats`, `/transactions`, `/version`. Enumerate with `grep -rn '\.route(' src/` (67 routes) before assuming an endpoint does not exist.

**src/facilitator.rs**: Core payment logic trait
- `Facilitator` trait defining verification and settlement interface
- Network-agnostic abstraction over payment operations

**src/facilitator_local.rs**: Local facilitator implementation
- `FacilitatorLocal` implements `Facilitator` trait
- Delegates to the 7 chain modules in `src/chain/` (evm, solana, near, stellar, sui, algorand, xrpl - see `src/chain/mod.rs`)
- Manages provider cache for RPC connections

**src/chain/**: Chain-specific payment logic
- `chain/evm.rs` - EIP-3009 payment verification and settlement for EVM chains
- `chain/near.rs`, `chain/stellar.rs`, `chain/sui.rs`, `chain/algorand.rs`, `chain/xrpl.rs` - the other chain families
- `chain/solana.rs` - Solana token transfer authorization support
  - **Smart wallet support** (v1.36.0): Two-path verification for Squads, Crossmint, SWIG wallets
    - Path 1: Top-level TransferChecked (standard wallets, unchanged)
    - Path 2: Simulation inner instruction scanning (CPI-based smart wallets)
  - **Settlement account support** (v1.36.0): For Crossmint custodial wallets that can only `sendTransaction`
    - `SettlementAccountPayload` type (**defined in `src/types.rs:552`**, only consumed by solana.rs): `{ transactionSignature, settleSecretKey, settlementRentDestination }`
    - Verify: fetches on-chain tx, checks confirmation, validates USDC transfer from token balances
    - Settle: sweeps USDC from settlement account to payTo (creates ATA if needed, transfers, closes)
- Handles signature verification, nonce validation, on-chain submission

**src/provider_cache.rs**: RPC provider management
- Caches Ethereum providers per network
- Loads RPC URLs from environment variables
- Initializes at startup with fail-fast behavior

**src/timestamp.rs**: EIP-3009 timestamp utilities
- Handles `validAfter`/`validBefore` timestamp validation
- See `docs/EIP3009_TIMESTAMP_BEST_PRACTICES.md` for context

**src/types.rs**: Protocol types and serialization
- `PaymentPayload`, `TokenAsset`, `TokenDeployment`
- Serde integration for x402 JSON protocol

**Subsystems not covered above** (all declared in `src/main.rs`; read the module before assuming a feature is missing):
- `discovery*.rs` (8 files) - Bazaar catalog: crawler, aggregator, curation, health, attestation, security, store
- `erc8004/` - Trustless Agents: `/register`, `/identity`, `/reputation`, `/feedback` (EVM + Solana)
- `escrow.rs`, `payment_operator/`, `upto/` - x402r escrow, PaymentOperator, Permit2 `upto` scheme
- `events.rs`, `transaction_store{,.rs}` - SSE stream + DynamoDB index behind `/events`, `/transactions`, `/api/stats`
- `nonce_store.rs`, `writer_lease.rs`, `idempotency_store.rs` - concurrency and replay control
- `blocklist.rs`, `redact.rs`, `sig_down.rs`, `json_depth.rs`, `fhe_proxy.rs` - compliance and hardening
- `dx402/` - **DX402 `durable-evidence`** (v1.75.0): seals a paid response to the payer's own public key and anchors it. OFF unless `ENABLE_DX402=true`. See below and `docs/DX402.md`.
- `version.rs` (resolves the VERSION file at runtime), `telemetry.rs`, `openapi.rs`, `from_env.rs`

### Workspace Structure

This is a Cargo workspace with multiple crates:

**Root crate (x402-rs)**: Main facilitator service
**crates/x402-axum**: Axum middleware for x402 protocol (library)
**crates/x402-reqwest**: Reqwest client for x402 payments (library)
**crates/x402-compliance**: x402 protocol conformance suite (library; CI tests it alongside axum/reqwest)
**examples/x402-axum-example**: Example server using x402-axum
**examples/x402-reqwest-example**: Example client using x402-reqwest

## Critical Customizations

**⚠️ THESE FILES ARE PROTECTED - DO NOT OVERWRITE FROM UPSTREAM:**

1. **static/index.html** (~225KB) - Ultravioleta DAO branded landing page
   - Replaces upstream's simple "Hello" message
   - Contains network grid, API documentation, DAO branding
   - **Recovery**: `git checkout HEAD~1 -- static/index.html`

2. **src/handlers.rs** - `get_root()` function (aliased by `get_index()`)
   - Uses `include_str!("../static/index.html")` instead of plain text
   - Embeds HTML at compile time for performance
   - **Must preserve this pattern when merging upstream changes**

3. **static/*.png** - Network and token logos (29 PNG files served directly from `static/`; there is NO `static/images/` directory)
   - Current set: `ls static/*.png`
   - Never overwrite from upstream

4. **src/network.rs** - Custom networks added beyond upstream
   - HyperEVM mainnet/testnet (Chain IDs: 999, 333)
   - Polygon mainnet/Amoy testnet (Chain IDs: 137, 80002)
   - Optimism mainnet/Sepolia testnet (Chain IDs: 10, 11155420)
   - Celo mainnet/Sepolia testnet (Chain IDs: 42220, 44787)
   - Solana mainnet/devnet
   - Sui mainnet/testnet (requires `--features sui`)
   - **Merge strategy**: Preserve ALL custom networks when pulling upstream

5. **Rust Edition** - edition 2021 (`Cargo.toml:5`)
   - This is NOT a low-MSRV guarantee: locked deps require **Rust >= 1.91** (aws-smithy-* 1.91.1). There is no `rust-version` key and the toolchain is unpinned `stable` (rust-toolchain.toml, ci.yaml, `FROM rust:bullseye`).
   - Edition 2024 needs only 1.85, so MSRV no longer blocks it - only syntax/lint churn does.
   - Downgraded in the v0.9.1 merge for broader compatibility

See `docs/CUSTOMIZATIONS.md` for complete documentation of all customizations and merge strategies.

## Configuration

### Environment Variables

Copy `.env.example` to `.env` and configure:

**Required** (Separate wallets per environment - RECOMMENDED):
- `EVM_PRIVATE_KEY_MAINNET` - Facilitator wallet for mainnet EVM chains (leave empty for AWS Secrets Manager)
- `EVM_PRIVATE_KEY_TESTNET` - Facilitator wallet for testnet EVM chains (leave empty for AWS Secrets Manager)
- `SOLANA_PRIVATE_KEY_MAINNET` - Facilitator wallet for Solana mainnet (leave empty for AWS Secrets Manager)
- `SOLANA_PRIVATE_KEY_TESTNET` - Facilitator wallet for Solana devnet (leave empty for AWS Secrets Manager)
- `SUI_PRIVATE_KEY_MAINNET` - Facilitator wallet for Sui mainnet (leave empty for AWS Secrets Manager)
- `SUI_PRIVATE_KEY_TESTNET` - Facilitator wallet for Sui testnet (leave empty for AWS Secrets Manager)
- `NEAR_PRIVATE_KEY_MAINNET` / `NEAR_PRIVATE_KEY_TESTNET` + `NEAR_ACCOUNT_ID_MAINNET` / `NEAR_ACCOUNT_ID_TESTNET`
- `STELLAR_PRIVATE_KEY_MAINNET` / `STELLAR_PRIVATE_KEY_TESTNET`
- `ALGORAND_MNEMONIC_MAINNET` / `ALGORAND_MNEMONIC_TESTNET` (25-word mnemonic)
- `XRPL_PRIVATE_KEY_MAINNET` / `XRPL_PRIVATE_KEY_TESTNET` (feature `xrpl`; prod runs relay-mode with no key)
- Authoritative list of variable NAMES: `grep -n 'pub const ENV_' src/from_env.rs`

**Backward Compatibility** (DEPRECATED):
- `EVM_PRIVATE_KEY` - Generic wallet for ALL EVM chains (only used if network-specific keys are not set)
- `SOLANA_PRIVATE_KEY` - Generic wallet for ALL Solana networks (only used if network-specific keys are not set)

**RPC URLs** (defaults provided, override for premium endpoints). Canonical names live in `src/from_env.rs` - **mainnet vars have NO `_MAINNET` suffix** (the single exception is `RPC_URL_XRPL_MAINNET`):
- Mainnet: `RPC_URL_BASE`, `RPC_URL_AVALANCHE`, `RPC_URL_CELO`, `RPC_URL_HYPEREVM`, `RPC_URL_POLYGON`, `RPC_URL_OPTIMISM`, `RPC_URL_ETHEREUM`, `RPC_URL_ARBITRUM`, `RPC_URL_BSC`, `RPC_URL_UNICHAIN`, `RPC_URL_SCROLL`, `RPC_URL_SKALE_BASE`, `RPC_URL_MONAD`, `RPC_URL_ROBINHOOD`, `RPC_URL_SOLANA`, `RPC_URL_FOGO`, `RPC_URL_SUI`, `RPC_URL_NEAR`, `RPC_URL_STELLAR`, `RPC_URL_ALGORAND`, `RPC_URL_XRPL_MAINNET`, `RPC_URL_SEI`, `RPC_URL_XDC`
- Testnet: same stem + `_SEPOLIA` / `_TESTNET` / `_FUJI` / `_AMOY` / `_DEVNET`. Celo testnet is `RPC_URL_CELO_SEPOLIA` - `RPC_URL_CELO_ALFAJORES` does not exist.
- Authoritative list (42 vars): `grep -oE 'RPC_URL_[A-Z0-9_]+' src/from_env.rs | sort -u`

**Optional**:
- `OTEL_EXPORTER_OTLP_ENDPOINT` - OpenTelemetry endpoint for observability
- `RUST_LOG` - Logging level. **Code default is `trace`** (`src/telemetry.rs:290`), not info - production only gets `info` because the ECS task definition sets it explicitly. When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, `RUST_LOG` is ignored entirely and the level is hardcoded (`src/telemetry.rs`).
- (`QUICKNODE_BASE_RPC` is dead: it survives in `.env.example` but no code reads it. Use `RPC_URL_<NETWORK>`.)
- `PORT`, `HOST` - Server binding (default: 8080, 0.0.0.0)

### AWS Secrets Manager (Production)

Leave wallet environment variables empty in `.env`. The facilitator will fetch them from AWS Secrets Manager if running on ECS with appropriate IAM permissions.

**IMPORTANT**: As of v1.3.0, the facilitator uses separate wallets for mainnet and testnet environments. This prevents the critical bug where testnet transactions were signed with mainnet keys.

Secret names (configured in infrastructure):
Golden source is `terraform/environments/production/secrets.tf`. The naming is NOT uniform - EVM/Solana/NEAR use `<chain>-<env>-<kind>`, Sui/Stellar use `<chain>-keypair-<env>`, Algorand uses `algorand-mnemonic-<env>`:

- `facilitator-evm-mainnet-private-key` / `facilitator-evm-testnet-private-key` - EVM wallets
- `facilitator-solana-mainnet-keypair` / `facilitator-solana-testnet-keypair` - Solana wallets
- `facilitator-near-mainnet-keypair` / `facilitator-near-testnet-keypair` - NEAR (JSON keys `private_key` + `account_id`)
- `facilitator-stellar-keypair-mainnet` / `facilitator-stellar-keypair-testnet` - Stellar
- `facilitator-sui-keypair-mainnet` / `facilitator-sui-keypair-testnet` - Sui
- `facilitator-algorand-mnemonic-mainnet` / `facilitator-algorand-mnemonic-testnet` - Algorand 25-word mnemonic
- `facilitator-rpc-mainnet` - premium mainnet RPC URLs, one JSON key per network: `base`, `avalanche`, `polygon`, `optimism`, `celo`, `hyperevm`, `ethereum`, `arbitrum`, `unichain`, `solana`, `near` (there is NO `sui` key - Sui/BSC/Scroll/SKALE/Monad/Fogo/Robinhood use free public endpoints declared inline in main.tf)
- `facilitator-rpc-testnet` - JSON keys: `solana-devnet`, `arbitrum-sepolia`, `near` (no `sui-testnet`)

**Legacy secrets** (deprecated, kept for backward compatibility):
- `facilitator-evm-private-key` - Generic EVM wallet (not recommended)
- `facilitator-solana-keypair` - Generic Solana wallet (not recommended)

### ⚠️ CRITICAL SECURITY: RPC URLs with API Keys

**NEVER** put RPC URLs containing API keys directly in ECS Task Definition environment variables. This is a CRITICAL security vulnerability because:

1. Task definitions are stored in plaintext and accessible to anyone with ECS read permissions
2. Task definition history is preserved, exposing keys even after rotation
3. API keys in URLs are visible in AWS Console, CLI output, and logs

**ALWAYS use AWS Secrets Manager references for RPC URLs with API keys:**

❌ **WRONG** (Exposes API key):
```json
{
  "name": "RPC_URL_ARBITRUM",
  "value": "https://node-name.arbitrum-mainnet.quiknode.pro/API_KEY_HERE/"
}
```

✅ **CORRECT** (Secure reference):
```json
{
  "name": "RPC_URL_ARBITRUM",
  "valueFrom": "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-rpc-mainnet-5QJ8PN:arbitrum::"
}
```

**When adding a new network with premium RPC:**

1. Add the RPC URL to the appropriate secret in AWS Secrets Manager:
   ```bash
   # For mainnet
   aws secretsmanager update-secret \
     --secret-id facilitator-rpc-mainnet \
     --region us-east-2 \
     --secret-string '{"network-name": "https://rpc-url-with-api-key"}'

   # For testnet
   aws secretsmanager update-secret \
     --secret-id facilitator-rpc-testnet \
     --region us-east-2 \
     --secret-string '{"network-name": "https://rpc-url"}'
   ```

2. Add the secret reference to the task definition's `secrets` array (NOT `environment`):
   ```json
   {
     "name": "RPC_URL_NETWORK_NAME",
     "valueFrom": "arn:aws:secretsmanager:REGION:ACCOUNT:secret:SECRET_NAME:KEY::"
   }
   ```

3. Public/free RPC endpoints (without API keys) can go directly in `environment` variables or `.env.example`

## Deployment

### AWS ECS (Production)

Infrastructure managed with Terraform in `terraform/environments/production/`.

**The deploy is automated.** Pushing to `main` runs `.github/workflows/ci.yaml`, which tests, builds the image, pushes it to ECR and `terraform apply -auto-approve`s it onto ECS (targeted at the task definition + service), then waits for the rollout and checks `/health`. The commands below are the manual fallback - do not run them in parallel with a CI deploy of the same commit.

```bash
# Initialize Terraform backend (once)
aws s3 mb s3://facilitator-terraform-state --region us-east-2
aws dynamodb create-table --table-name facilitator-terraform-locks \
  --attribute-definitions AttributeName=LockID,AttributeType=S \
  --key-schema AttributeName=LockID,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST --region us-east-2

# Create ECR repository (once)
aws ecr create-repository --repository-name facilitator \
  --image-scanning-configuration scanOnPush=true --region us-east-2

# Manual fallback build. NOTE: fast-build.sh does NOT pass --build-arg FACILITATOR_VERSION,
# so its image reports /version = "dev". For a hand-shipped release image use build-and-push.sh.
./scripts/fast-build.sh $(cat VERSION) --push

# Roll ECS to a new image - TARGETED, never a full apply
cd terraform/environments/production
terraform init
terraform apply -target=aws_ecs_task_definition.facilitator -target=aws_ecs_service.facilitator

# Update running service
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

A **full** `terraform apply` is not an image deploy: it also re-uploads the balances Lambda
and pulls in an ALB attribute modify. CI documents this and deliberately uses `-target`
(`.github/workflows/ci.yaml`). Use a full apply only for a deliberate infra change, and read
the whole plan first. Do NOT use `-refresh=false` (it invents drift).

**Production URL**: `https://facilitator.ultravioletadao.xyz`

**Cost**: NOT the ~$43-48/month this file used to claim. Egress is a **NAT Gateway** (`aws_nat_gateway` in main.tf, priced in-repo at ~$32/mo - the `use_nat_instance` tfvar is dead, referenced by no resource), and Fargate alone measured $36.04/mo in July 2026. On top of that: ALB, Container Insights, 3 DynamoDB tables, the balances Lambda, CloudWatch and a Secrets Manager interface endpoint. Cost-allocation tags are not activated, so per-project totals are not queryable - see `docs/COST_RIGHTSIZING_HANDOFF_2026-08-07.md`. Do not quote a monthly figure you have not sourced.

## Testing Approach

### Local Development Testing

1. Start facilitator locally: `cargo run --release`
2. Verify health: `curl http://localhost:8080/health`
3. Check branding: `curl http://localhost:8080/ | grep "Ultravioleta"`
4. List networks: `curl http://localhost:8080/supported`

### Integration Testing

Located in `tests/integration/`:

- `test_facilitator.py` - Full facilitator test suite (health, networks, payments)
- `test_usdc_payment.py` - USDC payment flow (Base, other EVM chains)
- `test_x402_integration.py` - x402 protocol compliance tests
- `test_complete_flow.py` - End-to-end buyer->facilitator->seller flow

**Run tests**: `cd tests/integration && python test_facilitator.py`

### Load Testing

Located in `tests/x402/load/`:
- `k6_load_test.js` - k6 load test (100+ TPS)
- `artillery_config.yml` - Artillery load profile
- Run: `k6 run --vus 100 --duration 5m tests/x402/load/k6_load_test.js`

### Protocol Testing

Located in `tests/x402/`:
- Python-based x402 protocol tests
- Verify/settle payload validation
- See `tests/x402/README.md` and `tests/x402/TROUBLESHOOTING.md`

### Crossmint Smart Wallet Testing

Located in `tests/crossmint-smart-wallet/`:
- End-to-end test for Crossmint custodial wallets on Solana mainnet
- Uses `@faremeter/middleware` + `@faremeter/wallet-crossmint` packages
- `server.mjs` - Mini paywall server supporting both standard and settlement account modes
- `test.mjs` - Automated test: checks balances, initializes Crossmint wallet, makes x402 payment
- `setup-wallet.mjs` - Wallet setup helper
- Tests both Path 1 (standard TransferChecked) and Path 2 (CPI inner instruction scanning)
- Requires: `CROSSMINT_API_KEY`, `CROSSMINT_WALLET` in `.env`, funded with SOL + USDC
- Run: `cd tests/crossmint-smart-wallet && npm install && node server.mjs` then `node test.mjs`

## Important Documentation

- **guides/ADDING_NEW_CHAINS.md** - Complete checklist and guide for adding new blockchain networks
- **docs/CUSTOMIZATIONS.md** - Detailed inventory of all customizations vs upstream
- **docs/CHANGELOG.md** - Version history and release notes. **Lags the shipped release** (top entry 1.64.0 while prod is 1.73.0) — for the real version use `curl -s https://facilitator.ultravioletadao.xyz/version` and the `VERSION` file.
- **docs/DEPLOYMENT.md** - Deployment procedures and infrastructure guide
- **docs/TESTING.md** - Complete testing guide
- **docs/WALLET_ROTATION.md** - Security procedures for rotating facilitator keys
- **docs/UPSTREAM_MERGE_STRATEGY.md** - How to safely merge upstream changes without losing branding
- **docs/EXTRACTION_MASTER_PLAN.md** - History of extracting facilitator from karmacadabra monorepo
- **docs/EIP3009_TIMESTAMP_BEST_PRACTICES.md** - Timestamp handling for payment authorizations
- **docs/ERC8004_SOLANA_INTEGRATION.md** - ERC-8004 Solana integration design and implementation
- **docs/ERC8004_SOLANA_SDK_GUIDE.md** - SDK guide for ERC-8004 Solana operations

## Troubleshooting

### "Invalid signature" errors
```bash
python scripts/diagnose_payment.py   # Base mainnet only - no --network flag exists
python scripts/compare_domain_separator.py
```

**CRITICAL: EIP-712 Domain Names Vary by Chain!**

Different chains use different domain names for the same stablecoin:

| Token | Usual name | Exceptions |
|-------|-----------|------------|
| EURC | `"Euro Coin"` (Ethereum, Avalanche) | `"EURC"` on Base |
| USDC | `"USD Coin"` - **including Base MAINNET** (`src/network.rs:733`) | `"USDC"` on Celo, HyperEVM, Unichain, Monad and most `-sepolia`/`-testnet` variants (Base Sepolia = `"USDC"`). The name FLIPS between a chain's mainnet and testnet (HyperEVM mainnet `"USDC"` vs testnet `"USD Coin"`). Bridged variants differ again: XDC `"Bridged USDC(XDC)"`, SKALE `"Bridged USDC (SKALE Bridge)"`. Never infer - grep `name:` in `src/network.rs`. |

The facilitator resolves domains in this priority order (`assert_domain()`, `src/chain/evm.rs:1588`):
1. Static lookup in `src/network.rs` (`find_known_eip712_metadata`) - for KNOWN deployments this WINS, and a differing client value is only logged as a warning
2. `PaymentRequirements.extra.name/version` - used only when the token is NOT in the static table
3. On-chain `token.name()`/`token.version()` calls (fallback)

**For tokens NOT in the static table, clients MUST provide domain info** (EURC and AUSD are already static, so `extra` is ignored for them - warn-only):
```json
{
  "paymentRequirements": {
    "asset": "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42",
    "extra": {
      "name": "EURC",
      "version": "2"
    }
  }
}
```

Key code locations:
- `src/network.rs:1489` - EURC_BASE uses `name: "EURC"` (value at `:1497`)
- `src/network.rs:1473` - EURC_ETHEREUM uses `name: "Euro Coin"` (value at `:1481`)
- `src/chain/evm.rs:assert_domain()` - Domain resolution logic

### "The facilitator is returning 5xx" — confirm before chasing

Establish the actual status code first. A report of "500s" has already once turned
out to be zero 500s (the real symptom was HTTP 429 from a mis-sized rate limit,
2026-07-24). Two things make a healthy facilitator look broken:

1. **Outbound failures read like our own.** The aggregator and crawler fetch from
   upstream bazaar feeds, several of which are permanently broken. Those log lines
   carry `direction=outbound` and `upstream_error=...` and may quote the upstream's
   own status ("HTTP 500"). They are not our responses.
2. **Our responses are the ones with `status=NNN`** (emitted by `telemetry`).

```bash
# 1) Ground truth from the ALB — is anything actually 5xx?
DIM=$(aws elbv2 describe-load-balancers --region us-east-2 \
  --query "LoadBalancers[?contains(LoadBalancerName,'facilitator')].LoadBalancerArn" \
  --output text | sed 's|.*loadbalancer/||')
for m in HTTPCode_Target_5XX_Count HTTPCode_ELB_5XX_Count HTTPCode_Target_4XX_Count; do
  echo "--- $m"; aws cloudwatch get-metric-statistics --region us-east-2 \
    --namespace AWS/ApplicationELB --metric-name $m \
    --dimensions Name=LoadBalancer,Value="$DIM" \
    --start-time "$(date -u -d '12 hours ago' +%Y-%m-%dT%H:%M:%S)" \
    --end-time "$(date -u +%Y-%m-%dT%H:%M:%S)" --period 3600 --statistics Sum \
    --query 'sort_by(Datapoints,&Timestamp)[*].[Timestamp,Sum]' --output text
done

# 2) Our own response codes only (NOT upstream noise)
S=$(( ($(date +%s) - 21600) * 1000 ))
for c in 500 502 503 429 404; do
  echo -n "status=$c : "
  aws logs filter-log-events --log-group-name /ecs/facilitator-production \
    --region us-east-2 --start-time $S --filter-pattern "\"status=$c\"" \
    --query 'length(events)' --output text
done
```

**The AWS CLI paginates**: that loop prints one count PER PAGE, not a total. Sum the numbers
before quoting one - reading only the first or last is off by ~2x.

A handful of `503`s clustered around a deploy is ECS replacing tasks, not an
outage. If 4xx is climbing instead, suspect a rate limit: see
`docs/plans/bazaar/09-SESSION-LOG-2026-07-24.md` — limits on list endpoints must cover a full
`total/page_size` walk, and `tower_governor`'s `per_second(n)` means *one token
every n seconds*, not n per second.

### RPC timeouts
- Use premium RPC endpoints (QuickNode, Alchemy)
- Override the network's own `RPC_URL_<NETWORK>` - that is the only var the code reads, there is no fallback chain, and `QUICKNODE_BASE_RPC` is dead
- In production `RPC_URL_*` comes from the `facilitator-rpc-mainnet` / `facilitator-rpc-testnet` secrets, not from `.env`
- Check network connectivity to RPC URLs

### Missing branding after deployment
- Verify `static/index.html` is >200KB (~225KB today, not the small upstream version): compare `ls -la static/index.html` against `curl -s https://facilitator.ultravioletadao.xyz/ | wc -c`
- Verify `src/handlers.rs::get_root()` still uses `include_str!()` (`get_index()` is only an alias)
- Rebuild and redeploy: push to `main` (CI builds + deploys). For local iteration use `./scripts/fast-build.sh <version> --push`; for a hand-shipped release image use `./scripts/build-and-push.sh <version>`, which passes `--build-arg FACILITATOR_VERSION`.

### Payment verification failures
- Check facilitator wallet gas: `python scripts/check_config.py` - **partial**: it covers only Base/Avalanche/Polygon/Optimism and reads the *deprecated* `facilitator-evm-private-key` secret, not `-mainnet`, so it can validate the wrong wallet. Check the other chains' balances directly.
- Verify token contract addresses in `src/network.rs`
- Check EIP-3009 timestamp validity (must be in seconds, not milliseconds)

## Upstream Relationship

**Upstream**: https://github.com/x402-rs/x402-rs (golden source)
**Your Fork**: https://github.com/UltravioletaDAO/x402-rs
**Current fork base**: upstream v0.10.0 (merged 2025-11-26, commit 35af558d)
**Current version**: see `VERSION` at the repo root / `curl -s https://facilitator.ultravioletadao.xyz/version` - never hardcode it here
**Sync frequency**: aspirational quarterly; **the last actual upstream merge was 2025-11-26** - assume significant drift and diff before relying on any upstream behaviour. Security patches: within 1 week.

**Git Remotes:**
- `origin` - Your fork (UltravioletaDAO/x402-rs)
- `upstream` - Golden source (x402-rs/x402-rs)

**Before merging upstream changes**:
1. Backup `static/` directory
2. Review changes to `handlers.rs`, `network.rs`, `Dockerfile`
3. Follow merge strategy in `docs/CUSTOMIZATIONS.md`
4. Test branding: `curl http://localhost:8080/ | grep Ultravioleta`
5. Test custom networks: `curl http://localhost:8080/supported | jq`

**To sync with upstream:**
```bash
git fetch upstream
git log HEAD..upstream/main  # Review changes
git merge upstream/main      # Follow docs/CUSTOMIZATIONS.md strategy
```

## Security Notes

### CRITICAL: NEVER PUT SECRETS IN DOCUMENTATION

**This rule is absolute and has no exceptions:**

- **NEVER** put actual private keys, secret keys, seed phrases, or API keys in ANY documentation file (*.md, *.txt, *.rst)
- **NEVER** use real secrets as "examples" in documentation - always use obvious placeholders like `YOUR_SECRET_KEY_HERE` or `S<PLACEHOLDER>`
- **NEVER** copy-paste secrets from AWS Secrets Manager, wallets, or .env files into documentation
- **ALWAYS** assume documentation files will be committed to git and become public
- **IF YOU SEE A SECRET IN DOCS**: Treat it as a critical security incident - rotate the key immediately, use `git-filter-repo` to purge history, and force push

**Why this matters**: On December 2025, a Stellar mainnet private key was accidentally committed to `SECRETS_MANAGEMENT.md` as a "format example". This exposed production credentials in git history for 14 days. The key had to be rotated and history rewritten.

### Other Security Rules

- **NEVER** commit `.env` file with actual private keys
- **NEVER** commit `.unused/` directory - it's in `.gitignore` and CONTAINS SECRETS
- Use testnet keys for local development only
- Production keys stored in AWS Secrets Manager
- Rotate facilitator wallets regularly (see `docs/WALLET_ROTATION.md`)
- Facilitator wallet needs native tokens (ETH/AVAX) for gas, not payment tokens
- If you accidentally commit secrets, rotate them IMMEDIATELY and use `git-filter-repo` to clean history

## Low-Priority Networks — Do NOT Mention Unless Explicitly Asked

The following networks exist as enum entries in `src/network.rs` but are **NOT served by production `/supported`** and are NOT active priorities. Do not mention them unless the user explicitly asks:

- **Sei** (chain ID 1329) — enum-only, absent from prod `/supported` (still emits a row in `scripts/stablecoin_matrix.py` output — ignore it there)
- **XDC** (chain ID 50) — same

**XRPL (native XRP Ledger) IS active** — `xrpl` / `xrpl-testnet` / `xrpl:0` are live in `/supported`. There is no `xrpl-evm` chain in this codebase (`src/network.rs:108` says so explicitly) and chain id 1440002 appears nowhere in `src/`.

**BSC (Binance Smart Chain, chain ID 56) IS active** — it is implemented and should be included normally in conversations and recommendations.

## Common Pitfalls

1. **Forgetting to preserve branding during upgrades** - Always backup `static/` before pulling upstream
2. **EIP-3009 timestamp format** - Must use Unix seconds (not milliseconds)
3. **Network naming** - Use exact enum names from `src/network.rs` (e.g., "avalanche-fuji", not "fuji" or "avalanche-fuji:43113")
4. **RPC rate limits** - Free RPC endpoints may throttle; use premium for production
5. **Gas funds vs payment funds** - Facilitator wallet needs native tokens (ETH/AVAX/SOL) for gas, not payment tokens (USDC)
6. **NEVER use emojis in Rust code** - No emojis in log messages, comments, or string literals. Use plain text like `[OK]`, `[FAIL]`, `[WARN]` instead of ✓, ✗, ⚠. Emojis cause encoding issues in CloudWatch logs and terminal output.
7. **NEVER disable ENABLE_ESCROW** - `ENABLE_ESCROW=true` is set in `terraform/environments/production/main.tf:924` and must stay. It gates the x402r escrow/refund path only (`src/handlers.rs:2808`, reached when `paymentPayload.extensions.refund` is present); without it those requests get 400 `"Escrow settlement is disabled. Set ENABLE_ESCROW=true to enable."`. Plain `/settle` calls never touch the flag - so 'settlements are failing' is NOT evidence this flag moved.
8. **ALWAYS update `config/supported_tokens.json`** when adding a new network or stablecoin. This file is the JSON source of truth for all supported chains, tokens, and facilitator wallet addresses. **NEVER type wallet addresses from memory** — always copy them from `lambda/balances/handler.py` (the authoritative source). Previous AI-generated addresses were hallucinated and caused data integrity issues.
9. **Facilitator wallet addresses are FIXED** — do not invent or guess them. Full mainnet / testnet pairs live in `lambda/balances/handler.py`:
   - EVM: `0x103040545AC5031A11E8C03dd11324C7333a13C7` / `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`
   - Solana + Fogo: `F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq` / `6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv`
   - SUI: `0xe7bbf2b13f7d72714760aa16e024fa1b35a978793f9893d0568a4fbf356a764a` / `0xabbd16a2fab2a502c9cfe835195a6fc7d70bfc27cffb40b8b286b52a97006e67`
   - NEAR: `uvd-facilitator.near` / `uvd-facilitator.testnet`
   - Stellar: `GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB` / `GBBFZMLUJEZVI32EN4XA2KPP445XIBTMTRBLYWFIL556RDTHS2OWFQ2Z`
   - Algorand: `KIMS5H6QLCUDL65L5UBTOXDPWLMTS7N3AAC3I6B2NCONEI5QIVK7LH2C2I` / `5DPPDQNYUPCTXRZWRYSF3WPYU6RKAUR25F3YG4EKXQRHV5AUAI62H5GXL4`
   - XRPL (native, classic r-address — NOT the EVM sidechain): `rfADKkVXBNqK3z72tVSS3LVzAR3psYkonp` / `rGhTioKAFHe75KgVnQtacRiKFuPv28Wbwk`

## API Endpoints Reference

- `GET /` - Ultravioleta DAO landing page (HTML)
- `GET /health` - Health check: `{"status":"healthy"}`
- `GET /supported` - List supported networks/schemes (returns both v1 and v2 formats)
- `GET /verify` - Verification schema
- `POST /verify` - Verify payment authorization (accepts both v1 and v2 request formats)
- `GET /settle` - Settlement schema
- `POST /settle` - Settle payment on-chain (accepts both v1 and v2 request formats)
- `POST /accepts` - Negotiate payment requirements (Faremeter middleware compatibility, enriches with feePayer/tokens/escrow)
- `GET /version` - Release version reported by the running binary (JSON)
- `GET /blacklist` - Blocked payer addresses
- `POST /escrow/state` - Query escrow state for a payment
- `GET /bazaar` - Bazaar catalog page (HTML)
- `GET /events` - Live traffic stream (SSE), one message per verify/settle
- `GET /events/live` - HTML viewer for the stream
- `GET /stats` - Aggregated metrics page (HTML)
- `GET /api/stats` - Aggregated totals per network and asset (JSON)
- `GET /api/stats/history` - Settlement history reconstructed from the chain. NOT the same claim as `/api/stats` (which is what the facilitator measured); every row carries `source`
- `GET /transactions` - Recent recorded operations (JSON, `limit` capped at 200)
- DX402: `POST /dx402/anchor`, `GET /dx402/evidence/{paymentId}`, `GET /dx402/receipt/{paymentId}`, `GET /dx402/blob/{paymentId}`, `GET /dx402/stats`, `POST /dx402/recover` (501 in v0.1). Present only when `ENABLE_DX402=true`.
- `POST /mcp` - MCP server (Streamable HTTP, stateless). Four tools, and only four: `x402_supported`, `x402_accepts`, `x402_verify`, `x402_settle`. Each is dispatched THROUGH the REST router (`src/mcp.rs`, `ServiceExt::oneshot`), never by calling the handler functions -- `POST /settle` carries `settle_writer_gate` and calling `post_settle` directly would skip the writer lease. `/mcp` shares the `verify_settle_config` governor Arc, so it draws on the SAME per-IP bucket as `/verify` and `/settle` (measured: 15 MCP calls + 20 verify calls from one IP = 5 x 429). `GET /mcp` is a JSON 405; there is no SSE stream in stateless mode. `MCP_ALLOWED_HOSTS` overrides the Host allowlist, whose default already contains the production host -- rmcp's own default is loopback only and would 403 everything behind the ALB.
- `GET /.well-known/mcp/server-card.json` - MCP server card. `serverInfo.version` is NOT in the static file; it is stamped from `FACILITATOR_VERSION` when served.
- `GET /docs` - Interactive Swagger UI (OpenAPI documentation)
- `GET /api-docs/openapi.json` - Raw OpenAPI 3.0 JSON spec (version resolved at runtime from `VERSION`, see below)
- Discovery (Bazaar) API: `POST /discovery/register`, `GET /discovery/resources`, `GET /discovery/stats`, `GET /discovery/attestation/{hash}`; admin: `DELETE /discovery/resources`, `POST /discovery/admin/suppress`, `POST /discovery/admin/release`
- Asset endpoints: `/logo.png`, `/favicon.ico`, `/avalanche.png`, etc.

### DX402 durable-evidence (v1.75.0)

x402 settles payment permanently but delivers the resource **once** and keeps
nothing. DX402 closes that: the seller seals a copy of the response body to the
payer's own public key and anchors it; the buyer recovers it later with the same
wallet they paid with.

**The insight**: a payment authorization is a signature, and a signature yields
the signer's *public key*, not just their address. Paying is publishing your
encryption key — so there is no registration and no extra round trip. Four of the
seven network families need nothing but the address (Solana, Stellar, Algorand,
NEAR are ed25519); EVM and XRPL recover from the signature; Sui reads it out of
the signature envelope.

**Where each piece runs** — the facilitator is NOT in the response path (it only
sees `/verify` and `/settle`, never a body), so:
- `crates/x402-axum/src/durable.rs` — the seller post-hook. Holds the plaintext,
  encrypts, uploads. Hooked into the `settle_after_execution` branch of
  `layer.rs`, the one point where the delivered body and the settlement identity
  coexist.
- `src/dx402/` — the facilitator as **notary and index**. Signs EIP-712 receipts,
  records pointers. In `direct` mode it never holds plaintext or key material.
- `crates/x402-reqwest/src/durable.rs` — the buyer. Fetches, decrypts, and
  verifies `contentHash`.

**Rules that are load-bearing:**
- **DX402 can never fail a payment.** Every failure degrades to a `SkipReason`
  (`too_large`, `anchor_failed`, `no_payer_key`, `disabled`) carried in the
  `X-Durable-Evidence` header. Same discipline as `transaction_store`.
- **`contentHash` is over the PLAINTEXT.** Over the ciphertext it would only
  prove the blob was not corrupted; over the plaintext it proves the anchor
  decrypts to what was actually delivered.
- **`paymentId` is the AEAD associated data** — `keccak256(caip2Network || txHash)`.
  Derive it differently on either side and decryption fails with no obvious cause.
- **Anchoring is publishing.** `retention` defaults to `90d` on purpose;
  `permanent` is irrevocable.
- **Small-order ed25519 keys are rejected** (RFC 7748 §6.1, constant time).
  `ed25519-dalek` accepts non-canonical and small-order encodings in
  `VerifyingKey::from_bytes`; unchecked, that collapses the ECDH shared secret to
  a constant. Tested against libsodium's 7-value blacklist — **not** against
  invented vectors, which is how the fabricated SEAL v1 hashes passed CI for
  months.
- `find_known_eip712_metadata` in `chain/evm.rs` is now `pub` **so DX402 does not
  duplicate domain resolution**. A second copy would drift and silently recover a
  different, perfectly valid public key.

Config (all optional; **default OFF**): `ENABLE_DX402`, `DX402_STORE_BACKEND`
(only `s3` in v0.1), `DX402_STORE_BUCKET`, `DX402_STORE_PUBLIC_BASE`,
`DX402_REGISTRY_TABLE_NAME`, `DX402_SIGNING_KEY`, `DX402_RETENTION`. Missing
config **disables** the feature and logs why — it never falls back to an
in-memory store that would report evidence for data that dies with the process.

**Infra** lives in `terraform/environments/production/dx402.tf` (S3 bucket +
DynamoDB table + IAM). Provisioning and switching on are SEPARATE: those
resources are created regardless (~$0 idle), while `var.enable_dx402` only
controls the container's environment. The evidence bucket is **private** — a
pointer resolves through `GET /dx402/blob/{paymentId}` on the facilitator, never
through a public bucket, and pointers address the *payment* rather than the S3
key layout so old pointers survive a re-layout. The receipt-signing key
(`facilitator-dx402-signing-key`) is created by
`scripts/dx402-bootstrap-secret.sh` and signs attestations only — no funds, no
gas. Runbook: `docs/plans/dx402/03-DEPLOY-RUNBOOK.md`.

Spec: `docs/plans/dx402/02-SPEC-v0.1.md`. Guide: `docs/DX402.md`. Research and
prior-art survey: `docs/plans/dx402/00-RESEARCH.md`. Handoffs for KarmaCadabra,
execution.market, MeshRelay and describe.net: `docs/handoffs/2026-08-14-dx402-*.md`.

**Not yet proposed upstream** — the x402 Foundation requires a reviewed PR, and a
proposal without production usage gets discarded. Propose after real traffic.

### ERC-8004 endpoints

- `POST /register` - Register an agent. `Prefer: respond-async` returns 202 + `jobId` instead of holding the request open for the mint
- `GET /register/status/{jobId}` - Poll an async registration (`pending` → `mint_confirmed` → `done`/`failed`; terminal jobs age out after 1h)
- `POST /feedback` - Submit reputation feedback. **The facilitator is the AUTHOR on this path** — the registry records `msg.sender` (EVM) / account 0 (SVM), and that is us. Use the prepare/submit pairs below for real authorship.
- `POST /feedback/evm/prepare` + `POST /feedback/evm/submit` - EIP-7702 relayed rating: the rater delegates their EOA to Execution Market's `FeedbackDelegate`, signs a digest, and we send a type-4 tx **to the rater's address** so the registry sees the rater. Served only where a delegate is deployed AND verified on-chain — today **`base-sepolia` only** (`0x3A68085499B62286468A35b7D9Dfc237ef2d3768`); the table lives in `src/erc8004/relay.rs`. Mainnet awaits EM's deploy.
- `POST /feedback/solana/prepare` + `POST /feedback/solana/submit` - partially-signed SVM tx: the rater signs as `client`, we stay fee payer. `submit` refuses any transaction that is not byte-for-byte the one it built.
- `POST /feedback/revoke` - Revoke feedback. **ADMIN ONLY**: `Authorization: Bearer <ERC8004_ADMIN_TOKEN>`, and **404 when no token is configured** (fail-closed, so the route is indistinguishable from absent). Deliberately NOT `BAZAAR_ADMIN_TOKEN` — this one erases third-party reputation irreversibly. In production the token comes from the `facilitator-erc8004-admin-token` secret.
- `POST /feedback/response` - Append an agent response to feedback
- `GET /reputation/:network/:agentId` - Reputation summary (+ `atomStats` on Solana)
- `GET /identity/:network/:agentId` - Agent identity
- `GET /identity/:network/:agentId/metadata/:key` - One metadata entry
- `GET /identity/:network/owner/:address` - Resolve an agent by owner (EVM + SVM)
- `GET /identity/:network/total-supply` - Registered agents

The supported-network set is `supported_networks()` in `src/erc8004/mod.rs` (currently **20**: 11 mainnets + 9 testnets, Solana included) — read it there, never from a hardcoded count. `src/openapi.rs` still claims "18 networks (10 mainnets + 8 testnets)" in four places and is stale; fix it whenever the set changes.

**The owner lookup has no index behind it, and that shapes everything.** The
registries expose no `owner -> agentId` mapping, are NOT `ERC721Enumerable`
(`totalSupply()` **reverts on every deployed registry** -- verified on-chain
2026-09-01 on celo and base), and SKALE caps `eth_getLogs` at 2000 blocks. So
every cold lookup derives the registry's highest agent ID and scans `ownerOf`
through Multicall3. Two rules follow:

- **Never make a registry capability load-bearing without checking the chain.**
  Run `python scripts/erc8004_registry_capabilities.py` first. A revision that
  read the bound from `totalSupply()` shipped as a complete no-op -- the call
  always reverted, so its "fallback" was the only path that ever ran -- and held
  the facilitator's p99 at 11.4s for sixteen hours (`docs/handoffs/2026-09-01-p99-identity-owner-lookup.md`).
- **A non-zero `balanceOf` with no token found is a CONTRADICTION, not a miss.**
  It means the scan range was wrong. It answers 503, never 404, and `POST
  /register` refuses to mint on it -- `Ok(None)` there used to be read as
  permission to mint, handing a duplicate identity to someone who already had one.
- Base held **83,984** agents on 2026-09-01 against the 192,000 the scan can
  walk (`OWNER_SCAN_MAX_BATCHES`). Past that cap every owner lookup on that chain
  answers 503; it warns from 75%. The fix at that point is an owner index, not a
  bigger cap.

**`/identity/:network/owner/:address` answers 404 and 503 for different things
and callers must not collapse them.** 404 is "this address owns no agent"; 503 is
"the lookup reached no verdict", and carries `"retryable": true`. Persisting
"not registered" from a 503 is how a transient RPC failure becomes a permanent
wrong answer (INC-2026-07-21) — and on a registration path it mints a duplicate
agent for someone who already has one. The same rule governs a `/register`
timeout: it is not a failure, the mint may still land.

### Solana ERC-8004 (realigned 2026-08-07, v1.70.0–v1.72.0)

`src/erc8004/solana.rs` had been written against a pre-v0.3.0 revision of the
QuantuLabs program; seven separate defects came out of that, four of them only
visible by running against the chain. Full account: `docs/handoffs/2026-08-07-erc8004-solana-facilitator.md`.

Three things worth carrying into any future change here:

- **Account layouts are the golden source, and they are wide.** `AtomStats` is
  561 bytes with 45 fields; every one has to be declared to reach `trust_tier`
  and `confidence` at the tail. Tests pin the exact byte sizes — keep them.
- **The config PDA is two hops, not one.** `["root_config"]` holds the
  collection, and the collection seeds `["registry_config", collection]`. The
  legacy `["config"]` seed derives an address that was never initialized;
  `test_legacy_config_seed_is_not_used` fails if it returns.
- **SEAL v1 is keccak256 over the feedback content only** — no agent or client
  pubkey enters the hash. Vectors are pinned against the `8004-solana` npm SDK
  rather than against our own implementation, because three fabricated SHA-256
  variants passed CI for months by being compared only to themselves.

Two behaviours that surprise integrators:

- **Feedback without `score` is never scored.** The ATOM Engine records it on the
  agent and reports `had_impact=false`; reputation stays at zero however much
  accumulates, and it is not retroactive.
- **The program forbids self-feedback** (`SelfFeedbackNotAllowed`, 12300). An
  agent registered without `recipient` stays with the facilitator, which then
  cannot rate it. `POST /register` with `recipient` mints, initializes the ATOM
  stats, and transfers — in that order, because only the owner can initialize.

### The proof-of-payment gate (v1.74.0+)

`ProofOfPayment` used to be produced by the settle path and then dropped. It is
now verified server-side on every feedback: the transaction exists on that
network and succeeded, sits in the block the proof claims, contains an ERC-20
`Transfer` of exactly `amount` in `token` from `payer` to `payee`, the payer is
the new `rater` field, the payee is an address the Identity Registry ties to the
agent, the block timestamp is inside the freshness window, `paymentHash`
recomputes, and the (payment, agent) pair has not already been spent.

| Variable | Default | Notes |
|---|---|---|
| `ERC8004_REQUIRE_PROOF` | `false` | **Phase 1**: verify and report, do not reject. Flip to `true` only after the logs show real traffic passes |
| `ERC8004_PROOF_MAX_AGE_SECS` | `604800` | 7 days; also the TTL of the anti-replay record |
| `ERC8004_ALLOW_FACILITATOR_AUTHORSHIP` | `true` | Deprecated SVM path where WE are the author; set `false` to close it |
| `ERC8004_RELAY_DEADLINE_SECS` | `900` | How long a rater's 7702 relay authorisation stays valid |
| `ERC8004_ADMIN_TOKEN` | *(unset)* | Gate on `/feedback/revoke`. Unset → 404 |

Three things that are easy to get wrong:

- **Two verdicts never block a write, in either phase.** `proof_rpc_unavailable`
  is "no verdict" (our outage must not erase somebody's reputation) and
  `proof_unverifiable_chain` is the Solana path, whose payment half has no EVM
  receipt to read. Enforcing a check that never ran would silently disable
  Solana reputation.
- **`getAgentWallet` is zero for almost every real agent** (measured on Base:
  18896, 58517, 100, 1000, 5000, 40000 all read `0x0`), so `ownerOf` is
  load-bearing and both are accepted.
- **Execution Market's payments carry TWO `Transfer`s** — a fee and the net to
  the agent. The proof must declare the NET the payee actually receives, or the
  gate answers `proof_transfer_not_found`.

Full account: `docs/handoffs/2026-08-13-erc8004-autoria-reputacion-p0.md` and
`docs/handoffs/2026-08-14-eip7702-stipend-h2.md`.

### Traffic stream and metrics (v1.60.0+)

`/events` publishes one SSE message per operation; `/transactions` and
`/api/stats` read a DynamoDB index of the same operations.

**Neither is a ledger — the chain is.** The record is written fire-and-forget
*after* settlement resolves, so an unreachable store loses rows and never blocks
a payment. Say this out loud whenever a number from these endpoints is quoted.

Config (all optional, safe defaults):

| Variable | Default | Notes |
|---|---|---|
| `X402_EVENTS_ENABLED` | `true` | `false` → `/events` 404s |
| `X402_EVENTS_DETAIL` | `full` | `minimal` = only `{ts, kind, network, ok, error}` |
| `X402_EVENTS_SCOPE` | `all` | `allowlist` = only payers in `X402_EVENTS_ALLOWLIST` |
| `X402_EVENTS_MAX_SUBSCRIBERS` | `64` | at the cap `/events` returns 503 + `Retry-After` |
| `X402_EVENTS_PUBLISH_FAILURES` | `false` | publish operations that ERRORED |
| `TRANSACTIONS_TABLE_NAME` | *(unset)* | unset → nothing is recorded, payments unaffected |
| `TRANSACTIONS_TTL_DAYS` | `90` | `0` keeps records forever; aggregates never expire |

**`X402_EVENTS_PUBLISH_FAILURES` matters for how the numbers read.** While it is
`false`, an operation that errors produces neither an event nor a row, so a 100%
success rate means *"no failures were recorded"*, not *"no failures occurred"*.
When failures are published they carry a **bounded category** (`contract_revert`,
`invalid_signature`, …) and never the error text — raw errors carry addresses and
sometimes RPC URLs with keys in them.

**Adding a network?** It joins `/events` and the store for free. But
`upto` is advertised only where the Permit2 proxy is actually deployed — see
`UPTO_DEPLOYED_NETWORKS` in `src/upto/types.rs` and verify with `eth_getCode`
against **two** independent RPCs before adding an entry.

### x402 Protocol v2 Support (v1.8.0+)

The facilitator supports both x402 v1 and v2 protocol formats:

- **V1 networks**: `"network": "base"` (string enum — the serde name in `src/network.rs`; there is no `base-mainnet`)
- **V2 networks**: `"network": "eip155:8453"` (CAIP-2 format)

Both formats are auto-detected and processed identically. Existing v1 clients work unchanged.

Key files for v2 support:
- `src/caip2.rs` - CAIP-2 parsing and validation
- `src/types_v2.rs` - v2 protocol types and conversion traits

## Development Workflow

### Making Changes

1. Make code changes
2. Format: `just format-all`
3. Lint: `just clippy-all`
4. Test: `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1` — this is CI's green gate and a red one blocks the production deploy. `--test-threads=1` is not optional: parallel runs hang on CI runners.
5. Test locally: `cargo run --release` + integration tests
6. Build Docker: `./scripts/fast-build.sh <version>` (never a bare `docker build` on WSL2 — see the Docker section)
7. Test Docker locally: `docker-compose up`
8. Commit with clear messages

**When adding new API endpoints**, always add corresponding documentation in `src/openapi.rs`. The version needs no attention — it is patched at runtime from the `VERSION` file via `FACILITATOR_VERSION` (`src/version.rs`) — but endpoint definitions must be added manually. Verify after deploy: `curl -s https://facilitator.ultravioletadao.xyz/api-docs/openapi.json | jq '.info.version'`

### Adding a New Network

**Preferred method:** Use the `/add-network` skill for automated integration:
```
add facilitator scroll
```

The skill handles research, EIP-3009 verification, prerequisite checks, implementation, and deployment.

**Manual guide**: `guides/ADDING_NEW_CHAINS.md`

This complete checklist covers:
- Backend integration (Network enum, chain IDs, USDC contracts, RPC configuration)
- Frontend integration (logo, network cards, CSS styling, balance loading)
- AWS Secrets Manager configuration for premium RPCs
- Wallet funding requirements (mainnet and testnet separation)
- Docker build and deployment process
- Verification and troubleshooting steps

**Quick summary** (refer to guide for full details):
1. Add enum variant to `Network` in `src/network.rs`
2. Add USDC deployment constants and chain ID mappings
3. Add RPC environment variables in ALL live sites — `src/from_env.rs` (parsing), `.env.example` (local dev), `terraform/environments/production/main.tf` (ECS `environment`) or `secrets.tf` (if the URL carries an API key), and `lambda/balances/handler.py`. Declared only in `from_env.rs`, the container never receives it.
4. Update `src/chain/evm.rs` or `src/chain/solana.rs`
5. Add logo PNG file to `static/` directory
6. Add logo handler to `src/handlers.rs`
7. Update `static/index.html` with network cards and CSS styling
8. Configure AWS Secrets Manager with premium mainnet RPC
9. Fund both mainnet and testnet facilitator wallets with native tokens
10. Build Docker image, push to ECR, and deploy to ECS
11. Verify in `/supported` endpoint and test frontend
11b. **UPDATE `src/openapi.rs`** - it hardcodes network lists in prose (`:31`, `:34`, `:57`, `:523`, `:527`, `:959`, `:1018`). A new network is invisible in `/docs` until these are edited.
12. **UPDATE README.md** - Update the network count and add the new network to the tables
13. **VERIFY STABLECOIN MATRIX** - Run `python scripts/stablecoin_matrix.py` and update README stablecoin tables
14. **UPDATE `config/supported_tokens.json`** - Add the new network with chainId, tokens, explorer, and facilitatorWallet. Copy wallet address from `lambda/balances/handler.py` — NEVER type from memory.
15. **UPDATE `lambda/balances/handler.py`** - Add the new network to `get_network_configs()` with RPC and wallet address.

**CRITICAL**: Always update these files when adding a new network OR stablecoin:
- `config/supported_tokens.json` - JSON source of truth for all chains and wallets
- `lambda/balances/handler.py` - Landing page balance checker (wallet addresses)
- README.md - Network counts get stale quickly
- Stablecoin coverage matrix must be regenerated with `python scripts/stablecoin_matrix.py --md`
- Copy the markdown table output to README.md

**Total work**: ~500-700 changed lines across ~20-24 files + 1-2 logo PNGs + AWS config + wallet funding + README/CHANGELOG/openapi update. (Measured on the Robinhood Chain add, `git show --stat 7dbe194e`: 689 insertions across 24 files.)

> **OpenAPI Sync**: EVERY new network needs `src/openapi.rs` edited — it enumerates networks and the upto/escrow lists and counts, not just endpoints. The version resolves at runtime from `VERSION`; nothing to bump there.

### Updating Branding

1. Edit `static/index.html` (preserve structure)
2. Update logos in `static/` (flat PNG files, e.g. `static/base.png`; there is no `static/images/` directory)
3. Verify `src/handlers.rs::get_root()` still uses `include_str!()`
4. Rebuild: `cargo build --release`
5. Test: `curl http://localhost:8080/ | grep "New Branding"`

### Important Notes

- **NEVER compile or deploy the facilitator on your own initiative** - make the code change, tell the user, wait for them to build/deploy. But know what shipping means here: **pushing to `main` deploys to production.** `.github/workflows/ci.yaml` tests, builds the image, pushes it to ECR and `terraform apply -auto-approve`s it onto ECS, then waits for the rollout. The gate is armed today (the AWS repo secrets exist, so `preflight` emits `deploy=true`); only a red `test` job blocks it. A merge is a release; a `git push` is not a save. A failed deploy leaves `main` ahead of production - compare `curl -s https://facilitator.ultravioletadao.xyz/version` with `git log -1` before assuming your commit is live.
- **Never add emojis to Rust code** - they do NOT break compilation (Rust source is UTF-8), but they corrupt CloudWatch log output and terminal rendering. Use `[OK]`/`[FAIL]`/`[WARN]`. Same rule as Common Pitfalls #6.
- **Rust Edition**: edition 2021 (`Cargo.toml:5`). The real toolchain floor is **Rust >= 1.91** (highest `rust-version` among locked deps, e.g. aws-smithy-* 1.91.1), NOT 1.82. Edition 2024 needs only 1.85, so MSRV no longer blocks the migration - only syntax/lint churn does.
  - See v0.9.1 merge for details (commit 75b37e6)
- **Version Bumping**: the release version lives in the `VERSION` file at the repo
  root, **not** in `Cargo.toml`. Check the deployed version first, then bump
  `VERSION` from that, not from whatever is local:
  ```bash
  curl -s https://facilitator.ultravioletadao.xyz/version
  echo "<deployed version + 1 minor>" > VERSION   # e.g. 1.73.0 deployed -> 1.74.0
  ```
  `Cargo.toml` holds a frozen `0.0.0` placeholder and must stay untouched. That is
  what keeps the Docker dependency layer cached across releases: a release used to
  edit `Cargo.toml`, which the image build reads before compiling dependencies, so
  every deploy recompiled the whole dependency tree (~12 min → ~6 min measured).
  It also means `Cargo.lock` no longer needs hand-syncing, which had already broken
  a `--locked` build once.

  CI reads `VERSION`, fails the run if it is empty, tags the image
  `<version>-<short-sha>` (plus a moving `:latest`), and passes the version as the
  `FACILITATOR_VERSION` build arg; the binary resolves it at runtime in
  `src/version.rs`. Never declare that ARG in the builder stage of the Dockerfile —
  it would key every layer below it on the release version and undo the whole
  thing.


## Using Gemini CLI for Large Codebase Analysis

When a task involves many files or directories and might overflow your context window, prefer using the local Gemini CLI and then summarize its output. Use `gemini -m gemini-3-flash-preview -p` with the `@` path syntax to let Gemini read the files while Claude focuses on planning and editing.

### Model Selection

**Always use `gemini-3-flash-preview`** for codebase analysis tasks:
- Fast, efficient, and cost-effective
- 1M token context window (handles large codebases)
- Pro-level intelligence at Flash speed

Available Gemini 3 models:
| Model | Use Case |
|-------|----------|
| `gemini-3-flash-preview` | **DEFAULT** - Fast codebase analysis, code review |
| `gemini-3-pro-preview` | Complex reasoning, when Flash is insufficient |

### File and Directory Syntax

Paths are relative to the directory where you run the `gemini` command, and `@` tells Gemini CLI which files or folders to load into context.

### Examples

**Single file:**
```bash
gemini -m gemini-3-flash-preview -p "@src/main.py Describe what this file does and how it is structured."
```

**Multiple files:**
```bash
gemini -m gemini-3-flash-preview -p "@package.json @src/index.js Analyze the dependencies and how they are used in the codebase."
```

**One directory:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ Summarize the architecture, main modules, and data flow of this codebase."
```

**Several directories:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ @tests/ Explain how the test suite covers the source code and where the gaps are."
```

**Whole project tree:**
```bash
gemini -m gemini-3-flash-preview -p "@./ Give me a high-level overview of this project: tech stack, structure, and main responsibilities of each area."
```

**Using all tracked files:** there is no `--all_files` in gemini 0.22.x (`gemini --help`) - use `@./`:
```bash
gemini -m gemini-3-flash-preview -p "@./ Analyze the project layout, build system, and external dependencies."
```

### Implementation Checks

Use Gemini CLI to confirm whether specific features or patterns exist across the repo:

**Feature present?**
```bash
gemini -m gemini-3-flash-preview -p "@src/ @lib/ Is dark mode implemented? List the relevant files and functions."
```

**Authentication:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ @middleware/ How is authentication implemented (e.g. JWT/session)? List auth-related endpoints and middleware."
```

**WebSocket hooks:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ Do we have React hooks or utilities that manage WebSocket connections? Show them with file paths."
```

**Error handling:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ @api/ Is error handling consistent for API endpoints? Show representative try/catch or error-handling logic."
```

**Rate limiting:**
```bash
gemini -m gemini-3-flash-preview -p "@backend/ @middleware/ Is there any rate limiting in place for the API? Describe the implementation."
```

**Caching:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ @lib/ @services/ Is Redis (or any cache layer) used? List cache-related functions and how they are used."
```

**Security measures:**
```bash
gemini -m gemini-3-flash-preview -p "@src/ @api/ How are inputs sanitized to avoid SQL injection and similar attacks?"
```

**Tests for a feature:**
```bash
gemini -m gemini-3-flash-preview -p "@src/payment/ @tests/ How well is the payment module tested? List the main test cases."
```

### When Claude Should Call Gemini

Prefer calling `gemini -m gemini-3-flash-preview -p` via the Bash tool when:

- You need to reason about an entire codebase or large folders
- Comparing or scanning many big files at once
- Investigating project-wide patterns, architecture, or cross-cutting concerns
- Total relevant files are likely > 100 KB of text
- Verifying whether specific features, patterns, or security practices exist
- Searching for coding patterns across many files

### Important Notes

- Treat Gemini CLI output as an external report: read it, then answer in your own words
- `@` paths are always relative to the current working directory where gemini is executed
- The CLI injects file contents directly into Gemini's context, so Claude does not spend its own context window on those files
- For read-only analysis you do not need any destructive flags
- Be explicit in the `-p` prompt about what you want Gemini to look for; this produces more accurate results
- **Always include `-m gemini-3-flash-preview`** to ensure the correct model is used