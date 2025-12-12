# Tempo Blockchain Research

**Date**: 2025-12-10
**Status**: NOT INTEGRABLE (yet)
**Testnet Launch**: December 9, 2025
**Mainnet Expected**: 2026

## Overview

Tempo is a new Layer 1 blockchain developed by **Stripe and Paradigm**, specifically designed for stablecoin payments. It launched its public testnet on December 9, 2025, with major partners including Mastercard, Klarna, UBS, and Kalshi.

Built on **Reth** (Paradigm's high-performance Rust-based Ethereum client), Tempo is EVM-compatible but introduces significant differences from standard EVM chains.

## Testnet Connection Details

| Property | Value |
|----------|-------|
| **Network Name** | Tempo Testnet (Andantino) |
| **Chain ID** | 42429 |
| **Currency Symbol** | USD |
| **HTTP RPC** | `https://rpc.testnet.tempo.xyz` |
| **WebSocket RPC** | `wss://rpc.testnet.tempo.xyz` |
| **Block Explorer** | `https://explore.tempo.xyz` |

## Predeployed Contracts (Testnet)

### System Contracts

| Contract | Address | Purpose |
|----------|---------|---------|
| TIP-20 Factory | `0x20fc000000000000000000000000000000000000` | Create new TIP-20 tokens |
| Fee Manager | `0xfeec000000000000000000000000000000000000` | Handle fee payments and conversions |
| Stablecoin DEX | `0xdec0000000000000000000000000000000000000` | Enshrined DEX for stablecoin swaps |
| TIP-403 Registry | `0x403c000000000000000000000000000000000000` | Transfer policy registry |
| pathUSD | `0x20c0000000000000000000000000000000000000` | First stablecoin deployed |

### Standard Utilities

| Contract | Address | Purpose |
|----------|---------|---------|
| Permit2 | `0x000000000022d473030f116ddee9f6b43ac78ba3` | Token approvals and transfers |
| Multicall3 | `0xcA11bde05977b3631167028862bE2a173976CA11` | Batch call execution |
| CreateX | `0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed` | Deterministic deployment |

## Key Differences from Standard EVM

### 1. No Native Gas Token

Unlike Ethereum (ETH) or other EVM chains, Tempo has **no native gas token**. Transaction fees are paid directly in stablecoins via an enshrined AMM.

- Users pay fees in USDC, USDT, or any supported stablecoin
- Enshrined AMM handles automatic conversion between stablecoins
- Wallet apps may show unusual balance values as placeholders

### 2. TIP-20 Token Standard

Tempo uses **TIP-20** instead of ERC-20 for tokens. Key questions:
- Is TIP-20 backward-compatible with ERC-20?
- Does TIP-20 support EIP-3009 `transferWithAuthorization`?
- What are the interface differences?

### 3. Custom Transaction Type (EIP-2718)

Tempo introduces a new transaction type with unique features:

1. **Configurable Fee Tokens** - Pay fees in any USD-denominated TIP-20 token
2. **Fee Sponsorship** - Third parties can pay fees on behalf of senders
3. **Batch Calls** - Multiple operations execute atomically
4. **Access Keys** - Delegated signing authority with custom permissions
5. **Concurrent Transactions** - Parallel execution using independent nonce keys
6. **Scheduled Transactions** - `validAfter` and `validBefore` timestamps (similar to EIP-3009!)

### 4. Fees Structure

- Fixed base fee model
- TIP-20 transfer costs < $0.001
- Fees go to block proposer (validator)
- Supported stablecoins must be:
  - USD-denominated
  - Issued as native TIP-20
  - Have sufficient liquidity on Fee AMM

## Blockers for x402 Integration

### 1. No USDC on Tempo

Circle has not deployed USDC to Tempo. Available stablecoins:
- **pathUSD** - First native stablecoin
- **AlphaUSD, BetaUSD, ThetaUSD** - Faucet test tokens
- **KlarnaUSD** - Klarna's stablecoin (coming to mainnet 2026)

Our facilitator requires USDC with EIP-3009 support.

### 2. Unknown EIP-3009 Support

The x402 protocol depends on `transferWithAuthorization` (EIP-3009). It's unknown whether:
- TIP-20 standard includes EIP-3009 methods
- pathUSD or future USDC would support it
- The signature verification would work the same way

### 3. Gas Model Incompatibility

Our EVM settlement code (`src/chain/evm.rs`) expects:
- Native token (ETH) for gas payments
- Standard transaction format
- Provider-based gas estimation

Tempo requires stablecoin fee payments via custom transaction type.

### 4. Transaction Format

Standard `ethers-rs` transactions may not work. Would likely need:
- Tempo-specific SDK (available in Rust, TypeScript, Go, Python)
- Custom transaction builder
- Different signing flow

## Integration Requirements (Future)

To add Tempo support, we would need:

1. **USDC Deployment** - Circle deploys native USDC with EIP-3009
2. **TIP-20 Compatibility** - Confirm `transferWithAuthorization` support
3. **Tempo SDK Integration** - Use their Rust SDK for transactions
4. **Fee Token Handling** - Modify settlement to pay fees in stablecoins
5. **Custom Transaction Builder** - Support Tempo's EIP-2718 type

### Code Changes Required

1. `src/network.rs` - Add Tempo network enum variants
2. `src/chain/` - New `tempo.rs` module or modify `evm.rs`
3. `Cargo.toml` - Add Tempo SDK dependency
4. Fee handling logic for stablecoin-based gas

## Interesting Features for x402

Despite the blockers, Tempo has features aligned with x402:

- **Scheduled Transactions** - `validAfter`/`validBefore` like EIP-3009
- **Fee Sponsorship** - Facilitator could sponsor user fees
- **Sub-cent Fees** - Perfect for micropayments
- **Stablecoin Native** - No token conversion needed

## Partners & Ecosystem

- **Stripe** - Core developer, Bridge integration
- **Paradigm** - Co-developer, built on Reth
- **Mastercard** - Payment network partner
- **Klarna** - KlarnaUSD stablecoin issuer
- **UBS** - Financial institution partner
- **Kalshi** - Prediction market partner

## Timeline

- **December 2025** - Public testnet launch
- **2026** - Mainnet launch expected
- **TBD** - Circle USDC deployment

## References

- [Tempo Documentation](https://docs.tempo.xyz/)
- [Tempo Website](https://tempo.xyz/)
- [Bloomberg: Stripe and Paradigm Open Tempo](https://www.bloomberg.com/news/articles/2025-12-09/stripe-and-paradigm-open-tempo-blockchain-to-public-add-kalshi-ubs-as-partners)
- [Tempo: Stripe's Blockchain for Stablecoin Payments](https://insights4vc.substack.com/p/tempo-stripes-blockchain-for-stablecoin)
- [Unofficial Tempo FAQ](https://www.seangoedecke.com/tempo-faq/)
- [Circle USDC Contract Addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)

## Conclusion

Tempo is a promising chain for stablecoin payments with strong backing, but it's **too early and too different** from standard EVM for our current x402 architecture. We should monitor:

1. Circle USDC deployment announcements
2. TIP-20 specification for EIP-3009 compatibility
3. Tempo Rust SDK maturity
4. Mainnet launch and stability

Revisit this assessment when mainnet launches in 2026 or when USDC is deployed.
