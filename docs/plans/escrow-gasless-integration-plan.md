# Escrow Gasless Integration Plan — Facilitator Side

**Source:** IRC discussion between claude-facilitator and claude-exec-market
**Date:** 2026-02-10
**Channel:** #execution-market-facilitator @ irc.meshrelay.xyz

---

## Context

Execution Market needs gasless escrow operations (release/refund) through the Facilitator. Currently the Facilitator handles gasless payments via EIP-3009 `transferWithAuthorization` on `/settle`. The goal is to extend this to escrow contract operations.

---

## FASE 1 — Immediate (No contract changes, no facilitator changes)

**Status:** Ready to implement NOW

### What changes
- **Only Execution Market changes** — Facilitator stays as-is
- Agent signs EIP-3009 auths at APPROVAL time (not at task creation)
- 2 transactions per task instead of 3 (agent->worker + agent->treasury)
- Platform wallet eliminated completely

### Flow
1. **Create task:** `/verify` to confirm agent has balance. No auth signed yet.
2. **Approve task:** Agent signs 2 fresh auths (agent->worker 92%, agent->treasury 8%). MCP sends 2x `POST /settle`.
3. **Cancel task:** Nothing happens. No auth was signed.

### Double-spend mitigation
- Double `/verify` (at creation + at approval)
- ERC-8004 reputation: agents that double-spend lose reputation and get banned
- Low risk for AI agents controlled by EM (not adversarial)

### Scalability confirmed
- 3,000 settles/day on Base = ~$3-9/day gas — sustainable
- Facilitator handles concurrency via async Rust + nonce management
- No rate limits in facilitator; bottleneck is RPC provider

### Gas costs per settle (transferWithAuthorization)
| Network | Cost per settle |
|---------|----------------|
| Base | $0.001-0.003 |
| Polygon | $0.002-0.005 |
| Arbitrum | $0.001-0.003 |
| Optimism | $0.001-0.003 |
| Ethereum | $0.50-3.00 (NOT viable for micropayments) |

### Cost model
- **Tier 1** (up to 50 tasks/day): FREE — subsidized to grow ecosystem
- **Tier 2** (50-500 tasks/day): 0.1% of settlement amount
- **Tier 3** (500+ tasks/day): 0.05% — negotiable with volume
- Uses existing `maxFeeBps` in PaymentInfo — zero UX change

---

## FASE 2 — Future (When x402r team can redeploy contracts)

**Status:** Plan documented, waiting on x402r contract updates
**Dependency:** x402r team deploys updated AuthCaptureEscrow + PaymentOperator on 9 networks

### Contract changes needed (Execution Market writes Solidity)
1. Add `captureToAddress(paymentInfo, amount, toAddress)` to PaymentOperator
   - Overrides the receiver fixed in PaymentInfo
   - Solves the "worker unknown at lock time" problem
2. Register facilitator wallet as authorized operator via `addOperator()`

### Facilitator changes needed
1. **New module:** `src/chain/escrow.rs`
   - ABI for AuthCaptureEscrow contract
   - Functions: `call_capture_to_address()`, `call_partial_void()`

2. **New endpoints in `src/handlers.rs`:**

   ```
   POST /escrow/release
   {
     network: "base-mainnet",
     escrow_address: "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
     payment_info: { receiver, amount, tier, maxFeeBps, feeReceiver },
     release_to: "0xWorkerAddress",
     release_amount: 1000000
   }
   ```
   Facilitator calls `captureToAddress()` paying gas.

   ```
   POST /escrow/refund
   {
     network: "base-mainnet",
     escrow_address: "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
     payment_info: { receiver, amount, tier, maxFeeBps, feeReceiver },
     refund_amount: 1000000
   }
   ```
   Facilitator calls `partialVoid()` paying gas.

3. **Authentication:** HMAC-SHA256
   - Header: `X-Escrow-Auth`
   - Shared key stored in AWS Secrets Manager
   - Same pattern as existing `/verify` and `/settle` auth

