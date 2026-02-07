# x402r Escrow Multi-Chain Deployment

## Overview

As of February 2026, the x402r team (BackTrackCo) deployed escrow contracts on **9 EVM networks** via the `A1igator/multichain-config` branch of the x402r-sdk. This document records the contract addresses, deployment status, and next steps.

**SDK Source**: https://github.com/BackTrackCo/x402r-sdk/blob/A1igator/multichain-config/packages/core/src/config/index.ts

## Contract Addresses by Network

### Base Sepolia (eip155:84532) - Testnet

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x29025c0E9D4239d438e169570818dB9FE0A80873` |
| PaymentOperatorFactory | `0x97d53e63A9CB97556c00BeFd325AF810c9b267B2` |
| TokenCollector | `0x5cA789000070DF15b4663DB64a50AeF5D49c5Ee0` |
| ProtocolFeeConfig | `0x8F96C493bAC365E41f0315cf45830069EBbDCaCe` |
| RefundRequest | `0x1C2Ab244aC8bDdDB74d43389FF34B118aF2E90F4` |
| USDC | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` |

### Base Mainnet (eip155:8453)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0xb9488351E48b23D798f24e8174514F28B741Eb4f` |
| PaymentOperatorFactory | `0x3D0837fF8Ea36F417261577b9BA568400A840260` |
| TokenCollector | `0x48ADf6E37F9b31dC2AAD0462C5862B5422C736B8` |
| ProtocolFeeConfig | `0x59314674BAbb1a24Eb2704468a9cCdD50668a1C6` |
| RefundRequest | `0x35fb2EFEfAc3Ee9f6E52A9AAE5C9655bC08dEc00` |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |

### Ethereum Sepolia (eip155:11155111) - Testnet

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| PaymentOperatorFactory | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenCollector | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| ProtocolFeeConfig | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238` |

### Ethereum Mainnet (eip155:1)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| PaymentOperatorFactory | `0xed02d3E5167BCc9582D851885A89b050AB816a56` |
| TokenCollector | `0xE78648e7af7B1BaDE717FF6E410B922F92adE80f` |
| ProtocolFeeConfig | `0xb33D6502EdBbC47201cd1E53C49d703EC0a660b8` |
| RefundRequest | `0xc9BbA6A2CF9838e7Dd8c19BC8B3BAC620B9D8178` |
| USDC | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` |

### Polygon PoS (eip155:137)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| PaymentOperatorFactory | `0xb33D6502EdBbC47201cd1E53C49d703EC0a660b8` |
| TokenCollector | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| ProtocolFeeConfig | `0xE78648e7af7B1BaDE717FF6E410B922F92adE80f` |
| RefundRequest | `0xed02d3E5167BCc9582D851885A89b050AB816a56` |
| USDC | `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` |

### Arbitrum One (eip155:42161)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| PaymentOperatorFactory | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenCollector | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| ProtocolFeeConfig | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` |

### Celo (eip155:42220)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| PaymentOperatorFactory | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenCollector | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| ProtocolFeeConfig | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0xcebA9300f2b948710d2653dD7B07f33A8B32118C` |

### Monad (eip155:143)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| PaymentOperatorFactory | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenCollector | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| ProtocolFeeConfig | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0x754704Bc059F8C67012fEd69BC8A327a5aafb603` |

### Avalanche C-Chain (eip155:43114)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| PaymentOperatorFactory | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenCollector | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| ProtocolFeeConfig | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E` |

## Key Findings

### 1. SDK Redeployed ALL Contracts

The x402r team deployed a completely new set of contracts on the `A1igator/multichain-config` branch. This means:

- **Base Sepolia**: All addresses changed (old escrow was `0xb9488351...`, new is `0x29025c0E...`)
- **Base Mainnet**: All addresses changed (old escrow was `0x320a3c35...`, new is `0xb9488351...`)
- **Old PaymentOperator** (`0xa06958D93135BEd7e43893897C0d9fA931EF051C`) on Base Mainnet was deployed from the OLD factory for the OLD escrow. It will NOT work with the new contracts.

### 2. Deterministic Addresses Across Chains

Many of the new chains share the same contract addresses (likely deployed with CREATE2):

- Arbitrum, Celo, Monad, Avalanche all share: `ESCROW=0x320a3c35...`, `FACTORY=0x32d6AC59...`, `TOKEN_COLLECTOR=0x230fd3A1...`
- Ethereum Sepolia shares these same addresses
- Base and Polygon have unique addresses

### 3. BSC Excluded

Ali confirmed BSC was excluded because USDC on BSC does not support EIP-3009 `transferWithAuthorization`.

## Changes Made to Facilitator

### `src/payment_operator/addresses.rs`

- Updated Base Sepolia and Base Mainnet addresses to new SDK deployment
- Added 7 new network modules: `ethereum_sepolia`, `ethereum_mainnet`, `polygon`, `arbitrum`, `celo`, `monad`, `avalanche`
- Added `ESCROW_NETWORKS` constant (9 networks) as single source of truth
- Updated all helper functions to support 9 networks
- Set `payment_operator: None` for ALL networks (operators need deployment)

### `src/facilitator_local.rs`

- Replaced hardcoded Base-only escrow advertising with dynamic loop over `ESCROW_NETWORKS`
- `/supported` endpoint only shows networks with deployed PaymentOperator (`payment_operator.is_some()`)
- Currently advertises 0 escrow networks (correct - no operators deployed yet)

### `src/openapi.rs`

- Updated Swagger docs to list all 9 escrow networks
- Updated `/supported` endpoint docs with escrow scheme examples

## Next Steps: Operator Deployment

For each network, deploy a PaymentOperator using the factory:

### 1. Deploy Operators

Use `tests/escrow/deploy_operator.py` (update for new factory addresses):

```python
# For each network:
# 1. Connect wallet to the network's RPC
# 2. Call factory.deployOperator(feeRecipient, conditions)
# 3. Record the deployed operator address
```

### 2. Update Code

For each deployed operator, update `src/payment_operator/addresses.rs`:

```rust
Network::Base => Some(Self {
    payment_operator: Some(address!("DEPLOYED_OPERATOR_ADDRESS_HERE")),
    // ...other addresses stay the same
}),
```

### 3. Verify

After deployment and code update:
1. Build and deploy facilitator
2. Check `/supported` endpoint includes escrow entries
3. Test settlement on each network

## Security Considerations

- `validate_addresses()` in `operator.rs` checks ALL client-provided addresses against hardcoded values
- Settlement fails safely with clear error if operator not deployed (returns `OperatorError::UnsupportedNetwork`)
- `ENABLE_PAYMENT_OPERATOR=true` is required in production (already set)
- Gas drain attacks prevented by address validation on escrow, token_collector, and operator addresses
