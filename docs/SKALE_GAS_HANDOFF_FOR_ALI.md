# x402r + Ultravioleta Facilitator: SKALE Integration Reference

**Date**: 2026-03-21
**Facilitator**: `https://facilitator.ultravioletadao.xyz` (v1.39.1)
**Status**: SKALE live -- payments, ERC-8004, and domain name fix deployed.

---

## SKALE Networks

| Property | Mainnet | Testnet |
|----------|---------|---------|
| Chain ID | `1187947933` | `324705682` |
| CAIP-2 | `eip155:1187947933` | `eip155:324705682` |
| x402 v1 name | `skale-base` | `skale-base-sepolia` |
| RPC (public) | `https://skale-base.skalenodes.com/v1/base` | `https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha` |
| Explorer | `https://skale-base-explorer.skalenodes.com/` | `https://base-sepolia-testnet-explorer.skalenodes.com/` |
| Gas token | CREDIT (free, pre-allocated at genesis) | CREDIT (free, pre-allocated at genesis) |
| EIP-1559 | No -- legacy tx only | No -- legacy tx only |

---

## Stablecoin: USDC.e (Verified On-Chain 2026-03-21)

SKALE Base uses **USDC.e** -- bridged from Base L2 via the SKALE Bridge. Full EIP-3009 support (`transferWithAuthorization`).

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
| Supply (mainnet) | ~199 USDC |

Clients constructing EIP-3009 authorizations must use domain name `"Bridged USDC (SKALE Bridge)"` with version `"2"`. Using `"USDC"` or `"USD Coin"` will produce the wrong DOMAIN_SEPARATOR and signatures will fail.

---

## Facilitator API Endpoints

Base URL: `https://facilitator.ultravioletadao.xyz`

Full interactive docs: [/docs](https://facilitator.ultravioletadao.xyz/docs) | OpenAPI spec: [/api-docs/openapi.json](https://facilitator.ultravioletadao.xyz/api-docs/openapi.json)

### x402 Payment

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/verify` | Verify EIP-3009 authorization |
| `POST` | `/settle` | Submit payment on-chain (facilitator pays CREDIT gas) |
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

## Network Identifier Compatibility

Both facilitators (ours and PayAI) expose SKALE in both v1 and v2 formats, matching the pattern used for all other networks:

| Format | Mainnet | Testnet |
|--------|---------|---------|
| v1 (string) | `skale-base` | `skale-base-sepolia` |
| v2 (CAIP-2) | `eip155:1187947933` | `eip155:324705682` |

Verified against `facilitator.payai.network/supported` -- both formats are already listed for SKALE, same as Base, Polygon, Avalanche, etc.
