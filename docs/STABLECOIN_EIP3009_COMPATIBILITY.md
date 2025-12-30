# Stablecoin EIP-3009 Compatibility Report

## For x402 Payment Facilitator Integration

**Date**: December 2025
**Author**: Claude Code (Ultravioleta DAO)
**Version**: 1.0

---

## Executive Summary

The x402 protocol requires **EIP-3009 (transferWithAuthorization)** for gasless meta-transactions. This standard enables single-call settlement where the facilitator submits one transaction that both verifies the signature AND transfers tokens atomically.

**Key Finding**: Only a small subset of stablecoins implement EIP-3009. The majority use either EIP-2612 (permit only) or no gasless standard at all.

### Quick Reference

| Compatibility | Count | Examples |
|---------------|-------|----------|
| Full EIP-3009 | 6 | USDC, EURC, PYUSD, USDP, AUSD, USDT0 |
| EIP-2612 Only | 8+ | DAI, crvUSD, FRAX, GHO, LUSD, MIM, ZCHF |
| No Support | 3+ | USDT (original), TUSD, FDUSD |

---

## Table of Contents

1. [Curve Finance Networks](#curve-finance-networks)
2. [Stablecoin Compatibility Matrix](#stablecoin-compatibility-matrix)
3. [Detailed Token Analysis](#detailed-token-analysis)
4. [Technical Background](#technical-background)
5. [Integration Recommendations](#integration-recommendations)
6. [Domain Separator Reference](#domain-separator-reference)
7. [Sources](#sources)

---

## Curve Finance Networks

Curve Finance operates on **20+ EVM networks**, making it one of the most widely deployed DeFi protocols. The x402 facilitator should prioritize networks where both Curve has liquidity AND EIP-3009 tokens are deployed.

### Core Networks

| Network | Chain ID | Curve Status | USDC Native | Notes |
|---------|----------|--------------|-------------|-------|
| Ethereum | 1 | Core (DAO home) | Yes | Primary deployment |
| Arbitrum | 42161 | Active | Yes | High volume L2 |
| Optimism | 10 | Active | Yes | OP Stack L2 |
| Base | 8453 | Active | Yes | Coinbase L2, growing fast |
| Polygon | 137 | Active | Yes* | *Bridged USDC.e NOT compatible |
| Avalanche | 43114 | Active | Yes | C-Chain |
| BNB Chain | 56 | Active | No native | USDC via bridges only |
| Gnosis | 100 | Active | Yes | xDai chain |
| Fantom | 250 | Active | Limited | Opera network |
| Fraxtal | 252 | Active | Yes | Frax L2 |
| Celo | 42220 | Active | Yes | Mobile-first |

### New Networks (2025)

| Network | Chain ID | Curve Status | USDC Native | Notes |
|---------|----------|--------------|-------------|-------|
| HyperEVM | 999 | Full deployment | Yes | Hyperliquid ecosystem |
| Monad | TBD | Live | Yes | High-performance L1 |
| X Layer | 196 | Live | Yes | OKX L2 |
| Etherlink | TBD | Live | TBD | Tezos L2 |
| Plasma | TBD | Live | TBD | Tether-backed, stablecoin-focused |
| TAC (TON) | N/A | Live | TBD | TON ecosystem |

---

## Stablecoin Compatibility Matrix

### Compatible with x402 (EIP-3009 Implemented)

| Token | Symbol | Issuer | Networks | Contract Pattern |
|-------|--------|--------|----------|------------------|
| USD Coin | USDC | Circle | 28+ chains | Reference implementation |
| Euro Coin | EURC | Circle | 5 chains | Same as USDC |
| PayPal USD | PYUSD | Paxos | Ethereum, Solana | Includes batch transfers |
| Pax Dollar | USDP | Paxos | Ethereum | Full EIP-3009 |
| Agora Dollar | AUSD | Agora | 5+ chains | Full EIP-3009 |
| USDT0 | USDT0 | Tether | LayerZero OFT | NEW omnichain version |

### Partially Compatible (EIP-2612 Only)

These tokens support gasless **approvals** but require a second `transferFrom` call, making them incompatible with x402's single-call settlement model:

| Token | Symbol | Issuer | Networks | Why Not Compatible |
|-------|--------|--------|----------|-------------------|
| DAI | DAI | MakerDAO/Sky | Multi-chain | Permit only, no transferWithAuth |
| Sky Dollar | USDS | Sky Protocol | Ethereum | Successor to DAI, same limitation |
| LUSD | LUSD | Liquity | Ethereum | EIP-2612 via OpenZeppelin |
| GHO | GHO | Aave | Ethereum, Arbitrum | Permit only |
| FRAX | FRAX | Frax Finance | Multi-chain | Permit only |
| crvUSD | crvUSD | Curve | Multi-chain | Vyper contract, permit only |
| MIM | MIM | Abracadabra | Multi-chain | Custom transferWithPermit |
| Frankencoin | ZCHF | Frankencoin | Ethereum, Optimism | ERC20PermitLight |

### Not Compatible (No Gasless Standard)

| Token | Symbol | Issuer | Networks | Status |
|-------|--------|--------|----------|--------|
| Tether USD | USDT | Tether | All major | **No plans to implement** |
| TrueUSD | TUSD | TrustToken | Multi-chain | No native support |
| First Digital USD | FDUSD | First Digital | Ethereum, BSC, Sui | Unconfirmed |
| sUSD | sUSD | Synthetix | Ethereum, Optimism | Standard ERC-20 only |

---

## Detailed Token Analysis

### USDC (Circle) - RECOMMENDED

**Status**: Full EIP-3009 Support

USD Coin is the reference implementation for EIP-3009, developed by Circle and Coinbase. It offers the broadest network support and is the primary token for x402 payments.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | Yes (v2 contracts) |
| EIP-2612 | Yes |
| Networks | 28+ (Ethereum, Base, Arbitrum, Optimism, Polygon, Avalanche, Celo, etc.) |
| Decimals | 6 |

**Important Caveats**:
- Polygon bridged USDC (USDC.e) uses a different message structure and is NOT EIP-3009 compatible
- Domain name varies by chain (see Domain Separator Reference)
- Only USDC v2 contracts support EIP-3009; legacy deployments may not

**Already in x402-rs**: Yes (all major networks)

---

### EURC (Circle)

**Status**: Full EIP-3009 Support

Euro Coin follows the same contract pattern as USDC, making integration straightforward.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | Yes |
| EIP-2612 | Yes |
| Networks | Ethereum, Base, Avalanche, Solana, Stellar |
| Decimals | 6 |

**Domain Name Variation**:
- Ethereum/Avalanche: `"Euro Coin"`
- Base: `"EURC"`

**Already in x402-rs**: Yes (Ethereum, Base)

---

### PYUSD (PayPal/Paxos)

**Status**: Full EIP-3009 Support

PayPal USD is issued by Paxos and includes advanced features like batch transfers.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | Yes |
| EIP-2612 | Yes |
| Networks | Ethereum, Solana |
| Decimals | 6 |
| Contract (ETH) | `0x6c3ea9036406852006290770bedfcaba0e23a0e8` |

**Unique Features**:
- `transferWithAuthorizationBatch()` for multiple transfers in one call
- Audited by Trail of Bits and Zellic

**Already in x402-rs**: No

---

### USDP (Paxos)

**Status**: Full EIP-3009 Support

Pax Dollar (formerly Paxos Standard) shares the same contract architecture as PYUSD.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | Yes |
| EIP-2612 | Yes |
| Networks | Ethereum |
| Decimals | 18 |

**Already in x402-rs**: No

---

### AUSD (Agora)

**Status**: Full EIP-3009 Support

Agora Dollar is a newer stablecoin with gas-optimized contracts and full EIP-3009 support.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | Yes |
| EIP-2612 | Yes |
| Networks | Ethereum, Avalanche, Sui, Base, BSC |
| Decimals | 6 |
| Contract (ETH) | `0x00000000efe302beaa2b3e6e1b18d08d69a9012a` |

**Already in x402-rs**: Yes (BSC mainnet)

---

### USDT0 (Tether Omnichain)

**Status**: Full EIP-3009 Support

USDT0 is Tether's new omnichain token using LayerZero's OFT standard. Unlike original USDT, it implements modern token standards.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | Yes |
| EIP-2612 | Yes |
| Networks | Multiple via LayerZero |
| Decimals | 6 |

**Important Notes**:
- This is a **separate token** from original USDT
- Lower liquidity than original USDT
- Designed for cross-chain transfers

**Already in x402-rs**: No

---

### USDT (Tether) - NOT COMPATIBLE

**Status**: No EIP-3009 Support

Tether USD is the largest stablecoin by market cap but implements neither EIP-3009 nor EIP-2612. Tether has publicly stated they have **no plans** to add these standards.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | No |
| Market Cap | ~$140B (largest stablecoin) |
| Networks | All major chains |

**Impact**: Approximately 50% of stablecoin market cap cannot be used with x402 natively.

**Workaround**: Use USDT0 (omnichain version) or implement an EIP-3009 Forwarder contract (requires user approval first).

---

### DAI (MakerDAO/Sky) - NOT COMPATIBLE

**Status**: EIP-2612 Only

DAI was one of the first tokens to implement EIP-2612 permit, but it does not support EIP-3009 transferWithAuthorization.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | Yes |
| Networks | Multi-chain |
| Decimals | 18 |

**Why Not Compatible**: EIP-2612 permit only sets an allowance; a separate `transferFrom` call is still required.

---

### crvUSD (Curve) - NOT COMPATIBLE

**Status**: EIP-2612 Only

Curve's native stablecoin is written in Vyper and implements EIP-2612 permit but not EIP-3009.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | Yes |
| Networks | Ethereum, L2s |
| Decimals | 18 |
| Contract (ETH) | `0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E` |

**Irony**: Curve's own stablecoin is not compatible with x402.

---

### FRAX - NOT COMPATIBLE

**Status**: EIP-2612 Only

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | Yes |
| Networks | Multi-chain (including Fraxtal L2) |
| Related Tokens | FPI, frxETH, sfrxETH |

---

### GHO (Aave) - NOT COMPATIBLE

**Status**: EIP-2612 Only

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | Yes |
| Networks | Ethereum, Arbitrum |
| Decimals | 18 |

---

### LUSD (Liquity) - NOT COMPATIBLE

**Status**: EIP-2612 Only

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | Yes (OpenZeppelin ERC20Permit) |
| Networks | Ethereum |
| Decimals | 18 |
| Contract (ETH) | `0x5f98805A4E8be255a32880FDeC7F6728C6568bA0` |

**Note**: LUSD is fully decentralized with no governance, making upgrades unlikely.

---

### MIM (Abracadabra) - NOT COMPATIBLE

**Status**: Custom Standard

MIM implements a custom `transferWithPermit` function that is similar to but not identical to EIP-3009.

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No (custom variant) |
| EIP-2612 | Yes |
| Networks | Ethereum, Arbitrum, BSC, Fantom, Avalanche |

---

### ZCHF (Frankencoin) - NOT COMPATIBLE

**Status**: EIP-2612 Only

| Attribute | Value |
|-----------|-------|
| EIP-3009 | No |
| EIP-2612 | Yes (ERC20PermitLight) |
| Networks | Ethereum, Optimism |
| Decimals | 18 |
| Contract (ETH) | `0xB58E61C3098d85632Df34EecfB899A1Ed80921cB` |

---

## Technical Background

### EIP-3009 vs EIP-2612

| Feature | EIP-2612 (Permit) | EIP-3009 (TransferWithAuth) |
|---------|-------------------|------------------------------|
| **Purpose** | Set allowance | Execute transfer |
| **On-chain calls** | 2 (permit + transferFrom) | 1 (atomic) |
| **Nonce type** | Sequential | Random 32-byte |
| **Replay protection** | Nonce + deadline | Nonce + validAfter/validBefore |
| **Best for** | DeFi approvals | One-time payments |
| **x402 compatible** | No | Yes |

### EIP-3009 Function Signature

```solidity
function transferWithAuthorization(
    address from,
    address to,
    uint256 value,
    uint256 validAfter,
    uint256 validBefore,
    bytes32 nonce,
    uint8 v,
    bytes32 r,
    bytes32 s
) external;
```

### EIP-712 Domain Separator

EIP-3009 uses EIP-712 typed data signing with a domain separator that includes:
- `name`: Token name (varies by deployment)
- `version`: Usually "1" or "2"
- `chainId`: Network chain ID
- `verifyingContract`: Token contract address

This prevents signature replay across chains and contracts.

---

## Integration Recommendations

### Priority 1: High Value, Low Effort

Tokens that implement EIP-3009 and would expand x402-rs coverage:

| Token | Network | Effort | Value |
|-------|---------|--------|-------|
| PYUSD | Ethereum | Low | PayPal ecosystem, institutional |
| USDP | Ethereum | Low | Paxos ecosystem |
| USDT0 | Multi-chain | Medium | Access Tether users |

### Priority 2: Network Expansion

Add USDC/EURC support to new Curve networks:

| Network | Token | Notes |
|---------|-------|-------|
| Monad | USDC | New high-perf L1 |
| X Layer | USDC | OKX ecosystem |
| Fraxtal | USDC | Frax L2 |

### Priority 3: Not Feasible (Protocol Change Required)

These would require implementing a two-step settlement flow:

- DAI, USDS (Sky)
- crvUSD
- FRAX
- GHO
- LUSD
- MIM
- ZCHF

**Alternative**: Deploy EIP-3009 Forwarder contracts that users approve once, then use for x402 payments. This adds complexity and requires user setup.

### Not Recommended

| Token | Reason |
|-------|--------|
| USDT (original) | No gasless standard, no plans to add |
| FDUSD | Unconfirmed support |
| TUSD | No confirmed support |

---

## Domain Separator Reference

Critical for signature verification. Domain names vary by chain:

### USDC Domains

| Network | Domain Name | Version |
|---------|-------------|---------|
| Ethereum | "USD Coin" | "2" |
| Base | "USDC" | "2" |
| Arbitrum | "USD Coin" | "2" |
| Optimism | "USD Coin" | "2" |
| Polygon | "USD Coin" | "2" |
| Avalanche | "USD Coin" | "2" |
| Celo | "USDC" | "2" |
| HyperEVM | "USDC" | "2" |

### EURC Domains

| Network | Domain Name | Version |
|---------|-------------|---------|
| Ethereum | "Euro Coin" | "2" |
| Base | "EURC" | "2" |
| Avalanche | "Euro Coin" | "2" |

### Handling Domain Variations

The x402-rs facilitator resolves domains in this priority order:
1. `PaymentRequirements.extra.name/version` (client-provided)
2. Static lookup in `src/network.rs` (known deployments)
3. On-chain `token.name()`/`token.version()` calls (fallback)

For non-USDC tokens, clients should always provide domain info in the `extra` field.

---

## Sources

### Official Documentation
- [Circle Multi-chain USDC](https://www.circle.com/multi-chain-usdc)
- [Circle EURC](https://www.circle.com/eurc)
- [Coinbase x402 Network Support](https://docs.cdp.coinbase.com/x402/network-support)
- [Curve Finance Documentation](https://curve.readthedocs.io/)
- [Aave GHO Documentation](https://aave.com/docs/developers/gho)

### EIP Specifications
- [EIP-3009: Transfer With Authorization](https://eips.ethereum.org/EIPS/eip-3009)
- [EIP-2612: Permit](https://eips.ethereum.org/EIPS/eip-2612)
- [EIP-712: Typed Structured Data Hashing](https://eips.ethereum.org/EIPS/eip-712)

### Contract Repositories
- [Circle Stablecoin EVM](https://github.com/circlefin/stablecoin-evm)
- [Coinbase EIP-3009](https://github.com/CoinbaseStablecoin/eip-3009)
- [Paxos PYUSD Contract](https://github.com/paxosglobal/pyusd-contract)
- [Paxos USDP Contract](https://github.com/paxosglobal/usdp-contracts)
- [Liquity LUSD](https://github.com/liquity/dev)

### Articles and Guides
- [Extropy: Overview of EIP-3009](https://academy.extropy.io/pages/articles/review-eip-3009.html)
- [Circle: 4 Ways to Authorize USDC Interactions](https://www.circle.com/blog/four-ways-to-authorize-usdc-smart-contract-interactions-with-circle-sdk)
- [x402 Protocol](https://www.x402.org/)

---

## Conclusion

For maximum x402 compatibility across Curve networks, focus on:

1. **Circle tokens (USDC, EURC)** - Best supported, widest network coverage
2. **Paxos tokens (PYUSD, USDP, AUSD)** - Full EIP-3009, institutional backing
3. **USDT0** - Access to Tether ecosystem with modern standards

The largest gap remains **original USDT** (~50% of stablecoin market cap) which has no EIP-3009 support and no plans to add it. Users requiring USDT payments would need to use USDT0 or alternative solutions.

---

*Report generated by Claude Code for Ultravioleta DAO x402-rs facilitator*
