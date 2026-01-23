# Known Networks Reference

This reference contains verified information about blockchain networks commonly requested for x402-rs facilitator integration.

## Quick Lookup Table

| Network | Mainnet Chain ID | Testnet Chain ID | Native Token | EIP-1559 |
|---------|------------------|------------------|--------------|----------|
| Scroll | 534352 | 534351 | ETH | Yes |
| Linea | 59144 | 59141 | ETH | Yes |
| Blast | 81457 | 168587773 | ETH | Yes |
| zkSync Era | 324 | 300 | ETH | No (zkSync native) |
| Mantle | 5000 | 5003 | MNT | Yes |
| Mode | 34443 | 919 | ETH | Yes |
| Taiko | 167000 | 167009 | ETH | Yes |
| Abstract | 2741 | 11124 | ETH | Yes |
| Ink | 57073 | 763373 | ETH | Yes |
| World Chain | 480 | 4801 | ETH | Yes |
| Soneium | 1868 | 1946 | ETH | Yes |
| Berachain | 80094 | 80069 | BERA | Yes |
| Monad | TBD | TBD | MON | TBD |

---

## Detailed Network Information

### Scroll

**Type:** zkEVM L2 on Ethereum

**Chain IDs:**
- Mainnet: 534352
- Sepolia Testnet: 534351

**RPCs:**
- Mainnet: `https://rpc.scroll.io`
- Testnet: `https://sepolia-rpc.scroll.io`

**Explorers:**
- Mainnet: https://scrollscan.com
- Testnet: https://sepolia.scrollscan.com

**USDC (Circle Native):**
- Mainnet: `0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4`
- Testnet: Check Circle faucet
- EIP-712 Name: `"USD Coin"`
- EIP-712 Version: `"2"`
- EIP-3009: YES

**Brand Color:** #FFEEDA (cream/beige)

**Notes:**
- Uses standard ETH for gas
- EIP-1559 supported
- Circle has native USDC deployment

---

### Linea

**Type:** zkEVM L2 by Consensys

**Chain IDs:**
- Mainnet: 59144
- Sepolia Testnet: 59141

**RPCs:**
- Mainnet: `https://rpc.linea.build`
- Testnet: `https://rpc.sepolia.linea.build`

**Explorers:**
- Mainnet: https://lineascan.build
- Testnet: https://sepolia.lineascan.build

**USDC:**
- Mainnet: `0x176211869cA2b568f2A7D4EE941E073a821EE1ff` (Bridged USDC.e)
- Native USDC coming - check Circle announcements
- EIP-712 Name: Verify from contract
- EIP-3009: Verify before implementing

**Brand Color:** #61DFFF (cyan blue)

**Notes:**
- Bridged USDC may not have EIP-3009
- Wait for Circle native USDC if possible

---

### Blast

**Type:** Optimistic L2 with native yield

**Chain IDs:**
- Mainnet: 81457
- Sepolia Testnet: 168587773

**RPCs:**
- Mainnet: `https://rpc.blast.io`
- Testnet: `https://sepolia.blast.io`

**Explorers:**
- Mainnet: https://blastscan.io
- Testnet: https://sepolia.blastscan.io

**USDB (Native Stablecoin):**
- Mainnet: `0x4300000000000000000000000000000000000003`
- Note: Blast uses USDB, not USDC
- EIP-3009: Verify before implementing

**USDC (if available):**
- Check for bridged or native USDC

**Brand Color:** #FCFC03 (yellow)

**Notes:**
- Native yield on ETH and USDB
- May need special handling for USDB vs USDC

---

### zkSync Era

**Type:** zkRollup L2 by Matter Labs

**Chain IDs:**
- Mainnet: 324
- Sepolia Testnet: 300

**RPCs:**
- Mainnet: `https://mainnet.era.zksync.io`
- Testnet: `https://sepolia.era.zksync.dev`

**Explorers:**
- Mainnet: https://explorer.zksync.io
- Testnet: https://sepolia.explorer.zksync.io

