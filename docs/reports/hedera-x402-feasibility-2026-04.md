# Hedera x402 Feasibility Report — April 2026

**Date**: April 4, 2026
**Purpose**: Determine if x402 EIP-3009 payments are now feasible on Hedera
**Previous research**: `docs/reports/hedera-integration-analysis.md` (pre-deployment)

---

## Executive Summary

**USDC on Hedera does NOT support EIP-3009** — it is an HTS (Hedera Token Service) native token, not a Solidity ERC-20 contract. However, **USDT0 (Tether Omnichain) IS deployed as a real EVM ERC-20 on Hedera with full EIP-3009 support**. This is the only stablecoin on Hedera that could work with the facilitator's existing `scheme_exact_evm` flow.

| Stablecoin | On Hedera? | Type | EIP-3009? | EIP-2612? | x402 Viable? |
|------------|-----------|------|-----------|-----------|--------------|
| **USDC** | Yes | HTS native | NO | NO | **NO** |
| **USDT0** | Yes | **EVM ERC-20 (OFT)** | **YES** | **YES** | **YES** |
| **USDT[HTS]** | Yes | HTS native (bridged) | NO | NO | NO |
| **EURC** | No | — | — | — | NO |
| **PYUSD** | No | — | — | — | NO |
| **AUSD** | No | — | — | — | NO |
| **FRNT** (Wyoming) | Yes | LayerZero OFT | Unknown | Unknown | Needs verification |

---

## 1. USDC on Hedera — BLOCKED

Circle deployed USDC on Hedera as a native HTS token (November 2021), **not** by deploying their standard `FiatTokenV2_2.sol` contract.

| Property | Value |
|----------|-------|
| Mainnet Token ID | `0.0.456858` |
| Mainnet EVM Address | `0x000000000000000000000000000000000006f89a` (auto-derived facade) |
| Testnet Token ID | `0.0.429274` |
| Testnet EVM Address | `0x0000000000000000000000000000000000068cda` |
| Supply | ~$56.2M |
| EIP-3009 | **NOT AVAILABLE** |
| EIP-2612 | **NOT AVAILABLE** |
| CCTP | **NOT SUPPORTED** on Hedera |

The HTS ERC-20 facade (HIP-218/HIP-376) only exposes basic functions: `transfer()`, `approve()`, `transferFrom()`, `balanceOf()`. No `transferWithAuthorization()`, no `DOMAIN_SEPARATOR()`, no `permit()`.

**Verdict: USDC x402 on Hedera is not possible.**

