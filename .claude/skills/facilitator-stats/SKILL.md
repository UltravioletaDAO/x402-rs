---
name: facilitator-stats
description: Generate on-chain statistics report for the x402 facilitator across all 19+ supported mainnet blockchains. This skill should be used when the user asks for facilitator stats, on-chain stats, blockchain analysis, transaction volume, settlement counts, or says "dame las stats", "estadisticas on-chain", "give me stats", "/facilitator-stats", "how many transactions", "cuantas transacciones". It queries explorer APIs and RPCs across EVM, Solana, SUI, NEAR, Stellar, and Algorand chains to produce a comprehensive cross-chain settlement report.
---

# Facilitator On-Chain Statistics Report

Generate a comprehensive on-chain statistics report for the x402 payment facilitator across all supported mainnet blockchains.

## Step 1: Read Wallet Addresses

**CRITICAL: NEVER hardcode or type wallet addresses from memory. ALWAYS read from source files.**

Read wallet addresses from `lambda/balances/handler.py`:

```python
# Extract these variables at runtime:
MAINNET_ADDRESS        # EVM mainnet facilitator wallet
SOLANA_MAINNET_ADDRESS # Solana + Fogo wallet (SVM chains share a keypair)
SUI_MAINNET_ADDRESS    # SUI wallet
NEAR_MAINNET_ADDRESS   # NEAR wallet
STELLAR_MAINNET_ADDRESS # Stellar wallet
ALGORAND_MAINNET_ADDRESS # Algorand wallet
```

Also read `config/supported_tokens.json` for chain IDs, explorer URLs, and token contract addresses.

## Step 2: Deploy 5 Parallel Agents

Launch all 5 agents in background using the Agent tool. Each agent receives the wallet addresses read in Step 1.

### Agent 1: Priority EVM (Base + SKALE Base)

**Base (Chain ID 8453)** - Typically the most active chain:
- Basescan API: `api.basescan.org/api?module=account&action=tokentx&address={WALLET}&sort=asc`
- Also: `action=txlist` for total tx count including escrow/ERC-8004 operations

**SKALE Base (Chain ID 1187947933):**
- Blockscout: `skale-base.explorer.skalenodes.com/api/v2/addresses/{WALLET}/transactions`
- Note: SKALE escrow may be blocked on Cancun EVM support (TSTORE/TLOAD)

### Agent 2: Core EVM (Ethereum + Arbitrum + Polygon)

Etherscan-compatible API pattern for each chain:
```
curl -s "https://{EXPLORER}/api?module=account&action=tokentx&address={WALLET}&sort=asc"
curl -s "https://{EXPLORER}/api?module=account&action=txlist&address={WALLET}&sort=asc"
```

If free-tier API fails, fallback to:
1. RPC nonce check: `eth_getTransactionCount` (confirms wallet activity)
2. Blockscout instances (e.g., `arbitrum.blockscout.com/api/v2/addresses/{WALLET}/transactions`)

Explorer APIs: `api.etherscan.io`, `api.arbiscan.io`, `api.polygonscan.com`

### Agent 3: Secondary EVM (Optimism + Avalanche + Celo + Unichain + Scroll)

Same Etherscan-compatible methodology. Explorers:
- Optimism: `api-optimistic.etherscan.io/api`
- Avalanche: `api.routescan.io/v2/network/mainnet/evm/43114/etherscan/api`
- Celo: `api.celoscan.io/api`
- Unichain: `unichain.blockscout.com/api/v2/addresses/{WALLET}/transactions`
- Scroll: `scroll.blockscout.com/api/v2/addresses/{WALLET}/transactions`

### Agent 4: Newer EVM (BSC + HyperEVM + Monad)

- **BSC**: `api.bscscan.com/api` — Note: BSC USDC uses **18 decimals** (not 6)
- **HyperEVM**: No public explorer API. Check nonce via `rpc.hyperliquid.xyz/evm`. If nonce equals a value at block 0, it is genesis state (not real txs).
- **Monad**: RPC `rpc.monad.xyz` prunes historical state. Check nonce for count, note that volume is inaccessible without MonadScan API key.

### Agent 5: Non-EVM (Solana + Fogo + SUI + NEAR + Stellar + Algorand)

