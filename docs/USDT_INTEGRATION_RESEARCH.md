# USDT Integration Research Report

**Date:** 2025-12-21
**Status:** Research Complete
**Author:** Claude (Automated Research)

## Executive Summary

USDT (Tether) has historically NOT supported EIP-3009 `transferWithAuthorization`, which is required for the x402 protocol. However, Tether launched **USDT0** in January 2025, an upgraded omnichain version using LayerZero's OFT standard that **does implement EIP-3009**.

**Key Finding:** We can integrate USDT on **3 networks** where EIP-3009 is now supported:
- Arbitrum (upgraded to USDT0)
- Celo (has EIP-3009)
- Optimism (new USDT0 deployment)

## Why USDT Wasn't Supported Before

The original USDT contract on Ethereum (deployed 2017) is an older implementation that predates EIP-3009 (proposed 2020). Unlike Circle's USDC which was designed with meta-transactions in mind, Tether's original contracts only implement basic ERC-20 functionality.

**Original USDT Limitations:**
- No `transferWithAuthorization` function
- No `receiveWithAuthorization` function
- No EIP-2612 `permit` support
- Cannot be used for gasless transfers

## USDT0: The Game Changer

In January 2025, Tether launched USDT0 - a new cross-chain stablecoin using LayerZero's Omnichain Fungible Token (OFT) standard.

### USDT0 Features
- Full EIP-3009 support (`transferWithAuthorization`, `receiveWithAuthorization`)
- Full EIP-2612 support (`permit`)
- Cross-chain interoperability via LayerZero
- 1:1 backing with existing USDT reserves
- Audited by OpenZeppelin, Guardian Audits, and ChainSecurity