### Sources
- [Circle: USDC on Hedera](https://www.circle.com/multi-chain-usdc/hedera)
- [Circle: USDC Contract Addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)
- [Circle: CCTP Supported Blockchains](https://developers.circle.com/cctp/cctp-supported-blockchains)

---

## 2. USDT0 on Hedera — THE OPPORTUNITY

**Launched March 12, 2026.** USDT0 is Tether's omnichain deployment via LayerZero OFT (Omnichain Fungible Token) standard. Unlike USDC, it is deployed as a **real Solidity ERC-20 smart contract** on Hedera's EVM.

| Property | Value |
|----------|-------|
| Hedera EVM Address | `0xe3119e23fC2371d1E6b01775ba312035425A53d6` |
| Standard | LayerZero OFT (ERC-20) |
| `transferWithAuthorization()` | **YES** — confirmed in OpenZeppelin audit |
| `receiveWithAuthorization()` | **YES** |
| `permit()` (EIP-2612) | **YES** (ERC20Permit) |
| `DOMAIN_SEPARATOR()` | **YES** |
| Audited by | OpenZeppelin |
| Audit repo | `Everdawn-Labs/usdt0-tether-contracts-hardhat` |

### What this means for x402

USDT0 on Hedera could work with our existing `scheme_exact_evm` flow because:
1. It's a standard ERC-20 contract on the EVM (not an HTS token)
2. It implements full EIP-3009 (`transferWithAuthorization`)
3. It has EIP-712 domain separators
4. The facilitator would pay gas in HBAR (same as ERC-8004 transactions)

### What we'd need to implement

1. Add a `USDT0_HEDERA` deployment constant in `src/network.rs` with:
   - Contract address: `0xe3119e23fC2371d1E6b01775ba312035425A53d6`
   - EIP-712 domain `name` and `version` (must be read from contract on-chain)
2. Add USDT0 as a new `TokenType` variant (or extend USDT support to include USDT0)
3. Verify the EIP-712 domain separator values by calling the contract
4. Fund the facilitator mainnet wallet with HBAR for gas

### Open questions

- What are the EIP-712 `name` and `version` values? (Need to call `name()` and `EIP712_VERSION()` on the contract)
- Is the USDT0 contract upgradeable? (Likely yes, given Tether's practices)
- What is the current USDT0 liquidity on Hedera?
- Does the facilitator need to handle any LayerZero-specific behavior?

### Sources
- [USDT0 Deployments](https://docs.usdt0.to/technical-documentation/deployments)
- [USDT0 Developer Guide](https://docs.usdt0.to/technical-documentation/developer/)
- [OpenZeppelin USDT0 Audit](https://www.openzeppelin.com/news/usdt0-audit)
- [Hedera integrates USDT0](https://hedera.com/blog/hedera-integrates-usdt0-for-crosschain-stablecoin-liquidity/)

---

## 3. Other Stablecoins — NOT ON HEDERA

| Stablecoin | Deployed on | Hedera? |
|------------|-------------|---------|
| **EURC** (Circle) | Ethereum, Solana, Base, Avalanche, Stellar, World Chain | NO |
| **PYUSD** (PayPal/Paxos) | Ethereum, Solana, Arbitrum | NO |
| **AUSD** (Agora) | Ethereum, Avalanche, Sui, Mantle, Monad | NO |
| **USDT** (HTS bridged) | Hedera (via Hashport) | Yes, but HTS native, no EIP-3009, ~280K supply, bridge dying |

**Note:** There is an **AUDD** (Australian Digital Dollar) on Hedera — it is AUD-denominated, built with Hedera Stablecoin Studio, completely unrelated to Agora's AUSD.

**FRNT (Wyoming Frontier Stable Token)** launched on Hedera March 12, 2026 via LayerZero OFT. Backed 102% by US Treasuries. Unknown if it implements EIP-3009 — would need verification.

---

## 4. Hedera EVM Compatibility

Hedera runs a **Cancun-equivalent EVM** (Besu engine) as of mainnet v0.50.0:

| Feature | Status |
|---------|--------|
| EVM version | Cancun (TSTORE, TLOAD, MCOPY) |
| EIP-712 | **Supported** (via JSON-RPC relay `eth_signTypedData_v4`) |
| ecrecover | **Works** (ECDSA secp256k1 fully compatible) |
| Gas model | Fixed USD pricing, paid in HBAR |
| Smart contracts | Full Solidity support (Besu v25.2.2) |
| Current mainnet | v0.71.3 (deployed March 18, 2026) |
| Coming: v0.72 | Hiero Hooks (HIP-1195), Jumbo Ethereum Txs (128KB callData) |
| Pectra evaluation | HIP-1341 in progress |

**Key takeaway:** The EVM itself is fully compatible with EIP-3009. The blocker is that USDC specifically is NOT an EVM contract on Hedera. But contracts like USDT0 that ARE deployed as EVM contracts work perfectly.

### Sources
- [Hedera EVM Differences](https://docs.hedera.com/hedera/core-concepts/smart-contracts/understanding-hederas-evm-differences-and-compatibility)
- [HIP-865: Cancun Transient Storage](https://hips.hedera.com/hip/hip-865)
- [Consensus Node Release Notes](https://docs.hedera.com/hedera/networks/release-notes/services)

---

## 5. Ecosystem Context (Jan–Apr 2026)

### DeFi
- **SaucerSwap**: $77.6M TVL, dominant DEX
- **Bonzo Finance**: Leading lending protocol
- Total DeFi TVL: $208M (+141% growth)

### Infrastructure Changes
- **Hashport bridge shutting down May 31, 2026** — users must unport assets immediately
- **Axelar Network**: Replacement bridge, connects Hedera to 60+ chains
- **LayerZero**: Active, powering USDT0 and FRNT cross-chain

### Enterprise & Grants
- **HBAR Foundation consolidating** — core biz dev moving to Hashgraph entity
- **HEAT program** launched: 4 verticals including payments/stablecoins
- **HederaCon**: May 4, 2026, Miami Beach — stablecoins/compliance as key theme
- 4.86 billion HBAR allocated for further development

### x402 on Hedera (BlockyDevs)
- BlockyDevs built **Blocky402** — their own x402 facilitator supporting Hedera testnet
- Uses Hedera-native partially-signed HTS transactions (NOT EIP-3009)
- Published February 10, 2026 on the Hedera blog
- TheGreatAxios built an **EIP-3009 Forwarder** contract as a workaround wrapper

---

## 6. Recommendations

### Short-term (Low effort, high value)
**Add USDT0 support on Hedera.** This is the path of least resistance:
- Contract is already deployed with EIP-3009
- Uses the same `scheme_exact_evm` flow as all our EVM chains
- Requires: new token deployment constant, EIP-712 domain verification, HBAR funding (already done for ERC-8004)

### Medium-term (If demand exists)
**Monitor FRNT (Wyoming Stable Token).** If it also implements EIP-3009 as a LayerZero OFT, it could be a second supported stablecoin on Hedera. Needs verification.

### Not recommended
- Waiting for Circle to deploy FiatTokenV2 on Hedera (unlikely, no public plans)
- Building a Hedera-native payment scheme (high effort, non-standard)
- Using the EIP-3009 Forwarder pattern (requires user pre-approval, breaks gasless UX)

---

## 7. Comparison with Original Research

| Finding | Original (pre-deploy) | Updated (April 2026) |
|---------|----------------------|---------------------|
| USDC EIP-3009 | Not available | **Still not available** |
| USDT0 EIP-3009 | Not researched | **AVAILABLE — new finding** |
| EURC on Hedera | Not deployed | Still not deployed |
| PYUSD on Hedera | Not deployed | Still not deployed |
| Hedera EVM level | Cancun-equivalent | Confirmed Cancun (v0.71.3) |
| Hashport bridge | Active | **Shutting down May 31, 2026** |
| x402 feasibility | ERC-8004 only | **ERC-8004 + USDT0 payments possible** |

---

## Action Items

- [ ] Read USDT0 EIP-712 domain name/version from contract on Hedera mainnet
- [ ] Verify USDT0 `transferWithAuthorization` function signature matches our implementation
- [ ] Check USDT0 liquidity on Hedera (SaucerSwap, DEX aggregators)
- [ ] Decide: add USDT0 as variant of existing USDT `TokenType`, or create new `Usdt0` type
- [ ] Fund facilitator mainnet wallet with HBAR (in progress)
- [ ] Verify FRNT EIP-3009 support (secondary priority)
