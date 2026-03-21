# x402r + Ultravioleta Facilitator: SKALE Integration Reference

**Date**: 2026-03-21
**Facilitator**: `https://facilitator.ultravioletadao.xyz` (v1.39.0)
**Status**: SKALE live in `/supported` and ERC-8004. Domain name bug in USDC.e pending fix (see below).

---

## SKALE Networks

| Property | Mainnet | Testnet |
|----------|---------|---------|
| Chain ID | `1187947933` | `324705682` |
| CAIP-2 | `eip155:1187947933` | `eip155:324705682` |
| x402 v1 name | `skale-base` | `skale-base-sepolia` |
| RPC (public) | `https://skale-base.skalenodes.com/v1/base` | `https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha` |
| Explorer | `https://skale-base.explorer.skalenodes.com/` | `https://base-sepolia-testnet-explorer.skalenodes.com/` |
| Gas | sFUEL (free, faucet: sfuelstation.com) | sFUEL (free, faucet: sfuel.dirtroad.dev/staging) |
| EIP-1559 | No -- legacy tx only | No -- legacy tx only |

---

## Stablecoins on SKALE Base (Verified On-Chain 2026-03-21)

Both tokens are bridge-minted clones (SKALE Bridge from Base L2), both support full EIP-3009 (`transferWithAuthorization`).

### USDC.e

| Property | Value |
|----------|-------|
| Contract (mainnet) | `0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20` |
| Contract (testnet) | `0x2e08028E3C4c2356572E096d8EF835cD5C6030bD` |
| `name()` | `"Bridged USDC (SKALE Bridge)"` |
| `symbol()` | `"USDC.e"` |
| `decimals()` | `6` |
| `version()` | `"2"` |
| EIP-712 domain name | **`"Bridged USDC (SKALE Bridge)"`** |
| EIP-712 domain version | `"2"` |
| DOMAIN_SEPARATOR (mainnet) | `0xe182bdd83730d37cc41c779b38c22661c4c347f647925627d97926ee54dd4044` |
| EIP-3009 | Confirmed (`transferWithAuthorization` exists) |
| Supply (mainnet) | ~199 USDC |

### USDT

| Property | Value |
|----------|-------|
| Contract (mainnet) | `0x2bF5bF154b515EaA82C31a65ec11554fF5aF7fCA` |
| `name()` | `"Tether USD"` |
| `symbol()` | `"USDT"` |
| `decimals()` | `6` |
| EIP-712 domain name | **`"Tether USD"`** |
| EIP-712 domain version | `"1"` |
| DOMAIN_SEPARATOR (mainnet) | `0x91b8f5ae0eabbc330a822f2131f63b373800bfbc36416ade82b7035e6bb940af` |
| EIP-3009 | Confirmed (`transferWithAuthorization` exists) |
| Supply (mainnet) | ~1.08 USDT |

---

## Known Issue: EIP-712 Domain Name Bug

Our facilitator currently has the USDC.e EIP-712 domain name hardcoded as `"USDC"` in `src/network.rs`. The actual on-chain domain name is `"Bridged USDC (SKALE Bridge)"`.

This means **USDC.e signature verification will fail** until we deploy the fix. Clients constructing EIP-3009 authorizations must use the correct domain name or signatures won't verify.

**Fix**: Pending deployment. Will update `network.rs` with `name: "Bridged USDC (SKALE Bridge)"`.

---

## Facilitator API Endpoints

Base URL: `https://facilitator.ultravioletadao.xyz`

Full interactive docs: [/docs](https://facilitator.ultravioletadao.xyz/docs) | OpenAPI spec: [/api-docs/openapi.json](https://facilitator.ultravioletadao.xyz/api-docs/openapi.json)

### x402 Payment

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/verify` | Verify EIP-3009 authorization |
| `POST` | `/settle` | Submit payment on-chain (facilitator pays sFUEL gas) |
| `POST` | `/accepts` | Payment requirements negotiation |
| `GET` | `/supported` | Supported networks and tokens |

### ERC-8004

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/register` | Register agent on IdentityRegistry |
| `GET` | `/identity/{network}/{agent_id}` | Agent identity lookup |
| `GET` | `/identity/{network}/{agent_id}/metadata/{key}` | Agent metadata |
| `GET` | `/identity/{network}/total-supply` | Registered agent count |
| `POST` | `/feedback` | Submit reputation feedback |
| `POST` | `/feedback/revoke` | Revoke feedback |
| `POST` | `/feedback/response` | Append response to feedback |
| `GET` | `/reputation/{network}/{agent_id}` | Agent reputation score |

### Discovery

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/discovery/register` | Register a resource |
| `GET` | `/discovery/resources` | List resources |

---

## ERC-8004 Contracts on SKALE

| Contract | Mainnet | Testnet |
|----------|---------|---------|
| IdentityRegistry | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004A818BFB912233c491871b3d84c89A494BD9e` |
| ReputationRegistry | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| ValidationRegistry | Not deployed | Not deployed |

---

## What x402r Needs to Decide

- [ ] Which network identifier format? (`skale-base` vs `eip155:1187947933`)
- [ ] Which stablecoin(s)? USDC.e is the primary (higher liquidity). USDT exists with EIP-3009 but has ~1 USDT supply.
- [ ] Does x402r call the facilitator HTTP API, or interact with SKALE on-chain directly?
- [ ] Any additional endpoints or capabilities needed?
