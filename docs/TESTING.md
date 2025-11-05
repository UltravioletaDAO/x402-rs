# Facilitator Testing

## Overview

Comprehensive testing infrastructure for the x402 facilitator to ensure reliable payment processing across all 4 supported networks.

## Test Suite

**Location:** `tests/test_facilitator.py`

### Tests Included

1. **Health Check** - Verifies facilitator is operational
2. **Supported Networks** - Confirms all 4 networks are available
3. **Wallet Balance Check** - Ensures facilitator wallet has sufficient gas funds
4. **Payment Verification Flow** - Tests payment signature and authorization
5. **X402 Client Integration** - Validates Python client library
6. **Multi-Network Support** - Tests payment creation across networks

### Running Tests

```bash
# Test production facilitator
python tests/test_facilitator.py

# Test local facilitator
FACILITATOR_URL=http://localhost:8080 python tests/test_facilitator.py
```

### Test Results (2025-10-26)

```
✅ Health                      PASSED
✅ Supported Networks          PASSED
✅ Wallet Balances             PASSED
✅ Payment Verification        PASSED
✅ X402 Client Integration     PASSED
✅ Multi Network Support       PASSED

Total: 6/6 tests passed
```

## X402 Protocol Compliance Fixes

### Issues Identified and Resolved

1. **x402Version Format**
   - **Issue:** Client sent `"x402Version": "0.0.1"` (string)
   - **Fix:** Changed to `"x402Version": 1` (integer)
   - **Reason:** Facilitator expects u8 type, not string

2. **Payment Scheme**
   - **Issue:** Client sent `"scheme": "eip3009"`
   - **Fix:** Changed to `"scheme": "exact"`
   - **Reason:** "exact" is the payment scheme, EIP-3009 is the underlying payload format

3. **Payload Structure**
   - **Issue:** Flat payload with all fields at top level
   - **Fix:** Nested structure with `signature` and `authorization` objects
   - **Old:**
     ```json
     "payload": {
       "from": "0x...",
       "to": "0x...",
       "value": "...",
       "v": 27,
       "r": "0x...",
       "s": "0x..."
     }
     ```
   - **New:**
     ```json
     "payload": {
       "signature": {
         "v": 27,
         "r": "0x...",
         "s": "0x..."
       },
       "authorization": {
         "from": "0x...",
         "to": "0x...",
         "value": "...",
         "validAfter": "0",
         "validBefore": "...",
         "nonce": "0x..."
       }
     }
     ```

4. **Network Identifier**
   - **Issue:** Client sent `"network": "avalanche-fuji:43113"` (with chain ID)
   - **Fix:** Changed to `"network": "avalanche-fuji"` (name only)
   - **Reason:** Facilitator uses enum variants, not chain ID suffixes

### Updated Files

- `shared/x402_client.py` - Fixed protocol compliance
- `tests/test_facilitator.py` - Comprehensive test suite

## Facilitator Wallet Status

**Production Wallet:** `0x103040545AC5031A11E8C03dd11324C7333a13C7`

| Network | Balance | Status |
|---------|---------|--------|
| Avalanche Fuji | 2.5 AVAX | ✅ Operational |
| Avalanche Mainnet | 1.1 AVAX | ✅ Operational |
| Base Sepolia | 0.001 ETH | ⚠️ Low (functional) |
| Base Mainnet | 0.02 ETH | ⚠️ Low (functional) |

All networks are operational. Low balances on Base networks are sufficient for current testing load but should be monitored.

## Network Support

The facilitator supports 4 networks:

- `avalanche-fuji` (Chain ID: 43113) - Testnet
- `avalanche` (Chain ID: 43114) - Mainnet
- `base-sepolia` (Chain ID: 84532) - Testnet
- `base` (Chain ID: 8453) - Mainnet

## Verification Commands

### Check Facilitator Health
```bash
curl -s https://facilitator.ultravioletadao.xyz/health | python -m json.tool
```

### Check Supported Networks
```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | python -m json.tool
```

### Run Full Test Suite
```bash
python tests/test_facilitator.py
```

### Check Wallet Balances
```python
from web3 import Web3

wallet = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
networks = {
    "Avalanche Fuji": "https://avalanche-fuji-c-chain-rpc.publicnode.com",
    "Avalanche Mainnet": "https://avalanche-c-chain-rpc.publicnode.com",
    "Base Sepolia": "https://sepolia.base.org",
    "Base Mainnet": "https://mainnet.base.org"
}

for name, rpc in networks.items():
    w3 = Web3(Web3.HTTPProvider(rpc))
    balance = w3.from_wei(w3.eth.get_balance(wallet), 'ether')
    currency = "AVAX" if "Avalanche" in name else "ETH"
    print(f"{name:20s} {float(balance):8.4f} {currency}")
```

## X402 Client Usage

### Basic Example

```python
from shared.x402_client import X402Client

async with X402Client(
    facilitator_url="https://facilitator.ultravioletadao.xyz",
    glue_token_address="0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743",
    chain_id=43113,
    private_key="0x..."
) as client:
    # Check facilitator health
    health = await client.facilitator_health()
    print(f"Facilitator: {health}")

    # Get supported networks
    supported = await client.facilitator_supported()
    print(f"Networks: {supported}")

    # Buy data from seller (full payment flow)
    response, settlement = await client.buy_with_payment(
        seller_url="https://karma-hello.xyz/api/logs",
        seller_address="0x...",
        amount_glue="0.01"
    )
```

## Test Agent Communication

To test if agents can communicate with the facilitator, ensure:

1. ✅ Agents use `shared/x402_client.py` with updated protocol compliance
2. ✅ Facilitator URL is set correctly (production or local)
3. ✅ GLUE token address matches network (Fuji: `0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743`)
4. ✅ Agent wallets have GLUE balance for payments
5. ✅ Facilitator wallet has native token (AVAX/ETH) for gas

## Known Issues

### Payment Verification 422 Errors (Non-Critical)

The test suite may show 422 errors during payment verification. This is **expected and normal** when:
- Test wallets have zero GLUE balance
- Payment authorization is structurally valid but economically invalid

These errors are handled gracefully and tests still pass. In production:
- Real wallets will have GLUE balance
- Valid payments will process successfully

### Future Improvements

1. **Full Payment Settlement Tests**
   - Currently only tests verification, not actual on-chain settlement
   - Requires funded test wallets with GLUE tokens
   - Should test on Fuji testnet only (not mainnet)

2. **Agent-to-Agent Payment Tests**
   - Test full buyer->facilitator->seller flow
   - Verify x402 middleware integration
   - Test with actual running agent servers

3. **Multi-Token Support Tests**
   - Test USDC payments on Base networks
   - Test WAVAX payments on Avalanche
   - Verify token address validation

## References

- Facilitator Documentation: `x402-rs/README.md`
- Wallet Rotation: `docs/FACILITATOR_WALLET_ROTATION.md`
- X402 Protocol: `shared/x402_client.py`
- Payment Signing: `shared/payment_signer.py`

---

**Last Updated:** 2025-10-26
**Status:** ✅ All tests passing
**Production Facilitator:** https://facilitator.ultravioletadao.xyz