4. **Operator registration:**
   - Facilitator mainnet wallet registered as operator on all 9 networks
   - Facilitator testnet wallet registered on testnet contracts

### Escrow contract addresses (current deployments)
| Network | Address |
|---------|---------|
| Base | `0xb9488351E48b23D798f24e8174514F28B741Eb4f` |
| Ethereum | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| Polygon | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| Arbitrum | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| Celo | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| Monad | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| Avalanche | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| Optimism | `0x320a3c35F131E5D2Fb36af56345726B298936037` |

---

## Action Items

### Execution Market (Fase 1 — NOW)
1. Refactor `payment_dispatcher.py` — auth at approval, not creation
2. Implement double `/verify` (create + approve)
3. Remove platform wallet from flow

### Execution Market (Fase 2 — FUTURE)
1. Write `captureToAddress()` Solidity function
2. Submit for facilitator team review

### Facilitator (Fase 2 — FUTURE)
1. Review + deploy updated contract on 9 networks
2. Register facilitator wallet as operator on each network
3. Implement `src/chain/escrow.rs` module
4. Add `POST /escrow/release` and `POST /escrow/refund` endpoints
5. Add HMAC-SHA256 auth middleware for escrow endpoints

---

## Sync Session Results (2026-02-10, Session 2)

**Participants:** claude-facilitator, claude-exec-market, claude-python-sdk, claude-ts-sdk

### Facilitator Golden Source Inventory (v1.31.2)
- **22 mainnets**, 5 stablecoins (USDC 22 redes, EURC 3, USDT 4, AUSD 8, PYUSD 1)
- **Core x402 endpoints:** POST /verify, POST /settle, GET /supported, GET /health, GET /version, GET /docs
- **ERC-8004 endpoints:** POST /register, POST /feedback, POST /feedback/revoke, POST /feedback/response, GET /reputation/{network}/{agentId}
- **Discovery:** POST /discovery/register
- **Protocol:** v1 (string network names) + v2 (CAIP-2 format), auto-detected

### Execution Market Inventory (reported by claude-exec-market)
- **8 networks enabled:** Base, Ethereum, Polygon, Arbitrum, Celo, Monad, Avalanche, Optimism
- **Token registry has 15 EVM total** (but only 8 active)
- **Stablecoins:** USDC, EURC, USDT, PYUSD, AUSD
- **SDK:** uvd-x402-sdk >= 0.11.0 (Python)
- **Facilitator URL:** https://facilitator.ultravioletadao.xyz

### Key Findings
1. **No endpoint mismatch** — ERC-8004 endpoints (/register, /feedback, /reputation) DO exist in facilitator (initially reported incorrectly, then corrected)
2. **POST /settle has NO `pay_to` field** — receiver is determined by the `to` field in the EIP-3009 authorization itself
3. **13+ networks available for Execution Market expansion** — Scroll, Sei, Unichain, BSC, XDC, XRPL_EVM, Fogo, SKALE, HyperEVM, NEAR, Stellar, Algorand, Sui
4. **SDKs did not confirm settle_dual() helper** — Exec-market will implement locally

### Agreed Action Plan
| # | Action | Owner | Status | Facilitator Impact |
|---|--------|-------|--------|-------------------|
| 1 | Fase 1: auth on approve, 2 direct settles | Exec-Market | Immediate | 0 changes |
| 2 | End-to-end test with real money | Exec-Market | Immediate | Verify tx on-chain |
| 3 | settle_dual() helper with retry | Exec-Market (local) | Short-term | 0 changes |
| 4 | Escrow gasless (Fase 2) | Facilitator + x402r | Future | New endpoints |
| 5 | Add 7+ new networks to EM registry | Exec-Market | Short-term | 0 changes |

### Technical Notes from Session
- EIP-3009 nonces MUST be unique per signer — recommended: `keccak256(taskId + type + timestamp)`
- For retry logic: do NOT reuse failed nonces — generate new ones
- Use exponential backoff for settle retries
- Network priority for expansion: Tier 1 (already have), Tier 2 (Celo, Monad), Tier 3 (Scroll, Sei, Unichain)
