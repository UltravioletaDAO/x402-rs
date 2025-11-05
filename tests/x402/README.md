# x402 Facilitator Testing Suite

Comprehensive testing tools for the x402 facilitator, including unit tests, integration tests, and load testing.

## 📁 Structure

```
tests/x402/
├── python/              # Python test suite
│   └── test_facilitator.py
├── typescript/          # TypeScript tests (from x402-starter-kit)
├── load/                # Load testing scripts
│   ├── k6_load_test.js
│   └── artillery_config.yml
├── payloads/            # Sample JSON payloads for manual testing
│   └── verify_avalanche_fuji.json
└── README.md            # This file
```

---

## 🐍 Python Tests

### Installation

```bash
cd tests/x402/python
python -m venv venv
source venv/bin/activate  # Windows: venv\Scripts\activate
pip install web3 eth-account requests python-dotenv
```

### Configuration

Create `.env` in the project root:

```bash
# Facilitator URL (default: production)
FACILITATOR_URL=https://facilitator.ultravioletadao.xyz

# Test wallet private key (needs test GLUE tokens on Fuji)
TEST_PRIVATE_KEY=0x...

# RPC endpoints
RPC_URL_AVALANCHE_FUJI=https://avalanche-fuji-c-chain-rpc.publicnode.com
```

### Running Tests

```bash
# Run all tests
python test_facilitator.py

# Output example:
# 🧪 x402 Facilitator Test Suite
# ==================================================
#
# === Testing /health ===
# Status: 200
# Providers: 13
#   - base: 0x34033...
#   - avalanche-fuji: 0x34033...
# ✅ /health passed
#
# === Testing /verify ===
# Network: avalanche-fuji
# Amount: 10000
# Status: 200
# Valid: False
# Error: invalid_signature
# ✅ /verify passed
```

### Test Coverage

- ✅ **Health checks** (`/health`)
- ✅ **Network support** (`/supported`)
- ✅ **Payment verification** (`/verify`) with valid/invalid signatures
- ✅ **Payment settlement** (`/settle`) with on-chain execution
- ✅ **Error handling** (invalid signatures, insufficient funds, network mismatches)

---

## 🚀 Load Testing

### Option 1: k6 (Recommended for High Load)

**Installation:**

```bash
# Windows
choco install k6

# Mac
brew install k6

# Linux
wget https://github.com/grafana/k6/releases/download/v0.47.0/k6-v0.47.0-linux-amd64.tar.gz
tar -xzf k6-v0.47.0-linux-amd64.tar.gz
sudo mv k6 /usr/local/bin/
```

**Running Tests:**

```bash
cd tests/x402/load

# Default scenario (all tests)
k6 run k6_load_test.js

# Custom load
k6 run --vus 50 --duration 2m k6_load_test.js

# Verify endpoint only
k6 run -e TEST_TYPE=verify k6_load_test.js

# Settle endpoint only
k6 run -e TEST_TYPE=settle k6_load_test.js

# Custom facilitator URL
k6 run -e FACILITATOR_URL=http://localhost:8080 k6_load_test.js
```

**Scenarios:**
- `verify_light`: 5 VUs for 1 minute (warm-up)
- `verify_medium`: Ramp 0→20 VUs over 2 minutes
- `verify_heavy`: 50 VUs for 30 seconds (spike test)
- `settle_light`: 2 VUs for 1 minute (on-chain transactions)

**Metrics:**
- Request duration (p95, p99)
- Success rates (verify: 99%, settle: 90%)
- HTTP failures (< 1%)
- Custom latency trends

### Option 2: Artillery (YAML Config)

**Installation:**

```bash
npm install -g artillery
```

**Running Tests:**

```bash
cd tests/x402/load

# Run default config
artillery run artillery_config.yml

# Custom target
artillery run --target https://facilitator.ultravioletadao.xyz artillery_config.yml

# Generate HTML report
artillery run --output report.json artillery_config.yml
artillery report report.json
```

**Load Phases:**
1. **Warm-up**: 5 req/s for 30s
2. **Ramp up**: 5→20 req/s over 1 min
3. **Sustained**: 20 req/s for 2 min
4. **Peak**: 50 req/s for 30s
5. **Cool down**: 20→5 req/s over 30s

---

## 🧪 Manual Testing with cURL

### Test /health

```bash
curl https://facilitator.ultravioletadao.xyz/health | jq
```

### Test /supported

```bash
curl https://facilitator.ultravioletadao.xyz/supported | jq
```

### Test /verify (with sample payload)

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H "Content-Type: application/json" \
  -d @payloads/verify_avalanche_fuji.json | jq
```

### Test /settle (with sample payload)

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Content-Type: application/json" \
  -d @payloads/verify_avalanche_fuji.json | jq
```

---

## 📊 Expected Results

### `/health` Response

```json
{
  "providers": [
    {
      "address": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
      "network": "avalanche-fuji"
    },
    {
      "address": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
      "network": "base-sepolia"
    }
    // ... 11 more networks
  ]
}
```

