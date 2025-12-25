# x402-rs

**Gasless multi-chain payment facilitator**

```
 _  _  _  _  ___  ____      ____  ____
( \/ )/ )( \(__ \(  _ \ ___(  _ \/ ___)
 )  ( ) __ ( / _/ )   /(___))   /\___ \
(_/\_)\_)(_/(____)(__\_)   (__\_)(____/
```

[![Live](https://img.shields.io/badge/live-facilitator.ultravioletadao.xyz-00d4aa)](https://facilitator.ultravioletadao.xyz)
[![Version](https://img.shields.io/badge/version-1.14.9-blue)](https://github.com/UltravioletaDAO/x402-rs)
[![Rust](https://img.shields.io/badge/rust-2021-orange)](https://www.rust-lang.org/)

---

## What is this?

A payment settlement service implementing the [HTTP 402](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/402) protocol. Users sign payment authorizations off-chain, the facilitator submits them on-chain and pays gas fees.

**No custody. No trust. Just payments.**

---

## Supported Networks

### Mainnets (12)

| Network | Chain ID | Token | Explorer |
|---------|----------|-------|----------|
| **Ethereum** | 1 | USDC | [etherscan.io](https://etherscan.io) |
| **Base** | 8453 | USDC | [basescan.org](https://basescan.org) |
| **Arbitrum** | 42161 | USDC | [arbiscan.io](https://arbiscan.io) |
| **Optimism** | 10 | USDC | [optimistic.etherscan.io](https://optimistic.etherscan.io) |
| **Polygon** | 137 | USDC | [polygonscan.com](https://polygonscan.com) |
| **Avalanche** | 43114 | USDC | [snowtrace.io](https://snowtrace.io) |
| **Celo** | 42220 | cUSD | [celoscan.io](https://celoscan.io) |
| **Solana** | - | USDC | [solscan.io](https://solscan.io) |
| **Fogo** | - | USDC | [fogoscan.com](https://fogoscan.com) |
| **NEAR** | - | USDC | [nearblocks.io](https://nearblocks.io) |
| **HyperEVM** | 999 | USDC | [hyperliquid.xyz](https://hyperliquid.xyz) |
| **Unichain** | 130 | USDC | [uniscan.xyz](https://uniscan.xyz) |
| **Monad** | 10143 | MON | [monad.xyz](https://monad.xyz) |

### Testnets (8)

| Network | Chain ID | Faucet |
|---------|----------|--------|
| Base Sepolia | 84532 | [faucet.circle.com](https://faucet.circle.com) |
| Optimism Sepolia | 11155420 | [faucet.circle.com](https://faucet.circle.com) |
| Polygon Amoy | 80002 | [faucet.polygon.technology](https://faucet.polygon.technology) |
| Avalanche Fuji | 43113 | [faucet.avax.network](https://faucet.avax.network) |
| Celo Sepolia | 44787 | [faucet.celo.org](https://faucet.celo.org) |
| Solana Devnet | - | [solfaucet.com](https://solfaucet.com) |
| Fogo Testnet | - | [fogoscan.com](https://fogoscan.com/?cluster=testnet) |
| NEAR Testnet | - | [near-faucet.io](https://near-faucet.io) |
| HyperEVM Testnet | 333 | - |

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
cargo run --release --features solana,near

# Test
curl http://localhost:8080/health
curl http://localhost:8080/supported | jq '.kinds | length'
# => 20
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

### How It Works

**Standard Flow** (no escrow):
1. Buyer signs EIP-3009 authorization to merchant address
2. Facilitator calls `transferWithAuthorization()` on USDC
3. Merchant receives funds immediately

**Escrow Flow** (with `refund` extension):
1. Merchant registers with Escrow contract (one-time)
2. Factory deploys deterministic DepositRelay proxy for merchant
3. Buyer signs EIP-3009 authorization to **proxy address** (not merchant)
4. Facilitator detects `refund` extension and routes to escrow
5. Facilitator verifies proxy address via CREATE3 computation
6. Facilitator calls `executeDeposit()` on proxy
7. Proxy forwards tokens to Escrow with deposit record
8. Funds held until release (merchant) or refund (buyer/arbiter)

### Supported Networks

| Network | Chain ID | Factory | Escrow | Status |
|---------|----------|---------|--------|--------|
| Base | 8453 | `0x41Cc...A814` | `0xC409...f6bC` | Production |
| Base Sepolia | 84532 | `0xf981...BaC2` | `0xF7F2...0E58` | Testnet |

### Configuration

Enable escrow settlement:

```bash
export ENABLE_ESCROW=true
```

### Request Format

```json
{
  "x402Version": 2,
  "paymentPayload": {
    "x402Version": 2,
    "resource": {"url": "https://api.example.com/premium", "mimeType": "application/json"},
    "accepted": {
      "scheme": "exact",
      "network": "eip155:8453",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "amount": "10000",
      "payTo": "0xPROXY_ADDRESS"
    },
    "payload": {
      "authorization": {"from": "0xBUYER", "to": "0xPROXY_ADDRESS", "value": "10000", ...},
      "signature": "0x..."
    },
    "extensions": {
      "refund": {
        "info": {
          "factoryAddress": "0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814",
          "merchantPayouts": {
            "0xPROXY_ADDRESS": "0xMERCHANT_PAYOUT_ADDRESS"
          }
        }
      }
    }
  }
}
```

### Contract Addresses

| Contract | Base Mainnet | Base Sepolia |
|----------|--------------|--------------|
| CreateX | `0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed` | Same |
| Factory | `0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814` | `0xf981D813842eE78d18ef8ac825eef8e2C8A8BaC2` |
| Escrow | `0xC409e6da89E54253fbA86C1CE3E553d24E03f6bC` | `0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058` |
| Implementation | `0x55eEC2951Da58118ebf32fD925A9bBB13096e828` | `0x740785D15a77caCeE72De645f1bAeed880E2E99B` |

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

# RPC URLs (premium recommended for production)
RPC_URL_BASE=https://mainnet.base.org
RPC_URL_NEAR_MAINNET=https://rpc.mainnet.near.org
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
1. User signs EIP-3009 authorization (EVM) or NEP-366 delegate action (NEAR)
2. User sends signed payload to facilitator
3. Facilitator verifies signature and submits on-chain
4. Facilitator pays gas, user pays nothing

---

## Deployment

### AWS ECS (Production)

```bash
# Build & push
docker build -t facilitator:v1.14.1 .
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.14.1

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
cargo clippy --features solana,near

# Test
cd tests/integration && python test_facilitator.py
```

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