### USDT0 Timeline
- **January 16, 2025:** USDT0 launched on Ink (Kraken's L2)
- **January 29, 2025:** Arbitrum migration began
- **February 5, 2025:** Full interoperability across ETH, Arbitrum, Ink
- **July 2025:** USDT0 launched on Celo
- **Current:** Available on 12+ chains

## Verified EIP-3009 Support by Network

### Networks WITH EIP-3009 Support (Can Integrate)

| Network | Contract Address | Token Name | Decimals | Verified |
|---------|-----------------|------------|----------|----------|
| **Arbitrum** | `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` | USD₮0 | 6 | Yes |
| **Celo** | `0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e` | Tether USD | 6 | Yes |
| **Optimism** | `0x01bff41798a0bcf287b996046ca68b395dbc1071` | USD₮0 | 6 | Yes |

**Verification Method:** Called `transferWithAuthorization` with dummy parameters. Contracts that support EIP-3009 return "ECRecover: invalid signature" error (function exists, just needs valid sig). Contracts without EIP-3009 return generic "execution reverted" (function doesn't exist).

### Networks WITHOUT EIP-3009 Support (Cannot Integrate Yet)

| Network | Contract Address | Reason |
|---------|-----------------|--------|
| **Ethereum** | `0xdAC17F958D2ee523a2206206994597C13D831ec7` | Original 2017 contract, no upgrade |
| **Base** | `0xfde4C96c8593536E31F229EA8f37b2ADa2699bb2` | Not upgraded to USDT0 |
| **Polygon** | `0xc2132D05D31c914a87C6611C10748AEb04B58e8F` | Has DOMAIN_SEPARATOR but no transferWithAuthorization |
| **Avalanche** | `0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7` | Not upgraded |

### Networks Pending USDT0 Deployment

According to Tether's roadmap, USDT0 will expand to:
- Berachain
- MegaETH
- Flare
- Plasma
- HyperEVM
- SEI
- Rootstock

## Technical Verification Results

### Arbitrum (CONFIRMED EIP-3009)
```
Contract: 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9
Name: "USD₮0"
Decimals: 6
DOMAIN_SEPARATOR: 0x566af68fb471b22d6421762f84aa7bd761c670a2e4d5c8a47d4085d5957b127c
transferWithAuthorization: EXISTS (returns "ECRecover: invalid signature")
```

### Celo (CONFIRMED EIP-3009)
```
Contract: 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e
Name: "Tether USD"
Decimals: 6
DOMAIN_SEPARATOR: EXISTS
transferWithAuthorization: EXISTS (returns "ECRecover: invalid signature")
```

### Optimism USDT0 (CONFIRMED EIP-3009)
```
Contract: 0x01bff41798a0bcf287b996046ca68b395dbc1071
Name: "USD₮0"
Decimals: 6
transferWithAuthorization: EXISTS (returns "ECRecover: invalid signature")

NOTE: This is a NEW contract address, different from the old USDT on Optimism!
Old USDT (0x94b008aA00579c1307B0EF2c499aD98a8ce58e58) does NOT support EIP-3009.
```

### Ethereum Mainnet (NO EIP-3009)
```
Contract: 0xdAC17F958D2ee523a2206206994597C13D831ec7
Name: "Tether USD"
Decimals: 6
transferWithAuthorization: DOES NOT EXIST (generic revert)
```

## Integration Recommendation

### Phase 1: Immediate Integration (Ready Now)

Add USDT support on networks where we already support other stablecoins:

1. **Arbitrum** - We already support USDC on Arbitrum
   - Contract: `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9`
   - EIP-712 Name: "USD₮0" (needs verification)
   - EIP-712 Version: TBD (needs verification)

2. **Celo** - We already support USDC on Celo
   - Contract: `0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e`
   - EIP-712 Name: "Tether USD" (needs verification)
   - EIP-712 Version: TBD (needs verification)

3. **Optimism** - We already support USDC on Optimism
   - Contract: `0x01bff41798a0bcf287b996046ca68b395dbc1071` (NEW ADDRESS!)
   - EIP-712 Name: "USD₮0" (needs verification)
   - EIP-712 Version: TBD (needs verification)
   - **WARNING:** Old USDT address does NOT support EIP-3009

### Phase 2: Future Integration (Monitor)

Watch for USDT0 deployment on:
- Polygon (currently has DOMAIN_SEPARATOR, may be upgraded soon)
- Base
- Avalanche
- HyperEVM (mentioned in USDT0 roadmap)

### Not Recommended

- **Ethereum Mainnet USDT** - No EIP-3009 support, unlikely to change due to contract immutability

## Implementation Checklist

For each network integration:

- [ ] Verify EIP-712 domain name (call contract or check source)
- [ ] Verify EIP-712 version (call contract or check source)
- [ ] Add `USDTDeployment` struct to `src/network.rs`
- [ ] Add `TokenType::Usdt` variant to `src/types.rs`
- [ ] Update `get_token_deployment()` function
- [ ] Update `supported_networks_for_token()` function
- [ ] Add USDT to `find_known_eip712_metadata()` in `src/chain/evm.rs`
- [ ] Update frontend TOKEN_SUPPORT and TOKEN_INFO
- [ ] Add USDT logo to `static/` directory
- [ ] Test with real transaction on testnet (if available)

## EIP-712 Domain Research Needed

Before implementation, we need to verify the exact EIP-712 domain parameters for each USDT contract:

```solidity
// Expected structure
EIP712Domain {
    string name,      // "USD₮0" or "Tether USD"?
    string version,   // "1" or "2"?
    uint256 chainId,
    address verifyingContract
}
```

**Action Item:** Query each contract's `eip712Domain()` function or check verified source code on block explorers.

## Risks and Considerations

1. **Contract Upgrades:** USDT0 uses upgradeable proxy pattern - behavior could change
2. **Different Addresses:** Optimism USDT0 has a NEW address, users may have funds on old USDT
3. **Name Variations:** Some contracts use "USD₮0", others use "Tether USD"
4. **Liquidity:** USDT0 is newer, may have less liquidity than original USDT on some chains

## Sources

- [USDT0 Official Documentation](https://docs.usdt0.to/technical-documentation/developer)
- [OpenZeppelin USDT0 Audit](https://www.openzeppelin.com/news/usdt0-audit)
- [USDT0 Arbitrum Launch Announcement](https://mirror.xyz/tetherzero.eth/_6FNgGi0WHHQhA9qavZ4rlt-nV9ehVuJUHxQnSwOmbM)
- [EIP-3009 Specification](https://eips.ethereum.org/EIPS/eip-3009)
- [Tether-Arbitrum Partnership](https://cointelegraph.com/news/tether-arbitrum-interoperable-stablecoin)
- [USDT0 on Celo Announcement](https://blockchainreporter.net/usdt0-stablecoin-now-available-on-celo-powering-interoperability-and-removing-obstacles/)

## Conclusion

USDT integration is now viable on 3 networks (Arbitrum, Celo, Optimism) thanks to the USDT0 upgrade. This would make us one of the few x402 facilitators supporting USDT, significantly expanding our stablecoin coverage to include the world's largest stablecoin by market cap.

**Recommended Priority:**
1. Arbitrum USDT0 (highest volume)
2. Optimism USDT0 (growing ecosystem)
3. Celo USDT (regional importance)