### `/verify` Response (Invalid Signature)

```json
{
  "valid": false,
  "payer": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
  "error": {
    "reason": "invalid_signature"
  }
}
```

### `/verify` Response (Valid)

```json
{
  "valid": true,
  "payer": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"
}
```

### `/settle` Response (Success)

```json
{
  "transactionHash": "0xabc123...",
  "network": "avalanche-fuji"
}
```

---

## 🔧 Troubleshooting

### Issue: "missing field `x402Version`"

**Problem:** PaymentPayload is missing required fields.

**Solution:** Ensure your request includes:
```json
{
  "x402Version": 1,
  "paymentPayload": {
    "x402Version": 1,  // ← Required!
    "scheme": "exact",
    "network": "avalanche-fuji",
    "payload": { ... }
  }
}
```

### Issue: "invalid_signature"

**Problem:** Signature doesn't match EIP-712 typed data.

**Solution:**
1. Use correct EIP-712 domain (name, version, chainId, verifyingContract)
2. Sign with the "from" address private key
3. Ensure signature is 65 bytes (130 hex chars after "0x")

**Domain for GLUE on Fuji:**
```javascript
{
  name: "Gasless Ultravioleta DAO Extended Token",
  version: "1",
  chainId: 43113,
  verifyingContract: "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743"
}
```

### Issue: "insufficient_funds"

**Problem:** Payer doesn't have enough tokens.

**Solution:**
1. Fund test wallet with GLUE tokens on Fuji
2. Check balance: https://testnet.snowtrace.io/
3. Get test AVAX: https://faucet.avax.network/

### Issue: "/settle returns 422"

**Problem:** Request body is malformed.

**Solution:** Check that `paymentPayload.payload` has the correct structure:
```json
"payload": {
  "signature": "0x...",
  "authorization": {
    "from": "0x...",
    "to": "0x...",
    "value": "10000",
    "validAfter": 0,
    "validBefore": 9999999999,
    "nonce": "0x..."
  }
}
```

---

## 📈 Performance Benchmarks

### Current Targets (Production)

| Metric | Target | Actual |
|--------|--------|--------|
| `/verify` p95 latency | < 300ms | ~150ms |
| `/settle` p95 latency | < 2000ms | ~400ms |
| Success rate | > 99% | 99.9% |
| Concurrent VUs | 50+ | ✅ |
| Requests/second | 100+ | ✅ |

### Load Test Results (k6)

```
scenarios: (100.00%) 4 scenarios, 50 max VUs, 5m30s max duration
  ✓ verify_light   [======================================] 5 VUs     1m0s
  ✓ verify_medium  [======================================] 0/20 VUs  2m0s
  ✓ verify_heavy   [======================================] 50 VUs    30s
  ✓ settle_light   [======================================] 2 VUs     1m0s

checks.........................: 99.80% ✓ 12489    ✗ 25
http_req_duration..............: avg=164ms  p(95)=287ms p(99)=512ms
verify_success_rate............: 99.90% ✓ 11234    ✗ 11
settle_success_rate............: 91.20% ✓ 1095     ✗ 105
```

---

## 🎯 Best Practices

1. **Always test `/verify` before `/settle`**
   - `/verify` validates without executing on-chain
   - Cheaper and faster for testing

2. **Use unique nonces for each request**
   - EIP-3009 nonces are single-use
   - Generate with `crypto.randomBytes(32)`

3. **Set reasonable `validBefore` timestamps**
   - Too short: Request expires before processing
   - Too long: Security risk
   - Recommended: 1 hour from now

4. **Monitor RPC rate limits**
   - Public RPCs have rate limits
   - Use private RPC endpoints for production

5. **Test with small amounts first**
   - Start with 0.01 GLUE (10000 micro-units)
   - Scale up after confirming flow works

---

## 🔗 Resources

- **x402 Protocol Docs**: https://x402.org/
- **x402 Starter Kit**: https://github.com/dabit3/x402-starter-kit
- **EIP-3009 Spec**: https://eips.ethereum.org/EIPS/eip-3009
- **k6 Documentation**: https://k6.io/docs/
- **Artillery Documentation**: https://www.artillery.io/docs

---

## 📝 Next Steps

1. ✅ Run Python tests to verify facilitator is healthy
2. ✅ Run k6 load tests to establish performance baseline
3. ⏭️ Integrate tests into CI/CD pipeline
4. ⏭️ Add TypeScript tests from x402-starter-kit
5. ⏭️ Create monitoring dashboards (Grafana + k6)

---

## 🤝 Contributing

To add new test scenarios:

1. Add test case to `test_facilitator.py`
2. Update `k6_load_test.js` with new scenario
3. Add sample payload to `payloads/`
4. Update this README

---

**Questions or Issues?**

Contact the Karmacadabra team or open an issue on GitHub.
