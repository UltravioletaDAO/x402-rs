# x402r Escrow/Refund Extension

This document describes the x402r escrow extension implementation in the x402-rs facilitator, enabling trustless refunds for x402 payments.

## Overview

The x402r proposal (see [GitHub Issue #864](https://github.com/coinbase/x402/issues/864)) introduces a trustless escrow/refund mechanism for x402 payments. Instead of paying directly to a merchant, clients sign EIP-3009 authorizations to proxy contracts that forward tokens to a shared escrow contract. This enables:

- **Trustless Refunds**: Buyers can request refunds within a dispute window
- **Merchant Protection**: Merchants can release funds after the dispute period
- **Deterministic Addresses**: Proxy addresses are computed via CREATE3 for predictability

## Architecture

### Contract Components

1. **DepositRelayFactory** - Deploys deterministic proxy contracts per merchant
2. **DepositRelay** - Stateless implementation contract used via delegatecall
3. **Escrow** - Shared contract that holds funds and manages disputes

### Contract Addresses

#### Base Mainnet
- Factory: `0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814`
- Escrow: `0xC409e6da89E54253fbA86C1CE3E553d24E03f6bC`
- Implementation: `0x55eEC2951Da58118ebf32fD925A9bBB13096e828`

#### Base Sepolia
- Factory: `0xf981D813842eE78d18ef8ac825eef8e2C8A8BaC2`
- Escrow: `0xF7F2Bc463d79Bd3E5Cb693944B422c39114De058`
- Implementation: `0x740785D15a77caCeE72De645f1bAeed880E2E99B`

## Payment Flow

```
Client                   Facilitator              Proxy         Escrow
  |                          |                      |              |
  |  1. Sign EIP-3009 to    |                      |              |
  |     proxy address       |                      |              |
  |------------------------->|                      |              |
  |                          |                      |              |
  |  2. POST /settle with   |                      |              |
  |     refund extension    |                      |              |
  |------------------------->|                      |              |
  |                          |                      |              |
  |                          | 3. Verify proxy     |              |
  |                          |    (deterministic)  |              |
  |                          |--.                  |              |
  |                          |<-'                  |              |
  |                          |                      |              |
  |                          | 4. Call             |              |
  |                          |    executeDeposit   |              |
  |                          |--------------------->|              |
  |                          |                      |              |
  |                          |                      | 5. Forward   |
  |                          |                      |    to escrow |
  |                          |                      |------------->|
  |                          |                      |              |
  |  6. Settlement response  |                      |              |
  |<-------------------------|                      |              |
```

## Protocol Extension Format

The x402r extension is included in the `extensions` field of the v2 payment payload:

```json
{
  "paymentPayload": {
    "x402Version": 2,
    "network": "eip155:8453",
    "scheme": "exact",
    "accepted": {
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "payTo": "0x<proxy-address>",
      "amount": "10000"
    },
    "payload": {
      "signature": "0x...",
      "authorization": {
        "from": "0x<payer>",
        "to": "0x<proxy-address>",
        "value": "10000",
        "validAfter": "1700000000",
        "validBefore": "1700003600",
        "nonce": "0x..."
      }
    },
    "extensions": {
      "refund": {
        "info": {
          "factoryAddress": "0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814",
          "proxies": {
            "0x<proxy-address>": "0x<merchant-payout-address>"
          }
        }
      }
    }
  }
}
```

## Configuration

### Feature Flag

Escrow settlement is disabled by default. Enable it with:

```bash
export ENABLE_ESCROW=true
```

### Supported Networks

Currently only Base networks are supported:
- `base-mainnet` / `eip155:8453`
- `base-sepolia` / `eip155:84532`

## Implementation Details

### CREATE3 Address Computation

Proxy addresses are computed deterministically:

1. **Raw Salt**: `keccak256(factory || merchantPayout)`
2. **Guarded Salt**: `keccak256(factory || rawSalt)` (CreateX salt guarding)
3. **Proxy Address**: CREATE3 from CreateX deployer with guarded salt

This allows verification without on-chain calls.

### Verification Steps

1. **Feature Check**: Verify `ENABLE_ESCROW=true`
2. **Extension Parsing**: Extract `refund` extension from payload
3. **Proxy Validation**: Verify `payTo` address matches computed proxy
4. **On-Chain Check** (optional): Query factory to confirm proxy registration
5. **Settlement**: Call `executeDeposit` on the proxy contract

### Error Handling

The escrow module defines specific errors for clear debugging:

- `FeatureDisabled` - ENABLE_ESCROW is not set
- `MissingRefundExtension` - No refund extension in payload
- `InvalidProxyAddress` - Computed address doesn't match
- `UnsupportedNetwork` - Network doesn't have deployed factory
- `ProxyVerificationFailed` - On-chain verification failed

## Code Structure

```
src/
  escrow.rs          # Main escrow module
    - Contract bindings (sol! macro)
    - CREATE3 address computation
    - Proxy verification
    - Settlement logic

abi/
  DepositRelay.json         # Proxy contract ABI
  DepositRelayFactory.json  # Factory contract ABI
```

### Key Functions

- `is_escrow_enabled()` - Check feature flag
- `compute_proxy_address()` - Deterministic address computation
- `verify_proxy_deterministic()` - Off-chain verification
- `verify_proxy_onchain()` - On-chain factory query
- `settle_with_escrow()` - Main settlement entrypoint

## Testing

### Unit Tests

```bash
cargo test escrow
```

Tests cover:
- Deterministic address computation
- Factory address lookups
- Feature flag behavior
- Extension parsing

### Integration Testing

For integration testing against real contracts:

1. Start a local Anvil fork of Base Sepolia
2. Set `ENABLE_ESCROW=true`
3. Submit a payment with refund extension
4. Verify the escrow contract received the deposit

## Security Considerations

1. **Proxy Verification**: Always verify proxy addresses match deterministic computation
2. **Factory Validation**: On-chain verification provides extra assurance
3. **Network Matching**: Only process escrow on networks with deployed factories
4. **Feature Flag**: Keep disabled in production until fully tested

## Future Enhancements

- Additional network support (Ethereum mainnet, Arbitrum, etc.)
- Refund initiation endpoint
- Dispute resolution integration
- Escrow balance queries

## References

- [x402r Proposal (GitHub Issue #864)](https://github.com/coinbase/x402/issues/864)
- [x402r Contracts Repository](https://github.com/BackTrackCo/x402r-contracts)
- [CreateX Universal Deployer](https://github.com/pcaversaccio/createx)
- [EIP-3009: Transfer with Authorization](https://eips.ethereum.org/EIPS/eip-3009)