**Solana** (RPC: `api.mainnet-beta.solana.com`):
```bash
# Get signature count (paginate with 'before' param if >1000)
curl -s -X POST RPC -d '{"jsonrpc":"2.0","id":1,"method":"getSignaturesForAddress","params":["{WALLET}",{"limit":1000}]}'
```
- Public RPC is non-archival and rate-limited
- Use **statistical sampling** (~100 evenly distributed txs) to estimate USDC rate and volume
- Note: Many Solana txs may be micro-test payments ($0.0001)

**Fogo** (SVM-based, same wallet as Solana):
- RPC: `rpc.fogo.nightly.app`
- Same `getSignaturesForAddress` method

**SUI**:
```bash
curl -s -X POST "https://fullnode.mainnet.sui.io:443" -d \
  '{"jsonrpc":"2.0","id":1,"method":"suix_queryTransactionBlocks","params":[{"filter":{"FromAddress":"{WALLET}"},"options":{"showEffects":true}},null,100,false]}'
```

**NEAR**:
```bash
curl -s "https://api.nearblocks.io/v1/account/{WALLET}/ft-txns?per_page=100&order=asc"
```

**Stellar**:
```bash
curl -s "https://horizon.stellar.org/accounts/{WALLET}/payments?limit=200&order=asc"
```

**Algorand**:
```bash
# USDC ASA ID: 31566704
curl -s "https://mainnet-idx.algonode.cloud/v2/accounts/{WALLET}/transactions?asset-id=31566704&limit=100"
# Also check all txs for funding/opt-in activity
curl -s "https://mainnet-idx.algonode.cloud/v2/accounts/{WALLET}/transactions?limit=100"
```

## Step 3: Data Extraction Per Chain

For each chain, extract:

| Metric | Description |
|--------|-------------|
| Total txs | All facilitator transactions (nonce for EVM, sig count for Solana) |
| Settlements | USDC/stablecoin payment settlements only (transferWithAuthorization calls) |
| Volume | Sum of settlement amounts in USD (divide by token decimals) |
| Unique payers | Distinct addresses initiating payments |
| First tx date | Earliest transaction timestamp |
| Last tx date | Most recent transaction timestamp |
| Gas balance | Current native token balance |

## Step 4: Classify Transactions

The facilitator performs multiple types of on-chain operations. Distinguish:

- **Settlements**: `transferWithAuthorization` (selector `0xcf092995`) on stablecoin contracts - these are actual x402 payments
- **Escrow operations**: `authorize`, `settle` on PaymentOperator contracts - x402r escrow
- **ERC-8004 registrations**: `register`, `safeTransferFrom` on IdentityRegistry contracts
- **Gas funding**: Incoming native token transfers to the facilitator
- **Spam**: Unsolicited token airdrops (exclude entirely)

Report settlements separately from total tx count.

## Step 5: Compile Report

### Table 1: Per-Chain Breakdown (sorted by settlement count)

```markdown
| # | Chain | Txs Total | Settlements | Volume (USD) | Payers | First Tx | Last Tx | Gas Balance |
|---|-------|-----------|-------------|--------------|--------|----------|---------|-------------|
```

### Table 2: Cross-Chain Totals

```markdown
| Metric | Value |
|--------|-------|
| Total settlements | X |
| Total volume | $X |
| Active chains (with settlements) | X of 19 |
| Unique payers | ~X |
| Period | YYYY-MM-DD to YYYY-MM-DD |
```

### Table 3: Volume Distribution

Top chains by percentage of total volume.

### Notes Section

Include:
- Chains where data was partially or fully inaccessible (with reason)
- Methodology notes for statistical estimates (Solana sampling)
- Chains provisioned with gas but zero settlements
- Any anomalies (e.g., genesis-state nonces on HyperEVM)

## Known Limitations

- **Monad**: Public RPC prunes historical state. Nonce gives tx count but volume requires MonadScan API key.
- **HyperEVM**: No public explorer API. Nonce may include genesis-state values.
- **Solana**: Public RPC rate-limits `getTransaction`. Use sampling methodology for volume estimates.
- **BSC**: USDC uses 18 decimals (not 6 like other chains). Only AUSD supports gasless payments.
- **Explorer APIs**: Free tiers have rate limits (typically 5 calls/sec). Space requests accordingly.
