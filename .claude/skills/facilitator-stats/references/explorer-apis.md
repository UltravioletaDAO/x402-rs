# Explorer API Reference

## EVM Chain Explorers (Etherscan-compatible)

| Chain | API Base URL | Chain ID | Notes |
|-------|-------------|----------|-------|
| Ethereum | `api.etherscan.io/api` | 1 | Free tier available |
| Base | `api.basescan.org/api` | 8453 | Free tier available |
| Polygon | `api.polygonscan.com/api` | 137 | Free tier available |
| Arbitrum | `api.arbiscan.io/api` | 42161 | V1 deprecated, try Blockscout |
| Optimism | `api-optimistic.etherscan.io/api` | 10 | Free tier available |
| Avalanche | `api.routescan.io/v2/network/mainnet/evm/43114/etherscan/api` | 43114 | Routescan proxy |
| Celo | `api.celoscan.io/api` | 42220 | Free tier available |
| BSC | `api.bscscan.com/api` | 56 | Free tier available |
| Scroll | `api.scrollscan.com/api` | 534352 | May need API key |

## Blockscout Instances (fallback)

| Chain | Blockscout URL |
|-------|---------------|
| Arbitrum | `arbitrum.blockscout.com` |
| Unichain | `unichain.blockscout.com` |
| Scroll | `scroll.blockscout.com` |
| SKALE Base | `skale-base.explorer.skalenodes.com` |

Blockscout v2 endpoints:
- Transactions: `/api/v2/addresses/{addr}/transactions`
- Token transfers: `/api/v2/addresses/{addr}/token-transfers`

## RPC-Only Chains (no explorer API)

| Chain | RPC URL | Notes |
|-------|---------|-------|
| HyperEVM | `rpc.hyperliquid.xyz/evm` | getLogs limited to 1000 blocks |
| Monad | `rpc.monad.xyz` | Prunes historical state |

## Non-EVM APIs

| Chain | API | Endpoint Pattern |
|-------|-----|-----------------|
| Solana | Solana RPC | `api.mainnet-beta.solana.com` (getSignaturesForAddress) |
| Fogo | Fogo RPC | `rpc.fogo.nightly.app` (same as Solana methods) |
| SUI | SUI RPC | `fullnode.mainnet.sui.io:443` (suix_queryTransactionBlocks) |
| NEAR | NearBlocks | `api.nearblocks.io/v1/account/{addr}/ft-txns` |
| Stellar | Horizon | `horizon.stellar.org/accounts/{addr}/payments` |
| Algorand | Algonode | `mainnet-idx.algonode.cloud/v2/accounts/{addr}/transactions` |

## Token Decimals

| Token | Decimals | Exception |
|-------|----------|-----------|
| USDC | 6 | BSC USDC uses **18** decimals |
| EURC | 6 | |
| AUSD | 6 | |
| PYUSD | 6 | |
| USDT | 6 | |