**USDC (Circle Native):**
- Mainnet: `0x1d17CBcF0D6D143135aE902365D2E5e2A16538D4`
- Testnet: `0x0faF6df7054946141266420b43783387A78d82A9`
- EIP-712: Verify from contract
- EIP-3009: YES (Circle native)

**Brand Color:** #8C8DFC (purple)

**Notes:**
- Uses native account abstraction
- Transaction format differs from standard EVM
- May need special gas estimation
- EIP-1559: NO (uses zkSync native format)

---

### Mantle

**Type:** Optimistic L2 with EigenDA

**Chain IDs:**
- Mainnet: 5000
- Sepolia Testnet: 5003

**RPCs:**
- Mainnet: `https://rpc.mantle.xyz`
- Testnet: `https://rpc.sepolia.mantle.xyz`

**USDC:**
- Mainnet: `0x09Bc4E0D864854c6aFB6eB9A9cdF58aC190D0dF9`
- EIP-3009: Verify before implementing

**Native Token:** MNT (not ETH!)

**Brand Color:** #000000 (black)

**Notes:**
- Uses MNT for gas, not ETH
- Update native token name in frontend

---

### Mode

**Type:** Optimistic L2 on Optimism stack

**Chain IDs:**
- Mainnet: 34443
- Sepolia Testnet: 919

**RPCs:**
- Mainnet: `https://mainnet.mode.network`
- Testnet: `https://sepolia.mode.network`

**USDC:**
- Mainnet: `0xd988097fb8612cc24eeC14542bC03424c656005f`
- EIP-3009: Verify

**Brand Color:** #DFFE00 (lime green)

---

### Taiko

**Type:** Based rollup (decentralized sequencing)

**Chain IDs:**
- Mainnet: 167000
- Hekla Testnet: 167009

**RPCs:**
- Mainnet: `https://rpc.mainnet.taiko.xyz`
- Testnet: `https://rpc.hekla.taiko.xyz`

**USDC:**
- Mainnet: `0x07d83526730c7438048D55A4fc0b850e2aaB6f0b`
- EIP-3009: Verify

**Brand Color:** #E81899 (magenta/pink)

---

## Networks with Special Considerations

### zkSync Era
- Does NOT use EIP-1559
- Has native account abstraction
- Transaction encoding differs

### Mantle
- Uses MNT for gas (not ETH)
- Update `native_token` in frontend

### SKALE (Already Integrated)
- Does NOT use EIP-1559 (`eip1559: false`)
- Uses CREDIT/sFUEL for gas (free)
- L3 architecture

### Fogo (Already Integrated)
- Solana-based chain
- Different transaction format
- Separate wallet keys

---

## Circle USDC Deployment Status

Check https://developers.circle.com/stablecoins/docs/usdc-on-main-networks for latest.

| Network | Native USDC | Status |
|---------|-------------|--------|
| Scroll | Yes | Deployed |
| Linea | Coming | Bridged only currently |
| Blast | No (USDB) | N/A |
| zkSync | Yes | Deployed |
| Mantle | ? | Verify |
| Mode | ? | Verify |
| Taiko | ? | Verify |

---

## Faucets for Testnet Funding

| Network | Faucet URL |
|---------|------------|
| Scroll Sepolia | https://scroll.io/bridge (bridge from Sepolia) |
| Linea Sepolia | https://faucet.goerli.linea.build |
| Blast Sepolia | https://blastfaucet.com |
| zkSync Sepolia | https://portal.zksync.io/faucet |
| Mantle Sepolia | https://faucet.sepolia.mantle.xyz |
| Mode Sepolia | https://faucet.mode.network |

---

## Implementation Priority

Based on TVL, user demand, and EIP-3009 availability:

1. **High Priority (Native USDC confirmed):**
   - Scroll
   - zkSync Era

2. **Medium Priority (Verify EIP-3009):**
   - Linea (wait for native USDC)
   - Mode
   - Taiko

3. **Lower Priority (Special handling needed):**
   - Blast (USDB vs USDC)
   - Mantle (MNT gas token)

4. **Future (TBD):**
   - Abstract
   - Ink
   - World Chain
   - Soneium
   - Berachain
   - Monad
