# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## File Organization

This repository maintains a strict file organization structure. The root directory contains ONLY:

**Essential files in root:**
- `README.md` - Project overview
- `CLAUDE.md` - This file (Claude Code instructions)
- Build configuration: `Cargo.toml`, `Dockerfile`, `docker-compose.yml`, `.env.example`, `.gitignore`, etc.

**All other files organized in directories:**
- `docs/` - ALL documentation (CHANGELOG.md, CUSTOMIZATIONS.md, DEPLOYMENT.md, etc.)
- `static/` - Landing page HTML, images, assets
- `scripts/` - Python scripts, shell scripts, deployment tools
- `config/` - Configuration files (blacklist.json, prometheus.yml, etc.)
- `terraform/` - Infrastructure as code, task definitions
- `src/` - Rust source code
- `crates/` - Workspace crates
- `tests/` - Test suites
- `.unused/` - **IGNORED** (contains secrets, never commit!)

## Project Overview

This is the **x402-rs Payment Facilitator** - a standalone Rust-based service enabling gasless micropayments across 14+ blockchain networks using the HTTP 402 Payment Required protocol. The facilitator acts as a settlement intermediary, verifying EIP-3009 payment authorizations and submitting them on-chain.

**Key characteristics**:
- Production-ready service deployed on AWS ECS
- Supports 7 mainnets + 7 testnets (EVM: Base, Avalanche, Celo, HyperEVM, Polygon, Optimism; Non-EVM: Solana)
- Custom Ultravioleta DAO branding (landing page, logos)
- Forked from upstream [x402-rs/x402-rs](https://github.com/x402-rs/x402-rs)

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
python test_usdc_payment.py --network base-sepolia

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

### Docker
```bash
# Build and run with Docker Compose
docker-compose up -d

# View logs
docker-compose logs -f facilitator

# Build and push to ECR
./scripts/build-and-push.sh v1.0.0
```

### Diagnostics
```bash
# Check configuration
python scripts/check_config.py

# Diagnose payment issues
python scripts/diagnose_payment.py --network base-mainnet

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

**src/network.rs**: Network definitions (14+ networks)
- `Network` enum with chain IDs and display names
- `NetworkFamily` (Evm vs Solana) for dual-chain support
- Static USDC/token deployment addresses per network
- **CRITICAL**: Contains custom networks added by Ultravioleta DAO (HyperEVM, Polygon, Optimism, Celo)

**src/handlers.rs**: HTTP request handlers
- `get_index()` - **Custom handler** serving Ultravioleta DAO branded landing page via `include_str!("../static/index.html")`
- Asset handlers for logos (favicon, network logos)
- `/verify` - Verify payment authorization structure
- `/settle` - Submit payment on-chain
- `/supported` - List available networks/schemes
- `/health` - Health check endpoint

**src/facilitator.rs**: Core payment logic trait
- `Facilitator` trait defining verification and settlement interface
- Network-agnostic abstraction over payment operations

**src/facilitator_local.rs**: Local facilitator implementation
- `FacilitatorLocal` implements `Facilitator` trait
- Delegates to chain-specific implementations (EVM vs Solana)
- Manages provider cache for RPC connections

**src/chain/**: Chain-specific payment logic
- `chain/evm.rs` - EIP-3009 payment verification and settlement for EVM chains
- `chain/solana.rs` - Solana token transfer authorization support
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

### Workspace Structure

This is a Cargo workspace with multiple crates:

**Root crate (x402-rs)**: Main facilitator service
**crates/x402-axum**: Axum middleware for x402 protocol (library)
**crates/x402-reqwest**: Reqwest client for x402 payments (library)
**examples/x402-axum-example**: Example server using x402-axum
**examples/x402-reqwest-example**: Example client using x402-reqwest

## Critical Customizations

**⚠️ THESE FILES ARE PROTECTED - DO NOT OVERWRITE FROM UPSTREAM:**

1. **static/index.html** (57KB) - Ultravioleta DAO branded landing page
   - Replaces upstream's simple "Hello" message
   - Contains network grid, API documentation, DAO branding
   - **Recovery**: `git checkout HEAD~1 -- static/index.html`

2. **src/handlers.rs** - `get_index()` function
   - Uses `include_str!("../static/index.html")` instead of plain text
   - Embeds HTML at compile time for performance
   - **Must preserve this pattern when merging upstream changes**

3. **static/images/** - Network logos (9 PNG files)
   - avalanche.png, base.png, celo.png, hyperevm.png, optimism.png, polygon.png, solana.png, celo-colombia.png, logo.png (DAO logo)
   - Never overwrite from upstream

4. **src/network.rs** - Custom networks added beyond upstream
   - HyperEVM mainnet/testnet (Chain IDs: 999, 333)
   - Polygon mainnet/Amoy testnet (Chain IDs: 137, 80002)
   - Optimism mainnet/Sepolia testnet (Chain IDs: 10, 11155420)
   - Celo mainnet/Sepolia testnet (Chain IDs: 42220, 44787)
   - Solana mainnet/devnet
   - **Merge strategy**: Preserve ALL custom networks when pulling upstream

5. **Rust Edition** - Using edition 2021 for compatibility
   - Currently on Rust edition 2021 (compatible with Rust 1.82+)
   - Upstream uses edition 2024 (requires Rust 1.86+)
   - Downgraded in v0.9.1 merge for broader compatibility

See `docs/CUSTOMIZATIONS.md` for complete documentation of all customizations and merge strategies.

## Configuration

### Environment Variables

Copy `.env.example` to `.env` and configure:

**Required** (Separate wallets per environment - RECOMMENDED):
- `EVM_PRIVATE_KEY_MAINNET` - Facilitator wallet for mainnet EVM chains (leave empty for AWS Secrets Manager)
- `EVM_PRIVATE_KEY_TESTNET` - Facilitator wallet for testnet EVM chains (leave empty for AWS Secrets Manager)
- `SOLANA_PRIVATE_KEY_MAINNET` - Facilitator wallet for Solana mainnet (leave empty for AWS Secrets Manager)
- `SOLANA_PRIVATE_KEY_TESTNET` - Facilitator wallet for Solana devnet (leave empty for AWS Secrets Manager)

**Backward Compatibility** (DEPRECATED):
- `EVM_PRIVATE_KEY` - Generic wallet for ALL EVM chains (only used if network-specific keys are not set)
- `SOLANA_PRIVATE_KEY` - Generic wallet for ALL Solana networks (only used if network-specific keys are not set)

**RPC URLs** (defaults provided, override for premium endpoints):
- `RPC_URL_BASE_MAINNET`, `RPC_URL_BASE_SEPOLIA`
- `RPC_URL_AVALANCHE_MAINNET`, `RPC_URL_AVALANCHE_FUJI`
- `RPC_URL_CELO_MAINNET`, `RPC_URL_CELO_ALFAJORES`
- `RPC_URL_HYPEREVM_MAINNET`, `RPC_URL_HYPEREVM_TESTNET`
- `RPC_URL_POLYGON_MAINNET`, `RPC_URL_POLYGON_AMOY`
- `RPC_URL_OPTIMISM_MAINNET`, `RPC_URL_OPTIMISM_SEPOLIA`
- `RPC_URL_SOLANA_MAINNET`, `RPC_URL_SOLANA_DEVNET`
- Additional: SEI, XDC networks

**Optional**:
- `QUICKNODE_BASE_RPC` - Premium RPC for higher rate limits
- `OTEL_EXPORTER_OTLP_ENDPOINT` - OpenTelemetry endpoint for observability
- `RUST_LOG` - Logging level (default: info)
- `PORT`, `HOST` - Server binding (default: 8080, 0.0.0.0)

### AWS Secrets Manager (Production)

Leave wallet environment variables empty in `.env`. The facilitator will fetch them from AWS Secrets Manager if running on ECS with appropriate IAM permissions.

**IMPORTANT**: As of v1.3.0, the facilitator uses separate wallets for mainnet and testnet environments. This prevents the critical bug where testnet transactions were signed with mainnet keys.

Secret names (configured in infrastructure):
- `facilitator-evm-private-key-mainnet` - Mainnet EVM wallet
- `facilitator-evm-private-key-testnet` - Testnet EVM wallet
- `facilitator-solana-keypair-mainnet` - Solana mainnet wallet
- `facilitator-solana-keypair-testnet` - Solana devnet wallet
- `facilitator-rpc-mainnet` - Contains all mainnet RPC URLs (Base, Avalanche, Polygon, Optimism, HyperEVM, Solana, Ethereum, Arbitrum, Celo)
- `facilitator-rpc-testnet` - Contains all testnet RPC URLs (Solana Devnet, Arbitrum Sepolia)

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

# Build and push Docker image
./scripts/build-and-push.sh v1.0.0

# Deploy infrastructure
cd terraform/environments/production
terraform init
terraform plan -out=facilitator-prod.tfplan
terraform apply facilitator-prod.tfplan

# Update running service
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

**Production URL**: `https://facilitator.ultravioletadao.xyz` (currently)
**Target URL**: `https://facilitator.ultravioletadao.xyz` (after old stack destroyed)

**Cost estimate**: ~$43-48/month (Fargate 1vCPU/2GB, ALB, NAT instance)

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

Located in `tests/load/`:
- `k6_load_test.js` - k6 load test (100+ TPS)
- Run: `k6 run --vus 100 --duration 5m k6_load_test.js`

### Protocol Testing

Located in `tests/x402/`:
- Python-based x402 protocol tests
- Verify/settle payload validation
- See `tests/x402/README.md` and `tests/x402/TROUBLESHOOTING.md`

## Important Documentation

- **guides/ADDING_NEW_CHAINS.md** - Complete checklist and guide for adding new blockchain networks
- **docs/CUSTOMIZATIONS.md** - Detailed inventory of all customizations vs upstream
- **docs/CHANGELOG.md** - Version history and release notes
- **docs/DEPLOYMENT.md** - Deployment procedures and infrastructure guide
- **docs/TESTING.md** - Complete testing guide
- **docs/WALLET_ROTATION.md** - Security procedures for rotating facilitator keys
- **docs/UPSTREAM_MERGE_STRATEGY.md** - How to safely merge upstream changes without losing branding
- **docs/EXTRACTION_MASTER_PLAN.md** - History of extracting facilitator from karmacadabra monorepo
- **docs/EIP3009_TIMESTAMP_BEST_PRACTICES.md** - Timestamp handling for payment authorizations

## Troubleshooting

### "Invalid signature" errors
```bash
python scripts/diagnose_payment.py --network base-mainnet
python scripts/compare_domain_separator.py
```

### RPC timeouts
- Use premium RPC endpoints (QuickNode, Alchemy)
- Set `QUICKNODE_BASE_RPC` in `.env`
- Check network connectivity to RPC URLs

### Missing branding after deployment
- Verify `static/index.html` is 57KB (not small upstream version)
- Verify `src/handlers.rs::get_index()` uses `include_str!()`
- Rebuild Docker image: `./scripts/build-and-push.sh`

### Payment verification failures
- Check facilitator wallet has gas funds: `python scripts/check_config.py`
- Verify token contract addresses in `src/network.rs`
- Check EIP-3009 timestamp validity (must be in seconds, not milliseconds)

## Upstream Relationship

**Upstream**: https://github.com/x402-rs/x402-rs (golden source)
**Your Fork**: https://github.com/UltravioletaDAO/x402-rs
**Current fork base**: v0.9.1 (merged 2025-11-06)
**Current version**: v1.2.0
**Sync frequency**: Quarterly review for features, within 1 week for security patches

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

- **NEVER** commit `.env` file with actual private keys
- **NEVER** commit `.unused/` directory - it's in `.gitignore` and CONTAINS SECRETS
- Use testnet keys for local development only
- Production keys stored in AWS Secrets Manager
- Rotate facilitator wallets regularly (see `docs/WALLET_ROTATION.md`)
- Facilitator wallet needs native tokens (ETH/AVAX) for gas, not payment tokens
- If you accidentally commit secrets, rotate them IMMEDIATELY and use `git-filter-repo` to clean history

## Common Pitfalls

1. **Forgetting to preserve branding during upgrades** - Always backup `static/` before pulling upstream
2. **EIP-3009 timestamp format** - Must use Unix seconds (not milliseconds)
3. **Network naming** - Use exact enum names from `src/network.rs` (e.g., "avalanche-fuji", not "fuji" or "avalanche-fuji:43113")
4. **RPC rate limits** - Free RPC endpoints may throttle; use premium for production
5. **Gas funds vs payment funds** - Facilitator wallet needs native tokens (ETH/AVAX/SOL) for gas, not payment tokens (USDC)

## API Endpoints Reference

- `GET /` - Ultravioleta DAO landing page (HTML)
- `GET /health` - Health check: `{"status":"healthy"}`
- `GET /supported` - List supported networks/schemes
- `GET /verify` - Verification schema
- `POST /verify` - Verify payment authorization (does not settle)
- `GET /settle` - Settlement schema
- `POST /settle` - Settle payment on-chain (requires valid EIP-3009 authorization)
- Asset endpoints: `/logo.png`, `/favicon.ico`, `/avalanche.png`, etc.

## Development Workflow

### Making Changes

1. Make code changes
2. Format: `just format-all`
3. Lint: `just clippy-all`
4. Test locally: `cargo run --release` + integration tests
5. Build Docker: `docker build -t facilitator-test .`
6. Test Docker locally: `docker-compose up`
7. Commit with clear messages

### Adding a New Network

**See the comprehensive guide**: `guides/ADDING_NEW_CHAINS.md`

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
3. Add RPC environment variables to `src/from_env.rs`
4. Update `src/chain/evm.rs` or `src/chain/solana.rs`
5. Add logo PNG file to `static/` directory
6. Add logo handler to `src/handlers.rs`
7. Update `static/index.html` with network cards and CSS styling
8. Configure AWS Secrets Manager with premium mainnet RPC
9. Fund both mainnet and testnet facilitator wallets with native tokens
10. Build Docker image, push to ECR, and deploy to ECS
11. Verify in `/supported` endpoint and test frontend

**Total work**: ~155 lines of code + 1 logo file + AWS config + wallet funding

### Updating Branding

1. Edit `static/index.html` (preserve structure)
2. Update logos in `static/images/` (PNG format)
3. Verify `src/handlers.rs::get_index()` still uses `include_str!()`
4. Rebuild: `cargo build --release`
5. Test: `curl http://localhost:8080/ | grep "New Branding"`

### Important Notes

- **Never add emojis to Rust code** - it will break compilation
- **Rust Edition**: Currently using **edition 2021** for compatibility with Rust 1.82
  - Upstream uses edition 2024 (requires Rust 1.86+)
  - Can upgrade to edition 2024 when ready to require Rust 1.86+
  - See v0.9.1 merge for details (commit 75b37e6)