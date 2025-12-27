# x402-rs

**Gasless multi-chain payment facilitator**

```
 _  _  _  _  ___  ____      ____  ____
( \/ )/ )( \(__ \(  _ \ ___(  _ \/ ___)
 )  ( ) __ ( / _/ )   /(___))   /\___ \
(_/\_)\_)(_/(____)(__\_)   (__\_)(____/
```

[![Live](https://img.shields.io/badge/live-facilitator.ultravioletadao.xyz-00d4aa)](https://facilitator.ultravioletadao.xyz)
[![Version](https://img.shields.io/badge/version-1.15.9-blue)](https://github.com/UltravioletaDAO/x402-rs)
[![Rust](https://img.shields.io/badge/rust-2021-orange)](https://www.rust-lang.org/)

---

## What is this?

A payment settlement service implementing the [HTTP 402](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/402) protocol. Users sign payment authorizations off-chain, the facilitator submits them on-chain and pays gas fees.

**No custody. No trust. Just payments.**

---

## Supported Networks

### Mainnets (15)

| Network | Chain ID | Token | Explorer |
|---------|----------|-------|----------|
| **Ethereum** | 1 | USDC | [etherscan.io](https://etherscan.io) |
| **Base** | 8453 | USDC | [basescan.org](https://basescan.org) |
| **Arbitrum** | 42161 | USDC | [arbiscan.io](https://arbiscan.io) |
| **Optimism** | 10 | USDC | [optimistic.etherscan.io](https://optimistic.etherscan.io) |
| **Polygon** | 137 | USDC | [polygonscan.com](https://polygonscan.com) |
| **Avalanche** | 43114 | USDC | [snowtrace.io](https://snowtrace.io) |
| **Celo** | 42220 | cUSD | [celoscan.io](https://celoscan.io) |
| **HyperEVM** | 999 | USDC | [hyperliquid.xyz](https://hyperliquid.xyz) |
| **Unichain** | 130 | USDC | [uniscan.xyz](https://uniscan.xyz) |
| **Monad** | 10143 | MON | [monad.xyz](https://monad.xyz) |
| **Solana** | - | USDC, AUSD | [solscan.io](https://solscan.io) |
| **Fogo** | - | USDC | [fogoscan.com](https://fogoscan.com) |
| **NEAR** | - | USDC | [nearblocks.io](https://nearblocks.io) |
| **Stellar** | - | USDC | [stellarchain.io](https://stellarchain.io) |
| **Algorand** | - | USDC | [allo.info](https://allo.info) |

### Testnets (15)

| Network | Chain ID | Faucet |
|---------|----------|--------|
| Ethereum Sepolia | 11155111 | [faucet.circle.com](https://faucet.circle.com) |
| Base Sepolia | 84532 | [faucet.circle.com](https://faucet.circle.com) |
| Arbitrum Sepolia | 421614 | [faucet.circle.com](https://faucet.circle.com) |
| Optimism Sepolia | 11155420 | [faucet.circle.com](https://faucet.circle.com) |
| Polygon Amoy | 80002 | [faucet.polygon.technology](https://faucet.polygon.technology) |
| Avalanche Fuji | 43113 | [faucet.avax.network](https://faucet.avax.network) |
| Celo Alfajores | 44787 | [faucet.celo.org](https://faucet.celo.org) |
| HyperEVM Testnet | 333 | - |
| Unichain Sepolia | 1301 | - |
| Solana Devnet | - | [solfaucet.com](https://solfaucet.com) |
| Fogo Testnet | - | [fogoscan.com](https://fogoscan.com/?cluster=testnet) |
| NEAR Testnet | - | [near-faucet.io](https://near-faucet.io) |
| Stellar Testnet | - | [friendbot](https://friendbot.stellar.org) |
| Algorand Testnet | - | [dispenser.testnet.aws.algodev.network](https://dispenser.testnet.aws.algodev.network) |
| Monad Testnet | 10143 | [monad.xyz](https://monad.xyz) |

### Supported Stablecoins

| Token | Networks |
|-------|----------|
| **USDC** | All EVM, Solana, NEAR, Stellar, Algorand |
| **EURC** | Ethereum, Base, Avalanche |
| **AUSD** | Solana (Token2022) |
| **PYUSD** | Ethereum, Solana |
| **USDT** | Ethereum, Polygon |
| **cUSD** | Celo |

---

## Quick Start

```bash
# Clone
git clone https://github.com/UltravioletaDAO/x402-rs.git
cd x402-rs

# Configure
cp .env.example .env
# Add your private keys (use testnet keys for development)

# Run
cargo run --release --features solana,near,stellar,algorand

# Test
curl http://localhost:8080/health
curl http://localhost:8080/supported | jq '.kinds | length'
# => 60 (30 networks x2 for v1 and v2 formats)
```

### Docker

```bash
docker-compose up -d
curl http://localhost:8080/
```

---

## API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Landing page |
| `/health` | GET | Health check |
| `/version` | GET | Current version |
| `/supported` | GET | List all networks |
| `/verify` | POST | Verify payment authorization |
| `/settle` | POST | Submit payment on-chain (supports escrow with `refund` extension) |
| `/blacklist` | GET | OFAC sanctioned addresses |
| `/discovery/resources` | GET | List registered paid APIs |
| `/discovery/register` | POST | Register a paid endpoint |

### Example: Check supported networks

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network'
```

### Example: Settle a payment

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Content-Type: application/json" \
  -d '{"payload": "...", "network": "base"}'
```

---

## Chain-Specific Features

### EVM Chains (EIP-3009)
Standard `transferWithAuthorization` for gasless USDC transfers.

### Solana (SPL Token + Token2022)
Supports both SPL Token (USDC) and Token2022 (AUSD) programs.

### NEAR (NEP-366)
Meta-transactions with delegate actions for gasless payments.

### Stellar (Soroban)
Smart contract-based authorization on Stellar's Soroban VM.

### Algorand (Atomic Groups)
Fee pooling via atomic transaction groups. Facilitator signs transaction 0 (fee tx), user signs transaction 1 (payment tx). Based on [GoPlausible x402-avm spec](https://github.com/GoPlausible/x402-avm).

---

## x402r Escrow Extension (Trustless Refunds)

The facilitator supports the [x402r extension](https://github.com/coinbase/x402/issues/864) for trustless refunds via escrow contracts.

### Architecture Overview

```
                           STANDARD PAYMENT FLOW
  ┌──────────┐    ┌───────────────┐    ┌────────────┐    ┌──────────┐
  │  Buyer   │───>│  Facilitator  │───>│   USDC     │───>│ Merchant │
  │ (signs)  │    │  (pays gas)   │    │ (on-chain) │    │(receives)│
  └──────────┘    └───────────────┘    └────────────┘    └──────────┘
       │                 │
       │  EIP-3009       │  transferWithAuthorization()
       │  signature      │

                           ESCROW PAYMENT FLOW (x402r)
  ┌──────────┐    ┌───────────────┐    ┌──────────────┐    ┌──────────┐
  │  Buyer   │───>│  Facilitator  │───>│ DepositRelay │───>│  Escrow  │
  │ (signs)  │    │  (pays gas)   │    │   (proxy)    │    │ (holds)  │
  └──────────┘    └───────────────┘    └──────────────┘    └──────────┘
       │                 │                    │                  │
       │  EIP-3009       │  executeDeposit()  │  deposit()       │
       │  to PROXY       │                    │                  │
       │                 │                    │                  ▼
       │                 │                    │            ┌──────────┐
       │                 │                    │            │ Merchant │
       │                 │                    └───────────>│(after    │
       │                 │                      release()  │ window)  │
       │                 │                                 └──────────┘
       │                 │
       └─────────────────┴───── Buyer can request refund within window
```

### Supported Networks

| Network | Chain ID | Factory | Escrow | Status |
|---------|----------|---------|--------|--------|
| Base | 8453 | `0x41Cc...A814` | `0xC409...f6bC` | Production |
| Base Sepolia | 84532 | `0xf981...BaC2` | `0xF7F2...0E58` | Testnet |

### Documentation

- **Technical Deep-Dive:** [`docs/X402R_ESCROW.md`](docs/X402R_ESCROW.md)
- **Testing Guide:** [`docs/X402R_ESCROW_TESTING.md`](docs/X402R_ESCROW_TESTING.md)
- **x402r Proposal:** https://github.com/coinbase/x402/issues/864
- **x402r Contracts:** https://github.com/BackTrackCo/x402r-contracts

---

## Configuration

```bash
# Wallet keys (leave empty for AWS Secrets Manager)
EVM_PRIVATE_KEY_MAINNET=
EVM_PRIVATE_KEY_TESTNET=
SOLANA_PRIVATE_KEY_MAINNET=
SOLANA_PRIVATE_KEY_TESTNET=
NEAR_PRIVATE_KEY_MAINNET=
NEAR_ACCOUNT_ID_MAINNET=
STELLAR_SECRET_KEY_MAINNET=
STELLAR_SECRET_KEY_TESTNET=
ALGORAND_MNEMONIC_MAINNET=
ALGORAND_MNEMONIC_TESTNET=

# RPC URLs (premium recommended for production)
RPC_URL_BASE=https://mainnet.base.org
RPC_URL_NEAR_MAINNET=https://rpc.mainnet.near.org
RPC_URL_ALGORAND_MAINNET=https://mainnet-api.algonode.cloud
# ... see .env.example for all networks
```

---

## Architecture

```
┌─────────────┐     ┌─────────────────┐     ┌──────────────┐
│   Client    │────▶│   Facilitator   │────▶│  Blockchain  │
│  (signs)    │     │  (pays gas)     │     │  (settles)   │
└─────────────┘     └─────────────────┘     └──────────────┘
```

**Payment Flow:**
1. User signs EIP-3009 (EVM), NEP-366 (NEAR), or atomic group (Algorand)
2. User sends signed payload to facilitator
3. Facilitator verifies signature and submits on-chain
4. Facilitator pays gas, user pays nothing

---

## Deployment

### AWS ECS (Production)

```bash
# Build & push
docker build -t facilitator:v1.15.9 .
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.15.9

# Deploy
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment
```

**Infrastructure:** Terraform configs in `terraform/environments/production/`

**Cost:** ~$45/month (Fargate 1vCPU/2GB + ALB)

---

## Development

```bash
# Format
cargo fmt

# Lint
cargo clippy --features solana,near,stellar,algorand

# Test
cd tests/integration && python test_facilitator.py
```

---

## Acknowledgments

Special thanks to:
- **[GoPlausible](https://github.com/GoPlausible)** - For the [x402-avm specification](https://github.com/GoPlausible/x402-avm) and documentation that made Algorand integration possible
- **[x402-rs](https://github.com/x402-rs/x402-rs)** - The upstream project this facilitator is forked from

---

## Links

- **Live:** https://facilitator.ultravioletadao.xyz
- **Upstream:** https://github.com/x402-rs/x402-rs
- **x402 Protocol:** https://www.x402.org

---

## License

Apache 2.0

---

**Built by [Ultravioleta DAO](https://ultravioletadao.xyz)**
