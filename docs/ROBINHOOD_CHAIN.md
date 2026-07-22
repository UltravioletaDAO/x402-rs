# Robinhood Chain Integration (v1.50.0)

> Status as of **2026-07-20**: **LIVE in production** (payment mainnet #21).
> First USDC-less network in the facilitator — settlement runs on **Paxos USDG**.

## TL;DR

| | |
|---|---|
| Deployed version | v1.50.0 (ECS task def rev 252, CI run 29764577300) |
| Mainnet | `robinhood` / `eip155:4663` — **live**, settling USDG |
| Testnet | `robinhood-testnet` / `eip155:46630` — live, wallet funded 0.01 ETH |
| Schemes live on 4663 | `exact` (EIP-3009) + `upto` (Permit2) |
| Mainnet wallet gas | **NOT funded yet** (0 ETH) — see [Funding](#funding-the-mainnet-wallet) |
| Position | 2nd public x402 facilitator ever on this chain; 1st settling via native EIP-3009 |

## Chain data (all live-verified 2026-07-20)

| Field | Mainnet | Testnet |
|-------|---------|---------|
| Chain ID | 4663 (`0x1237`) | 46630 (`0xb626`) |
| CAIP-2 | `eip155:4663` | `eip155:46630` |
| RPC (keyless) | `https://rpc.mainnet.chain.robinhood.com` | `https://rpc.testnet.chain.robinhood.com` |
| Explorer | [robinhoodchain.blockscout.com](https://robinhoodchain.blockscout.com) | [explorer.testnet.chain.robinhood.com](https://explorer.testnet.chain.robinhood.com) |
| Faucet | — | [faucet.testnet.chain.robinhood.com](https://faucet.testnet.chain.robinhood.com) / [Chainlink](https://faucets.chain.link/robinhood-testnet) |
| Stack | Arbitrum Orbit (Nitro), settles to Ethereum | settles to Sepolia |
| Gas token | ETH (18 dec) | ETH |
| EIP-1559 | Yes (type-2 accepted; priority fee is a no-op — FCFS sequencer) | Yes |
| Blocks | ~100 ms | ~100 ms |
| Docs | <https://docs.robinhood.com/chain> | — |

Mainnet launched **2026-07-01**. Permissionless (anyone can deploy contracts).
Only ONE keyless public RPC exists (the official, rate-limited one); Alchemy/QuickNode/Chainstack serve it with API keys if we ever need premium.

## USDG (Global Dollar) — the settlement stablecoin

There is **NO Circle USDC on this chain** (native or bridged — verified against
Circle's official network list and the full Blockscout token census). The
canonical dollar is Paxos **USDG**:

| Field | Value |
|-------|-------|
| Mainnet address | `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168` |
| Testnet address | `0x7E955252E15c84f5768B83c41a71F9eba181802F` |
| Decimals | 6 |
| EIP-3009 | Yes — both variants (v,r,s `0xe3ee160e` AND bytes `0xcf092995`), verified from typehash + verified facet source + live dispatch probe |
| EIP-712 domain | `name="Global Dollar"`, `version="1"` — **cryptographically verified** against on-chain `DOMAIN_SEPARATOR()` on BOTH chains |
| Source | [Paxos docs](https://docs.paxos.com/guides/stablecoin/usdg/mainnet) + Blockscout admin-verified |

### Critical integration gotchas

1. **`version()` reverts on-chain** (Paxos facet dispatcher → `FacetNotFound`).
   The static EIP-712 entry in `src/network.rs` is *mandatory* — the on-chain
   fallback in `assert_domain` can never resolve this token. Clients using
   `PaymentRequirements.extra` should send `{"name":"Global Dollar","version":"1"}`.
2. **Impostor tokens everywhere.** The chain hosts fake 18-decimal
   "USDC" (`0x0CE454B6...`), "PYUSD" (`0x102a39df...`), "USDT0" (`0x602c5921...`)
   and even a fake USDG clone (`0x1383b43A...`). The strict allow-list
   (`supported_asset_addresses`) is **USDG-only** on this network and rejects
   all of them (covered by `test_robinhood_asset_allowlist_is_usdg_only`).
3. **Do not use the USDG OFT wrapper** `0x0d54755f...28d1` (LayerZero bridge
   adapter) as the asset address.
4. USDG is yield-bearing (rebases upward) and facet-upgradeable by Paxos —
   balance checks stay conservative; issuer-trust noted in the risk register.

## Scheme support matrix on Robinhood Chain

| Scheme | Status | Notes |
|--------|--------|-------|
| `exact` | ✅ LIVE (mainnet + testnet) | Native EIP-3009 `transferWithAuthorization` on USDG. Dexter (the only other facilitator here) settles via Permit2 instead — we are the first with native EIP-3009 on this chain. |
| `upto` | ✅ LIVE (mainnet) / ⛔ testnet | Zero per-network config. Canonical `x402UptoPermit2Proxy` `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002` **is deployed on 4663** (byte-identical to Base). NOT deployed on testnet 46630 — the new `assert_proxy_deployed` guard hard-fails there instead of faking success. |
| `escrow` / `commerce` (x402r) | ⛔ Blocked | Requires Ali/BackTrack's CREATE3 infra on 4663 (not deployed — verified: `create3::ESCROW`/factories have no code there) + our PaymentOperator deploy. Same playbook as SKALE. |
| `batch-settlement` | ⛔ Not implemented | It is a **public x402 Foundation scheme** (specs/schemes/batch-settlement + `@x402/evm`), NOT Dexter-proprietary. The contract `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003` is already deployed on 4663. Implementing it in our Rust facilitator is a standalone feature project. |

### The upto proxy bug this release also fixed

Our `UPTO_PERMIT2_PROXY_ADDRESS` used to be
`0x4020633461b2895a48930Ff97eE8fCdE8E520002` — an address with **no code on
any chain** (miscopied when upto was implemented). Because calls/transactions
to code-less addresses succeed vacuously, an upto settlement would have
returned `success=true` + a real tx hash **while moving zero tokens**.
v1.50.0 fixes the constant to the spec-canonical `0x4020A4f3...240002` and
adds `assert_proxy_deployed()` (eth_getCode) to both verify and settle.

Canonical proxy presence (checked 2026-07-20): ✅ Base, Ethereum, Arbitrum,
Optimism, BSC, HyperEVM, SKALE Base, Monad, World, Robinhood mainnet.
❌ Avalanche, Celo, Scroll, Unichain, Robinhood testnet (replayable by anyone
via Arachnid CREATE2 `0x4e59b448...956C`, salt `0x...b000000001db633d`, needs
Cancun EVM + gas).

## Wallets

| Wallet | Address | Status |
|--------|---------|--------|
| EVM Mainnet | `0x103040545AC5031A11E8C03dd11324C7333a13C7` | ⚠️ **0 ETH on 4663 — needs funding** |
| EVM Testnet | `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8` | ✅ 0.01 ETH (funded 2026-07-20) |

### Funding the mainnet wallet

All routes below were live-verified 2026-07-20 (API chain lists + real quotes).
Ranked easiest-first for ~0.01 ETH:

| Route | From | Cost / speed | Notes |
|-------|------|--------------|-------|
| **[Relay](https://relay.link/bridge/robinhood)** ⭐ | Base, Arbitrum, Optimism, L1, +many | ~$0.03 on 0.01 ETH, **~2-30 s** | Officially listed on [docs.robinhood.com/chain/bridging](https://docs.robinhood.com/chain/bridging). Live quote: 0.01 ETH in → 0.009986 ETH out, native ETH route (unambiguous). |
| [Across](https://across.to/?to=robinhood) | Base, Arbitrum, Optimism, L1, Unichain, +more | ~$0.03-0.04, seconds | Also officially listed. ETH route flagged `isNative:true` (auto-unwrap) — confirm you receive native ETH, not WETH. |
| [gas.zip](https://www.gas.zip/) | 300+ chains | cents, ~30-60 s | Chain 4663 listed (min $0.005, max $750/deposit). Best for tiny top-ups or splitting across chains. |
| Robinhood app withdrawal | Robinhood balance | network fee, minutes | ETH withdrawals support the "Robinhood Chain" network selector. Needs KYC'd US account. |
| [Canonical Arbitrum bridge](https://portal.arbitrum.io/bridge?destinationChain=robinhood-chain&sourceChain=ethereum) | Ethereum L1 only | L1 gas ~$1-5, ~10 min | Trustless, but overkill for 0.01 ETH; exits back to L1 take 7 days. |

NOT available (verified absent): Superbridge, Brid.gg, Orbiter, Stargate
(the LayerZero presence on 4663 is the USDG OFT only — no ETH lane).

**Recommended:** Relay from Base →
`0x103040545AC5031A11E8C03dd11324C7333a13C7`, then verify:
`cast balance 0x103040545AC5031A11E8C03dd11324C7333a13C7 --rpc-url https://rpc.mainnet.chain.robinhood.com --ether`

## Production verification (2026-07-20)

- `/version` → `1.50.0`; `/health` → healthy; branding intact
- `/supported` → `robinhood`, `robinhood-testnet`, `eip155:4663`, `eip155:46630`
  all present with USDG; `upto` advertised for both
- `python scripts/verify_landing_canonical.py` → `[OK]` (21 mainnets)
- All non-EVM families intact (Solana, Fogo, NEAR, Stellar, Algorand, Sui, XRPL)
- `/robinhood.png` + `/usdg.png` serving; landing cards live (mainnet + testnet)
- Balances Lambda updated manually (`aws lambda update-function-code`) — the
  CI targeted terraform apply intentionally excludes it. Lambda may return
  `null` for robinhood (rate-limited RPC; same as Scroll) — the browser-side
  RPC fallback covers the cards (CORS `*` confirmed on both Robinhood RPCs).
- OFAC blacklist: `status=loaded`, 724 records

## Remaining work

1. **Fund mainnet wallet** (see above) — until then, mainnet verify works but settle will fail on gas.
2. **Testnet e2e**: run a USDG `transferWithAuthorization` payment end-to-end on 46630 (wallet already funded). If the bytes variant ever reverts (it should not — the facet implements both), add USDG addresses to `requires_vrs_signature` in `src/chain/evm.rs`.
3. **SDK parity**: add robinhood + USDG to uvd-x402-sdk-python / uvd-x402-sdk-typescript token tables.
4. Optional: CREATE2-replay the upto proxy on testnet 46630 (+ Avalanche/Celo/Scroll/Unichain) so upto works there too.
5. Optional: escrow on 4663 — coordinate CREATE3 infra deploy with Ali, then deploy our PaymentOperator.
6. Optional: implement the `batch-settlement` scheme (public spec) in the Rust facilitator.
7. Optional: World Chain (`eip155:480`) — not in `src/network.rs` at all today.

## Session log (2026-07-20)

- 13 recon/verify agents across 3 workflows established chain facts, USDG
  authenticity (vs a competitor's claims), and repo touch-points.
- Implementation: 24 files (+689/−216) — `Network::Robinhood(-Testnet)`,
  `TokenType::Usdg`, `USDGDeployment`, chain-id/EIP-1559/RPC wiring, landing
  cards + i18n (21 mainnets), lambda, `config/supported_tokens.json`,
  openapi, stablecoin matrix, terraform env vars, README, CHANGELOG.
- Found + fixed the pre-existing upto proxy bug (above) during recon.
- 3 adversarial review agents (compile-completeness, security, consistency)
  caught 9 issues pre-push (incl. a CI-breaking test count); all fixed.
- Shipped via CI (commits `7dbe194` + `ee02844`, tag `v1.50.0`): tests green
  first try, deploy green, full production verification passed.
