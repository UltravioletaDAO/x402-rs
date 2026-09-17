# Hedera transaction log — September 16, 2026 (America/New_York)

Public receipts fetched independently from Hedera Mirror Node on September 17 UTC. [Machine-readable evidence with complete parent and child receipts](2026-09-16-hedera-transaction-ledger.json). No private keys, AWS resource identifiers or signed transaction payloads are included.

## Mainnet funding and allocation

The user funded bootstrap `0.0.10868282` with 0.5 HBAR, then 38.36460034 HBAR. The controlled EVM alias resolves to that native account. Setup allocated 30 HBAR to facilitator `0.0.10868300`, 0.5 HBAR to buyer `0.0.10868301`, and 0.1 HBAR to merchant `0.0.10868302`; account creation and USDC associations also incurred fees.

At the user's explicit request, a SaucerSwap V1 router swap (`0.0.3045981`, `swapETHForExactTokens`) bought exactly **0.1 native USDC `0.0.456858`**, delivered directly to the buyer. Input was **1.35504344 HBAR**, consensus fee **0.17672510 HBAR**, total **1.53176854 HBAR** from the bootstrap. The quote was simulated before signing, maximum input limited to **1.36859388 HBAR** (1% slippage), max transaction fee **1 HBAR**, immutable transaction ID, no retry with a new ID, and no token allowance. Router and token IDs were checked against [SaucerSwap's deployments](https://docs.saucerswap.finance/developers/contracts) and [Circle's native USDC](https://www.circle.com/multi-chain-usdc/hedera). Receipt: [0.0.10868282@1789613793.242236127](https://hashscan.io/mainnet/transaction/0.0.10868282-1789613793-242236127).

The table includes external incoming transfers for reconciliation. They were not submitted by the agent; unsolicited dust is not part of the operational funding request. `agent` identifies setup, swap, fixture and payment transactions submitted with controlled accounts.

## Transaction inventory

| Network | Submitted by | Operation | Result | Fee (HBAR) | Native transaction |
| --- | --- | --- | --- | --- | --- |
| hedera:testnet | agent | CRYPTOCREATEACCOUNT | SUCCESS | 0.64079560 | [0.0.8511157-1789601002-134584547](https://hashscan.io/testnet/transaction/0.0.8511157-1789601002-134584547) |
| hedera:testnet | agent | CRYPTOCREATEACCOUNT | SUCCESS | 0.64079560 | [0.0.8511157-1789601005-128855449](https://hashscan.io/testnet/transaction/0.0.8511157-1789601005-128855449) |
| hedera:testnet | agent | CRYPTOCREATEACCOUNT | SUCCESS | 0.64079560 | [0.0.8511157-1789601005-508869330](https://hashscan.io/testnet/transaction/0.0.8511157-1789601005-508869330) |
| hedera:testnet | agent | TOKENASSOCIATE | SUCCESS | 0.64207719 | [0.0.10576385-1789601394-393565693](https://hashscan.io/testnet/transaction/0.0.10576385-1789601394-393565693) |
| hedera:testnet | agent | TOKENASSOCIATE | SUCCESS | 0.64207719 | [0.0.10576385-1789601396-620842915](https://hashscan.io/testnet/transaction/0.0.10576385-1789601396-620842915) |
| hedera:testnet | external | CRYPTOTRANSFER | SUCCESS | 0.01281590 | [0.0.11920-1789602603-279916745](https://hashscan.io/testnet/transaction/0.0.11920-1789602603-279916745) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00256317 | [0.0.10576385-1789602861-163262387](https://hashscan.io/testnet/transaction/0.0.10576385-1789602861-163262387) |
| hedera:testnet | agent | TOKENCREATION | SUCCESS | 12.81719381 | [0.0.8511157-1789602996-824544319](https://hashscan.io/testnet/transaction/0.0.8511157-1789602996-824544319) |
| hedera:testnet | agent | TOKENASSOCIATE | SUCCESS | 0.64207719 | [0.0.8511157-1789602997-437960427](https://hashscan.io/testnet/transaction/0.0.8511157-1789602997-437960427) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00256317 | [0.0.10576385-1789603064-182136857](https://hashscan.io/testnet/transaction/0.0.10576385-1789603064-182136857) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.01409749 | [0.0.10576385-1789603066-663879859](https://hashscan.io/testnet/transaction/0.0.10576385-1789603066-663879859) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.01409749 | [0.0.10576385-1789603067-331865180](https://hashscan.io/testnet/transaction/0.0.10576385-1789603067-331865180) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00256317 | [0.0.10576385-1789603383-342495891](https://hashscan.io/testnet/transaction/0.0.10576385-1789603383-342495891) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.01409749 | [0.0.10576385-1789604566-023406789](https://hashscan.io/testnet/transaction/0.0.10576385-1789604566-023406789) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00128158 | [0.0.8511157-1789604666-286038316](https://hashscan.io/testnet/transaction/0.0.8511157-1789604666-286038316) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00256317 | [0.0.10576385-1789608851-534483569](https://hashscan.io/testnet/transaction/0.0.10576385-1789608851-534483569) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00256317 | [0.0.10576385-1789609544-527025786](https://hashscan.io/testnet/transaction/0.0.10576385-1789609544-527025786) |
| hedera:testnet | agent | CRYPTOTRANSFER | SUCCESS | 0.01409749 | [0.0.10576385-1789609553-483480778](https://hashscan.io/testnet/transaction/0.0.10576385-1789609553-483480778) |
| hedera:mainnet | external | CRYPTOTRANSFER | SUCCESS | 0.00136011 | [0.0.10868270-1789610858-792925761](https://hashscan.io/mainnet/transaction/0.0.10868270-1789610858-792925761) |
| hedera:mainnet | external | CRYPTOTRANSFER | SUCCESS | 0.00136011 | [0.0.10868270-1789610996-994069541](https://hashscan.io/mainnet/transaction/0.0.10868270-1789610996-994069541) |
| hedera:mainnet | external | CRYPTOTRANSFER | SUCCESS | 0.00136011 | [0.0.10231006-1789611039-483533706](https://hashscan.io/mainnet/transaction/0.0.10231006-1789611039-483533706) |
| hedera:mainnet | agent | CRYPTOCREATEACCOUNT | SUCCESS | 0.68005929 | [0.0.10868282-1789611247-839125036](https://hashscan.io/mainnet/transaction/0.0.10868282-1789611247-839125036) |
| hedera:mainnet | external | CRYPTOTRANSFER | SUCCESS | 0.00136011 | [0.0.10231006-1789611253-599393457](https://hashscan.io/mainnet/transaction/0.0.10231006-1789611253-599393457) |
| hedera:mainnet | agent | CRYPTOCREATEACCOUNT | SUCCESS | 0.68005929 | [0.0.10868282-1789611253-642507506](https://hashscan.io/mainnet/transaction/0.0.10868282-1789611253-642507506) |
| hedera:mainnet | external | CRYPTOTRANSFER | SUCCESS | 0.00136011 | [0.0.10231006-1789611256-314615261](https://hashscan.io/mainnet/transaction/0.0.10231006-1789611256-314615261) |
| hedera:mainnet | agent | CRYPTOCREATEACCOUNT | SUCCESS | 0.68005929 | [0.0.10868282-1789611258-322502377](https://hashscan.io/mainnet/transaction/0.0.10868282-1789611258-322502377) |
| hedera:mainnet | agent | TOKENASSOCIATE | SUCCESS | 0.68141941 | [0.0.10868300-1789611321-793960187](https://hashscan.io/mainnet/transaction/0.0.10868300-1789611321-793960187) |
| hedera:mainnet | agent | TOKENASSOCIATE | SUCCESS | 0.68141941 | [0.0.10868300-1789611324-530241915](https://hashscan.io/mainnet/transaction/0.0.10868300-1789611324-530241915) |
| hedera:mainnet | agent | CONTRACTCALL | SUCCESS | 0.17672510 | [0.0.10868282-1789613793-242236127](https://hashscan.io/mainnet/transaction/0.0.10868282-1789613793-242236127) |
| hedera:mainnet | agent | CRYPTOTRANSFER | SUCCESS | 0.00272023 | [0.0.10868300-1789613986-642051223](https://hashscan.io/mainnet/transaction/0.0.10868300-1789613986-642051223) |
| hedera:mainnet | agent | CRYPTOTRANSFER | SUCCESS | 0.01488410 | [0.0.10868300-1789614004-016143440](https://hashscan.io/mainnet/transaction/0.0.10868300-1789614004-016143440) |

## Acceptance and limits

- [Public mainnet payments](2026-09-16-hedera-mainnet-public-canaries.json): HBAR and USDC, verified 402 → 200 through the production facilitator, replay rejected, retry preserves ID, exact principal/fees reconciled, persisted SHA-384 matches Mirror.
- [Public testnet payments](2026-09-16-hedera-public-canaries.json): same acceptance checks; [additional validation](2026-09-16-hedera-validation.json) includes custom FT, concurrency, quota and real crash recovery.
- Current native throughput budget: ten newly admitted 1-HBAR-max-fee payments per network per UTC day. This is conservative sponsor exposure accounting, not actual consumed gas.
- Native rail: x402 v2/exact only. HBAR uses eight decimals; native USDC six. No native escrow, upto, NFT, allowance, custom-fee token or DX402 extension support is claimed.
- [Arc evidence and integration details](../networks/arc.md) remain separate: direct EOA exact v1/v2 on `eip155:5042` and `eip155:5042002`.
