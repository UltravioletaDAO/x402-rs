# Scroll Network Integration

**Date:** January 23, 2026
**Version:** v1.21.0
**Status:** Complete (Mainnet only)

## Overview

Scroll is a zkEVM Layer 2 scaling solution built on Ethereum. This integration adds Scroll mainnet support to the x402-rs facilitator.

**Note:** Scroll Sepolia testnet is NOT supported because Circle has not deployed official USDC on that network.

## Network Details

| Parameter | Mainnet |
|-----------|---------|
| **Network Name** | `scroll` |
| **CAIP-2** | `eip155:534352` |
| **Chain ID** | 534352 |
| **RPC URL** | `https://rpc.scroll.io` |
| **Explorer** | [scrollscan.com](https://scrollscan.com) |
| **Native Token** | ETH |
| **EIP-1559** | Supported |
| **Brand Color** | #FFEEDA |

## USDC Contract

| Parameter | Value |
|-----------|-------|
| **Address** | `0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4` |
| **Decimals** | 6 |
| **EIP-3009** | Supported |
| **EIP-712 Name** | `USD Coin` |
| **EIP-712 Version** | `2` |

## Facilitator Wallet

| Environment | Address | Funded |
|-------------|---------|--------|
| Mainnet | `0x103040545AC5031A11E8C03dd11324C7333a13C7` | Yes |

### Funding Transaction

Bridge from Ethereum mainnet:
- **Etherscan:** [0x339360876b56dc22e73551f1f4a96b439f7afbfc80329d0cb0f45daeb71402a7](https://etherscan.io/tx/0x339360876b56dc22e73551f1f4a96b439f7afbfc80329d0cb0f45daeb71402a7)

## Files Modified

| File | Changes |
|------|---------|
| `src/network.rs` | Added `Scroll` enum variant, USDC deployment, CAIP-2 mapping |
| `src/from_env.rs` | Added `ENV_RPC_SCROLL` constant |
| `src/chain/evm.rs` | Added chain ID 534352, EIP-1559 = true |
| `src/chain/solana.rs` | Added UnsupportedNetwork exclusion |
| `src/handlers.rs` | Added `get_scroll_logo()` handler and route |
| `static/index.html` | Added CSS, mainnet card, balance config, TOKEN_SUPPORT |
| `static/scroll.png` | Scroll logo (108KB) |
| `.env.example` | Added `RPC_URL_SCROLL` |
| `README.md` | Updated network count to 19, added Scroll to table |

## Implementation Notes

### Why No Testnet?

Circle has not deployed official USDC on Scroll Sepolia. The contract found at `0x4d7ff95a5e86b0aaade01df5adadded72c54a698` appears to be a wrapper/bridged token that doesn't have proper EIP-3009 support (missing `version()` function).

### EIP-3009 Verification

Scroll mainnet USDC was verified using:
```bash
# Check transferWithAuthorization exists
cast call 0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4 \
  "TRANSFER_WITH_AUTHORIZATION_TYPEHASH()" \
  --rpc-url https://rpc.scroll.io
```

### zkEVM Compatibility

Scroll is a zkEVM L2, which means:
- Full EVM bytecode compatibility
- Standard EIP-1559 transaction support
- No special transaction handling required
- Uses ETH for gas (same as Ethereum)

## Deployment Checklist

- [x] Added Network enum variant
- [x] Added USDC deployment constant
- [x] Added chain ID mapping
- [x] Added EIP-1559 support flag (true)
- [x] Added Solana exclusion
- [x] Added RPC environment variable
- [x] Added logo handler
- [x] Added landing page integration (mainnet only)
- [x] Updated README network count
- [x] Funded mainnet wallet
- [ ] Deploy to production
- [ ] Verify in /supported endpoint

## Testing

### Local Verification
```bash
# Build
cargo build --release

# Verify Scroll appears in supported networks
cargo run --release &
curl -s http://localhost:8080/supported | jq '.kinds[].network' | grep scroll
```

### Production Verification
```bash
# After deployment
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network' | grep scroll
```

## Related Resources

- [Scroll Documentation](https://docs.scroll.io/)
- [Scroll Bridge](https://scroll.io/bridge)
- [Circle USDC Docs](https://developers.circle.com/stablecoins/usdc-contract-addresses)
- [Scrollscan Explorer](https://scrollscan.com)
