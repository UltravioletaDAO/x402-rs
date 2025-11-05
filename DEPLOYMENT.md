# x402 Facilitator Deployment Guide
**Karmacadabra - Ultravioleta DAO**

Complete step-by-step guide for deploying the x402 payment facilitator to Cherry Servers.

---

## =� Table of Contents

1. [Prerequisites](#prerequisites)
2. [Phase 1: Initial Setup](#phase-1-initial-setup)
3. [Phase 2: Token Deployment](#phase-2-token-deployment)
4. [Phase 3: Facilitator Deployment](#phase-3-facilitator-deployment)
5. [Phase 4: HTTPS Configuration](#phase-4-https-configuration)
6. [Phase 5: Observability Setup](#phase-5-observability-setup)
7. [Phase 6: Testing](#phase-6-testing)
8. [Phase 7: Production Hardening](#phase-7-production-hardening)
9. [Troubleshooting](#troubleshooting)
10. [Maintenance](#maintenance)

---

## Prerequisites

### Required Software

```bash
# On Cherry Servers (Ubuntu 22.04+)
sudo apt update
sudo apt install -y \
    docker.io \
    docker-compose \
    caddy \
    curl \
    jq \
    git
```

### Required Accounts & Services

-  Cherry Servers account with VPS
-  Domain: `facilitator.ultravioletadao.xyz` (DNS configured)
-  Avalanche Fuji RPC endpoint (custom + public fallback)
-  Grafana instance at `grafana.ultravioletadao.xyz`

### Required Knowledge

- Basic Linux/SSH
- Docker & Docker Compose
- Ethereum/Avalanche basics
- Hot wallet management

---

## Phase 1: Initial Setup

### 1.1 Clone Repository

```bash
# SSH into Cherry Servers VPS
ssh user@your-server-ip

# Clone Karmacadabra repository
git clone https://github.com/ultravioletadao/karmacadabra.git
cd karmacadabra/x402-rs
```

### 1.2 Generate Hot Wallet

```bash
# Option 1: Using cast (foundry)
cast wallet new

# Option 2: Using OpenSSL
openssl rand -hex 32

# Save output:
# - Address: 0x...
# - Private Key: 0x...
```

**CRITICAL: Store private key securely!**

### 1.3 Fund Hot Wallet

```bash
# Get testnet AVAX from faucet
# https://faucet.avax.network/

# Minimum: 2 AVAX
# Recommended: 5 AVAX

# Verify balance
cast balance 0xYourWalletAddress \
  --rpc-url https://avalanche-fuji-c-chain-rpc.publicnode.com
```

### 1.4 Initialize Environment

```bash
# Run initialization script
./deploy-facilitator.sh init

# This creates:
# - .env file (from .env.example)
# - logs/ directory
# - data/ directory
```

---

## Phase 2: Token Deployment

### 2.1 Deploy GLUE Token

```bash
# Navigate to erc-20 folder
cd ../erc-20

# Deploy UVD token to Fuji
./deploy-fuji.sh

# Output:
#  GLUE deployed to: 0xABC...
#  Saved to deployment.json
```

### 2.2 Update Token Address

```bash
# Return to x402-rs folder
cd ../x402-rs

# Edit .env file
nano .env

# Update this line:
UVD_TOKEN_ADDRESS=0xYourDeployedUVDAddress
```

### 2.3 Verify Token Deployment

```bash
# Check UVD token details
cast call 0xYourDeployedUVDAddress \
  "name()(string)" \
  --rpc-url https://avalanche-fuji-c-chain-rpc.publicnode.com

# Should return: "Gasless Ultravioleta DAO Extended Token"

cast call 0xYourDeployedUVDAddress \
  "decimals()(uint8)" \
  --rpc-url https://avalanche-fuji-c-chain-rpc.publicnode.com

# Should return: 6
```

---

## Phase 3: Facilitator Deployment

### 3.1 Configure Environment Variables

Edit `.env` file with your actual values:

```bash
nano .env
```

**Critical variables to update:**

```bash
# Hot wallet private key
EVM_PRIVATE_KEY=0xYourActualPrivateKey

# UVD token address (from Phase 2)
UVD_TOKEN_ADDRESS=0xYourDeployedUVDAddress

# Custom RPC endpoint
RPC_URL_AVALANCHE_FUJI=https://your-custom-rpc.xyz/avalanche-fuji

# Grafana endpoint
OTEL_EXPORTER_OTLP_ENDPOINT=http://grafana.ultravioletadao.xyz:4317
```

### 3.2 Build Docker Image

```bash
# Build x402 facilitator image
./deploy-facilitator.sh build

# Expected output:
#  Docker image built: ultravioletadao/x402-facilitator:latest
```

### 3.3 Deploy with Docker Compose

```bash
# Deploy all services (facilitator + Caddy + Prometheus + Grafana)
./deploy-facilitator.sh deploy

# Check status
./deploy-facilitator.sh status

# View logs
./deploy-facilitator.sh logs
```

### 3.4 Verify Local Deployment

```bash
# Test health endpoint
curl http://localhost:8080/health

# Expected: {"status":"healthy"}

# Test supported methods
curl http://localhost:8080/supported | jq

# Expected:
# [
#   "evm-eip3009-USDC-fuji",
#   "evm-eip3009-UVD-fuji",
#   "evm-eip3009-WAVAX-fuji"
# ]
```

---

## Phase 4: HTTPS Configuration

### 4.1 Configure DNS

Ensure DNS A record points to your server:

```bash
facilitator.ultravioletadao.xyz � your-server-ip
```

Verify:

```bash
dig facilitator.ultravioletadao.xyz +short
```

### 4.2 Install & Configure Caddy

```bash
# Install Caddy (if not already installed)
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install caddy

# Copy Caddyfile
sudo cp Caddyfile /etc/caddy/Caddyfile

# Validate config
sudo caddy validate --config /etc/caddy/Caddyfile

# Reload Caddy
sudo systemctl reload caddy
```

### 4.3 Test HTTPS

```bash
# Test HTTPS endpoint
curl -I https://facilitator.ultravioletadao.xyz/health

# Expected: HTTP/2 200 (with SSL certificate)

# Test supported methods
curl https://facilitator.ultravioletadao.xyz/supported | jq
```

---

## Phase 5: Observability Setup

### 5.1 Verify Prometheus Metrics

```bash
# Check metrics endpoint
curl http://localhost:8080/metrics

# Expected: Prometheus-format metrics
# x402_payments_total
# x402_balance_avax
# etc.
```

### 5.2 Configure Grafana Dashboard

1. Open Grafana: `http://grafana.ultravioletadao.xyz:3000`
2. Login (admin/changeme)
3. Add Prometheus data source:
   - URL: `http://prometheus:9090`
4. Import dashboard:
   - Use `grafana/dashboards/x402-facilitator.json` (if exists)
   - Or create custom dashboard

### 5.3 Setup Alerts

Create alert rules in Prometheus (`prometheus.yml`):

```yaml
# Example: Low AVAX balance alert
- alert: FacilitatorBalanceLow
  expr: x402_balance_avax < 1.0
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "Facilitator AVAX balance low"
    description: "Balance: {{ $value }} AVAX"
```

---

## Phase 6: Testing

### 6.1 Integration Tests

```bash
# Run test script
./deploy-facilitator.sh test

# Test verify endpoint
curl -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H "Content-Type: application/json" \
  -d '{
    "from": "0x...",
    "to": "0x...",
    "value": "10000",
    "validAfter": "0",
    "validBefore": "9999999999",
    "nonce": "0xabc...",
    "v": 27,
    "r": "0x...",
    "s": "0x..."
  }'
```

### 6.2 End-to-End Test with Agents

```bash
# Test Karma-Hello agent payment flow
cd ../karma-hello-agent
python scripts/test_payment.py \
  --facilitator https://facilitator.ultravioletadao.xyz \
  --amount 0.01

# Expected:
#  Payment verified
#  Transaction settled: 0x...
#  Balance updated
```

### 6.3 Load Testing

```bash
# Simple load test (60 requests in 1 minute)
for i in {1..60}; do
  curl -s https://facilitator.ultravioletadao.xyz/health &
  sleep 1
done
wait

# Check metrics after load test
curl http://localhost:8080/metrics | grep x402_http_requests_total
```

---

## Phase 7: Production Hardening

### 7.1 Enable Rate Limiting

Edit `.env`:

```bash
RATE_LIMIT_ENABLED=true
RATE_LIMIT_PER_MINUTE=60
RATE_LIMIT_PER_HOUR=1000
```

Restart:

```bash
./deploy-facilitator.sh restart
```

### 7.2 Setup Key Rotation

Create standby wallet:

```bash
# Generate second hot wallet
cast wallet new

# Fund with 2-5 AVAX
# Add to .env as STANDBY_WALLET_ADDRESS
```

Schedule monthly rotation:

```bash
# Add to crontab
0 0 1 * * /path/to/rotate-wallet.sh
```

### 7.3 Configure Monitoring Alerts

Setup Discord/PagerDuty webhooks in `.env`:

```bash
WEBHOOKS_ENABLED=true
WEBHOOK_URL=https://discord.com/api/webhooks/...
```

### 7.4 Backup Strategy

```bash
# Create daily backups
./deploy-facilitator.sh backup

# Add to crontab
0 2 * * * cd /path/to/x402-rs && ./deploy-facilitator.sh backup
```

---

## Troubleshooting

### Facilitator Won't Start

```bash
# Check logs
./deploy-facilitator.sh logs

# Common issues:
# - Invalid private key � Check EVM_PRIVATE_KEY in .env
# - RPC connection failed � Test RPC_URL manually
# - Port 8080 in use � Check with: lsof -i :8080
```

### Payment Verification Fails

```bash
# Check RPC connectivity
curl $RPC_URL_AVALANCHE_FUJI

# Verify token address
cast call $UVD_TOKEN_ADDRESS "symbol()(string)" --rpc-url $RPC_URL

# Check facilitator balance
cast balance $FACILITATOR_ADDRESS --rpc-url $RPC_URL
```

### HTTPS Not Working

```bash
# Check Caddy status
sudo systemctl status caddy

# View Caddy logs
sudo journalctl -u caddy -f

# Test certificate
curl -vI https://facilitator.ultravioletadao.xyz/health
```

### Low Performance

```bash
# Check resource usage
docker stats

# Increase resources in docker-compose.yml:
resources:
  limits:
    cpus: '4.0'
    memory: 4G
```

---

## Maintenance

### Daily Tasks

```bash
# Check facilitator status
./deploy-facilitator.sh status

# Check AVAX balance
cast balance $FACILITATOR_ADDRESS --rpc-url $RPC_URL

# Review metrics in Grafana
```

### Weekly Tasks

```bash
# Review logs for errors
./deploy-facilitator.sh logs | grep ERROR

# Check disk space
df -h

# Backup configuration
./deploy-facilitator.sh backup
```

### Monthly Tasks

```bash
# Update Docker images
./deploy-facilitator.sh update

# Rotate hot wallet (if needed)
# ./rotate-wallet.sh

# Review and optimize resource allocation
docker stats
```

### Emergency Procedures

**Facilitator Crashed:**

```bash
./deploy-facilitator.sh restart
./deploy-facilitator.sh logs
```

**Hot Wallet Compromised:**

```bash
# Immediately switch to standby wallet
# 1. Stop facilitator
./deploy-facilitator.sh stop

# 2. Update .env with standby key
nano .env  # Change EVM_PRIVATE_KEY

# 3. Restart
./deploy-facilitator.sh start

# 4. Generate new standby key
cast wallet new
```

**RPC Endpoint Down:**

```bash
# Facilitator auto-switches to fallback RPC
# Check logs:
./deploy-facilitator.sh logs | grep "RPC"

# Update primary RPC when available
nano .env  # Change RPC_URL_AVALANCHE_FUJI
./deploy-facilitator.sh restart
```

---

## Support

**Documentation:**
- README.md - Project overview
- DEPLOYMENT.md - This guide
- .env.example - Configuration reference

**Monitoring:**
- Facilitator: https://facilitator.ultravioletadao.xyz/health
- Metrics: https://facilitator.ultravioletadao.xyz/metrics
- Grafana: http://grafana.ultravioletadao.xyz:3000

**Commands:**

```bash
./deploy-facilitator.sh help  # Show all commands
```

---

**Deployment Status**: ✅ Deployed to production at facilitator.ultravioletadao.xyz

**Current Configuration**: Using native USDC tokens on 13 networks (Base, Avalanche, Polygon, Optimism, Celo, HyperEVM, Solana + testnets)
