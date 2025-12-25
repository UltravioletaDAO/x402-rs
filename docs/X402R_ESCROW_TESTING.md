# x402r Escrow Extension - Testing Guide

## Overview

The x402r escrow extension enables trustless refunds via escrow proxy contracts. This is **NOT** a new endpoint - it's an extension to the existing `/settle` endpoint that activates when a `refund` extension is present in the payment payload.

## How It Works

### Architecture

```
┌─────────────┐     ┌─────────────────┐     ┌──────────────────┐     ┌──────────┐
│   Client    │────▶│   Facilitator   │────▶│  DepositRelay    │────▶│  Escrow  │
│  (signs)    │     │  (pays gas)     │     │  (proxy)         │     │ (holds)  │
└─────────────┘     └─────────────────┘     └──────────────────┘     └──────────┘
                                                     │
                                                     ▼
                                              ┌──────────────┐
                                              │   Merchant   │
                                              │  (receives)  │
                                              └──────────────┘
```

### Flow

1. **Merchant registers** with escrow contract: `escrow.registerMerchant(arbiter)`
2. **Factory deploys** deterministic proxy for merchant via CREATE3
3. **Client signs** EIP-3009 authorization to the **proxy address** (not merchant directly)
4. **Client sends** payment to facilitator with `refund` extension
5. **Facilitator detects** `refund` extension and routes to escrow settlement
6. **Facilitator verifies** proxy address matches computed CREATE3 address
7. **Facilitator calls** `DepositRelay.transferWithAuthorizationOnBehalf()`
8. **Proxy forwards** tokens to Escrow with deposit record
9. **Funds held** in escrow until release or refund

## Contract Addresses

### Base Mainnet (Chain ID: 8453)

| Contract | Address |
|----------|---------|
| CreateX | `0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed` |
| Escrow | `0xC409e6da89E54253fbA86C1CE3E553d24E03f6bC` |
| DepositRelayFactory | `0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814` |
| DepositRelay (impl) | `0x55eEC2951Da58118ebf32fD925A9bBB13096e828` |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |

### Base Sepolia (Chain ID: 84532)

| Contract | Address |
|----------|---------|
| CreateX | `0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed` |
| Escrow | `0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058` |
| DepositRelayFactory | `0xf981D813842eE78d18ef8ac825eef8e2C8A8BaC2` |
| DepositRelay (impl) | `0x740785D15a77caCeE72De645f1bAeed880E2E99B` |
| USDC | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` |

## Request Format

### Standard Settle Request (without escrow)

```json
{
  "paymentPayload": {
    "x402Version": 2,
    "accepted": {
      "scheme": "exact",
      "network": "eip155:84532",
      "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      "amount": "1000000",
      "payTo": "0xMerchantAddress..."
    },
    "authorization": {
      "from": "0xPayerAddress...",
      "signature": "0x..."
    },
    "extensions": {}
  }
}
```

### Escrow Settle Request (with refund extension)

```json
{
  "paymentPayload": {
    "x402Version": 2,
    "accepted": {
      "scheme": "exact",
      "network": "eip155:84532",
      "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      "amount": "1000000",
      "payTo": "0xProxyAddress..."
    },
    "authorization": {
      "from": "0xPayerAddress...",
      "signature": "0x..."
    },
    "extensions": {
      "refund": {
        "info": {
          "factoryAddress": "0xf981D813842eE78d18ef8ac825eef8e2C8A8BaC2",
          "merchantPayouts": {
            "0xProxyAddress...": "0xMerchantPayoutAddress..."
          }
        }
      }
    }
  }
}
```

### Key Differences

| Field | Standard | Escrow |
|-------|----------|--------|
| `payTo` | Merchant address | Proxy address |
| `extensions.refund` | Not present | Required with factory and merchantPayouts |
| Signature target | Merchant | Proxy |

## Testing Examples

### 1. Verify Escrow is Enabled

```bash
# Check version (should be 1.14.1+)
curl -s https://facilitator.ultravioletadao.xyz/version
# => {"version":"1.14.1"}

# Escrow enabled via ENABLE_ESCROW=true in task definition
```

### 2. Compute Proxy Address (for testing)

The proxy address is deterministically computed from:
- Factory address
- Merchant payout address

```python
from eth_abi import encode
from eth_utils import keccak

