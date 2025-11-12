# x402-rs Payment Facilitator

> Multi-chain payment facilitator supporting gasless micropayments via HTTP 402 protocol

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/docker-ready-blue.svg)](https://www.docker.com/)

## Overview

The x402-rs facilitator is a production-ready payment settlement service that enables gasless micropayments across 17 blockchain networks using the HTTP 402 Payment Required protocol. Built with Rust for high performance and reliability.

### Key Features

- 🌐 **17 Blockchain Networks** (7 mainnets + 10 testnets)
- ⚡ **Gasless Payments** via EIP-3009 transferWithAuthorization
- 🔒 **Trustless** - No custody, users sign payment authorizations off-chain
- 🚀 **High Performance** - Rust + Axum, handles 100+ transactions/second
- 📊 **Production Ready** - Battle-tested on AWS ECS, custom Ultravioleta DAO branding
- 🔧 **Self-Contained** - Standalone deployment, no external dependencies

### Supported Networks

| Network | Mainnet | Testnet | Token |
|---------|---------|---------|-------|
| **Base** | ✅ | ✅ Base Sepolia | USDC |
| **Avalanche** | ✅ | ✅ Fuji | USDC |
| **Celo** | ✅ | ✅ Alfajores | cUSD |
| **HyperEVM** | ✅ | ✅ Testnet | USDC |
| **Polygon** | ✅ | ✅ Amoy | USDC |
| **Solana** | ✅ | ✅ Devnet | USDC |
| **Optimism** | ✅ | ✅ Sepolia | USDC |
| **Sei** | ✅ | ✅ Testnet | - |
| **XDC** | ✅ | - | - |

**Total**: 7 mainnets, 10 testnets

---

## Quick Start

### Prerequisites

- **Rust** 1.75+ (stable or nightly)
- **Docker** 20.10+ (optional)
- **curl** (for testing)

### Local Development

```bash
# Clone repository
git clone <repository-url>
cd facilitator

# Configure environment
cp .env.example .env
# Edit .env with your private keys (testnet recommended)

# Build and run
cargo build --release
cargo run --release

# Test
curl http://localhost:8080/health
# Expected: {"status":"healthy"}
```

### Docker Deployment

```bash
# Start facilitator
docker-compose up -d

# View logs
docker-compose logs -f facilitator

# Test
curl http://localhost:8080/
# Expected: Ultravioleta DAO landing page
```

---

## Configuration

### Environment Variables

See `.env.example` for full configuration. Key variables:

```bash
# Blockchain Keys - RECOMMENDED: Separate keys per environment
# (Leave empty for AWS Secrets Manager in production)
EVM_PRIVATE_KEY_MAINNET=
EVM_PRIVATE_KEY_TESTNET=
SOLANA_PRIVATE_KEY_MAINNET=
SOLANA_PRIVATE_KEY_TESTNET=

# Legacy keys (deprecated - only used if network-specific keys not set)
EVM_PRIVATE_KEY=
SOLANA_PRIVATE_KEY=

# RPC URLs (defaults provided)
RPC_URL_BASE_MAINNET=https://mainnet.base.org
RPC_URL_AVALANCHE_FUJI=https://api.avax-test.network/ext/bc/C/rpc
# ... (17 networks total, see .env.example)

# Optional
RUST_LOG=info
OTEL_EXPORTER_OTLP_ENDPOINT=
```

### Security

- ⚠️ **NEVER commit .env file**
- ✅ Use AWS Secrets Manager in production
- ✅ Rotate keys regularly (see `docs/WALLET_ROTATION.md`)

### Address Blocklist

The facilitator supports blocking specific wallet addresses from processing payments. This is useful for preventing spam or malicious actors from using the service.

**Configuration**: Create a `config/blocklist.json` file:

```json
[
  {
    "account_type": "solana",
    "wallet": "41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az",
    "reason": "spam"
  },
  {
    "account_type": "evm",
    "wallet": "0x0000000000000000000000000000000000000000",
    "reason": "test blocked address"
  }
]
```

**Behavior**:
- Blocked addresses are rejected during the `/verify` endpoint with a `BlockedAddress` error
- The `/settle` endpoint will complete but log a warning if called directly
- Address matching is case-insensitive
- The blocklist is loaded at startup (graceful fallback to empty list if file missing)
- Changes to `config/blocklist.json` require a facilitator restart

**Fields**:
- `account_type`: Either `"evm"` or `"solana"`
- `wallet`: The wallet address to block (case-insensitive)
- `reason`: Human-readable explanation for why the address is blocked

---

## API Endpoints

### Health Check
```bash
GET /health
Response: {"status":"healthy"}
```

### Networks List
```bash
GET /networks
Response: {"networks":[...]}  # 17 networks
```

### Settle Payment
```bash
POST /settle
Body: {EIP-3009 authorization}
Response: {"success":true,"tx_hash":"0x..."}
```

---

## Testing

```bash
# Integration tests
cd tests/integration
python test_usdc_payment.py --network base-sepolia

# Load test
cd tests/load
k6 run --vus 100 --duration 5m k6_load_test.js
```

---

## Deployment

### AWS ECS (Production)

See `docs/DEPLOYMENT.md` for detailed instructions.

```bash
cd terraform/environments/production
terraform init
terraform apply

# Build and push
scripts/build-and-push.sh v1.0.0

# Update service
aws ecs update-service --cluster facilitator-prod --service facilitator-prod --force-new-deployment
```

**Cost**: ~$41-51/month (optimized)

---

## Customization

### Custom Branding

**⚠️ CRITICAL**: Protected files (NEVER overwrite):
- `static/index.html` (57KB Ultravioleta DAO landing page)
- `static/*.png` (logos)
- `src/handlers.rs` (custom get_root handler)

See `docs/UPSTREAM_MERGE_STRATEGY.md` for safe upgrade procedures.

---

## Documentation

- **[TESTING.md](docs/TESTING.md)** - Testing guide
- **[DEPLOYMENT.md](docs/DEPLOYMENT.md)** - AWS deployment (to be created)
- **[WALLET_ROTATION.md](docs/WALLET_ROTATION.md)** - Security procedures
- **[UPSTREAM_MERGE_STRATEGY.md](docs/UPSTREAM_MERGE_STRATEGY.md)** - Branding protection
- **[EXTRACTION_MASTER_PLAN.md](docs/EXTRACTION_MASTER_PLAN.md)** - Repository history

---

## Troubleshooting

### "Invalid signature"
```bash
python scripts/diagnose_payment.py --network base-mainnet
python scripts/compare_domain_separator.py
```

### "RPC timeout"
- Use premium RPC (QuickNode, Alchemy)
- Set `QUICKNODE_BASE_RPC` in `.env`

See `tests/x402/TROUBLESHOOTING.md` for more.

---

## License

**Apache License 2.0** - see [LICENSE](LICENSE)

**Upstream**: [polyphene/x402-rs](https://github.com/polyphene/x402-rs)

---

## Credits

**Developed by**: [Ultravioleta DAO](https://ultravioletadao.xyz)

**Original x402 Protocol**: [polyphene/x402-rs](https://github.com/polyphene/x402-rs)

---

## Changelog

**v1.0.0** (2025-11-01)
- Initial standalone release
- Extracted from karmacadabra monorepo
- 17 networks supported
- Custom Ultravioleta DAO branding
- Production-ready AWS deployment

---

**Made with ❤️ by Ultravioleta DAO**