def compute_proxy_address(factory: str, merchant: str) -> str:
    """Compute CREATE3 proxy address"""
    factory_bytes = bytes.fromhex(factory[2:])
    merchant_bytes = bytes.fromhex(merchant[2:])

    # Step 1: raw salt = keccak256(factory || merchant)
    raw_salt = keccak(factory_bytes + merchant_bytes)

    # Step 2: guarded salt = keccak256(factory || raw_salt)
    guarded_salt = keccak(factory_bytes + raw_salt)

    # Step 3: CREATE3 address computation
    createx = bytes.fromhex("ba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed")
    init_code_hash = bytes.fromhex("21c35dbe1b344a2488cf3321d6ce542f8e9f305544ff09e4993a62319a497c1f")

    # CREATE2 proxy address
    create2_input = b'\xff' + createx + guarded_salt + init_code_hash
    proxy_addr = keccak(create2_input)[12:]

    # CREATE address from proxy (nonce=1)
    rlp = b'\xd6\x94' + proxy_addr + b'\x01'
    final_addr = keccak(rlp)[12:]

    return "0x" + final_addr.hex()

# Example
factory = "0xf981D813842eE78d18ef8ac825eef8e2C8A8BaC2"  # Base Sepolia
merchant = "0x1234567890123456789012345678901234567890"
proxy = compute_proxy_address(factory, merchant)
print(f"Proxy address: {proxy}")
```

### 3. Test Escrow Settlement (Base Sepolia)

```bash
# Full escrow settle request
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Content-Type: application/json" \
  -d '{
    "paymentPayload": {
      "x402Version": 2,
      "accepted": {
        "scheme": "exact",
        "network": "eip155:84532",
        "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        "amount": "1000000",
        "payTo": "0xCOMPUTED_PROXY_ADDRESS"
      },
      "authorization": {
        "from": "0xPAYER_ADDRESS",
        "signature": "0xSIGNED_FOR_PROXY_ADDRESS"
      },
      "extensions": {
        "refund": {
          "info": {
            "factoryAddress": "0xf981D813842eE78d18ef8ac825eef8e2C8A8BaC2",
            "merchantPayouts": {
              "0xCOMPUTED_PROXY_ADDRESS": "0xMERCHANT_PAYOUT_ADDRESS"
            }
          }
        }
      }
    }
  }'
```

### 4. Error Scenarios

```bash
# Missing refund extension (treated as normal settle)
# Works normally, no escrow routing

# Wrong proxy address
# Error: "Invalid proxy address: expected 0x..., computed 0x..."

# Unsupported network (e.g., Polygon)
# Error: "Network polygon does not support escrow (factory not deployed)"

# Escrow disabled
# Error: "Escrow feature is disabled. Set ENABLE_ESCROW=true to enable."
```

## Merchant Registration (Prerequisite)

Before using escrow, merchants must register:

```solidity
// On Base Sepolia
IEscrow escrow = IEscrow(0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058);

// Register with an arbiter address for disputes
escrow.registerMerchant(arbiterAddress);
```

Or via cast:

```bash
cast send 0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058 \
  "registerMerchant(address)" \
  0xARBITER_ADDRESS \
  --rpc-url https://sepolia.base.org \
  --private-key $MERCHANT_PRIVATE_KEY
```

## Why Escrow Doesn't Appear in /supported

The `/supported` endpoint lists **payment schemes** (exact, streaming, etc.) and **networks** (base, polygon, etc.). Escrow is an **extension** to the existing settlement flow, not a new scheme or network.

Think of it like HTTP headers - you don't list all possible headers in the API spec, but the server supports them when present.

## Response Format

Escrow settlements return the same response format as normal settlements:

```json
{
  "transaction": "0xTRANSACTION_HASH",
  "network": "base-sepolia"
}
```

The transaction is the `transferWithAuthorizationOnBehalf` call to the DepositRelay proxy.

## Verifying Escrow Deposits

After settlement, verify the deposit in the Escrow contract:

```bash
# Check deposit on Base Sepolia
cast call 0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058 \
  "deposits(address,bytes32)(uint256,uint256,uint8)" \
  0xMERCHANT_ADDRESS \
  0xDEPOSIT_ID \
  --rpc-url https://sepolia.base.org
```

## Links

- **x402r Proposal**: https://github.com/coinbase/x402/issues/864
- **x402r Contracts**: https://github.com/BackTrackCo/x402r-contracts
- **Base Sepolia Escrow**: https://sepolia.basescan.org/address/0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058
- **Base Mainnet Escrow**: https://basescan.org/address/0xC409e6da89E54253fbA86C1CE3E553d24E03f6bC

## Warning

⚠️ The x402r contracts have NOT been audited. Use at your own risk, especially on mainnet.
