# Changelog

## [1.35.0] - 2026-03-02

### Added - Solana Smart Wallet Support (Squads, Crossmint, SWIG)

- **Two-path verification for Solana transactions** enabling smart wallet payments:
  - **Path 1 (unchanged)**: Top-level TransferChecked detection for standard EOA wallets (~5ms)
  - **Path 2 (new)**: CPI inner instruction scanning for smart wallet transfers (~50ms)
  - Automatic fallback: tries Path 1 first, falls back to Path 2 if no top-level transfer found
- Smart wallets execute token transfers via Cross-Program Invocation (CPI), where TransferChecked
  appears as an inner instruction rather than a top-level one. This blocked all program-controlled
  accounts from using x402 payments on Solana.
- Now supports: Squads multisig, Crossmint custodial wallets, SWIG session wallets,
  SPL Governance DAOs, and any smart wallet that uses CPI-based token transfers
- Simulation now requests `inner_instructions: true` to capture CPI calls at all depths
- Inner instruction validation: verifies exactly ONE matching TransferChecked with correct
  amount, destination ATA, mint, and authority (prevents split/double transfer attacks)
- Added dependency: `solana-transaction-status-client-types` for inner instruction type parsing

### Hardened - Solana ComputeBudget Duplicate Rejection

- Reject duplicate `SetComputeUnitLimit` instructions (Solana applies last-wins, which could
  bypass facilitator caps)
- Reject duplicate `SetComputeUnitPrice` instructions (same last-wins bypass risk)
- References: [coinbase/x402#646](https://github.com/coinbase/x402/issues/646) RFC security model

### Context

- Requested by CryptoFede (Crossmint) for [lobster.cash](https://lobster.cash) integration
- Dexter and Faremeter already shipped closed-source implementations
- Ultravioleta DAO is the first open-source facilitator with smart wallet support
- Fully backward compatible: existing standard wallet payments work unchanged

## [1.29.0] - 2026-02-07

### Added - x402r Escrow Multi-Chain Support (9 Networks)

- **x402r escrow contracts configured for 9 networks** (from x402r-sdk A1igator/multichain-config):
  - Mainnets: Base, Ethereum, Polygon, Arbitrum, Celo, Monad, Avalanche
  - Testnets: Base Sepolia, Ethereum Sepolia
- Updated all Base contract addresses to match new SDK deployment
- `/supported` endpoint dynamically advertises escrow networks with deployed PaymentOperators
- Added `ESCROW_NETWORKS` constant as single source of truth for escrow support
- PaymentOperator deployment required on each network before settlement is active

### Fixed - ERC-8004 Network Name Consistency & Identity Lookup Robustness

- **BREAKING FIX**: `supported_network_names()` now derives names from `Network::Display` instead of hardcoded strings
  - Fixes "base-mainnet" vs "base" mismatch: `/feedback` returned "base-mainnet" but POST endpoints expected "base"
  - All API responses now use the canonical network names that serde/FromStr accept
- Removed `exists()` calls from identity lookup handlers (`/identity/:network/:agentId`, `/identity/:network/:agentId/metadata/:key`)
  - `exists()` is not part of standard ERC-721 and may not be implemented on all proxy contracts
  - Now uses `ownerOf()` revert detection for non-existent agents (returns proper 404)
  - Fixes "execution reverted" errors on Base and Ethereum identity lookups
- Added ERC-8004 section to README.md with 14-network table, API endpoints, and usage examples

## [1.28.1] - 2026-02-06

### Fixed - Avalanche ERC-8004 missing from /feedback API

- Fixed `supported_network_names()` not including "avalanche" and "avalanche-fuji"
- Updated all ERC-8004 tests to include Avalanche networks
- `/feedback` endpoint now correctly reports 14 ERC-8004 networks

## [1.28.0] - 2026-02-06

### Added - Avalanche C-Chain ERC-8004 Support (14 Networks)

- Added Avalanche C-Chain mainnet ERC-8004 contracts (CREATE2 deterministic addresses)
- Added Avalanche Fuji testnet ERC-8004 contracts
- Updated landing page ERC-8004 showcase: 8 mainnet badges, 14 total networks
- Updated all network counts (stats card, feature card, i18n EN/ES)
- On-chain bytecode verification confirmed for all 4 contracts

## [1.27.0] - 2026-02-05

### Improved - Landing Page ERC-8004 Showcase & Audit Fixes

- Added dedicated ERC-8004 showcase section with three-pillar design (Identity, Reputation, Validation)
- Added network badges with logos for all 7 ERC-8004 mainnets
- Added 4th stat card showing "12 ERC-8004 Networks" with purple gradient
- Added 4th feature card "On-Chain Reputation" with ERC-8004 highlight
- Updated SDK section from "14+ networks" to "19 mainnets supported"
- Full i18n support (EN/ES) for all new ERC-8004 content
- Fixed agent file parse errors (CRLF line endings in aegis-rust-architect.md, terraform-aws-architect.md)
- Removed invalid ralph-wiggum plugin references from global settings

## [1.26.0] - 2026-02-05

### Added - ERC-8004 Multi-Network Expansion (12 Networks)

This release expands ERC-8004 Trustless Agents support from 3 to 12 networks,
enabling cross-chain reputation across all major EVM ecosystems.

#### Supported Networks (12 total)

**Mainnets (7):**
| Network | Contract Addresses |
|---------|-------------------|
| Ethereum | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` / `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| Base | Same (CREATE2 deterministic) |
| Polygon | Same (CREATE2 deterministic) |
| Arbitrum | Same (CREATE2 deterministic) |
| Celo | Same (CREATE2 deterministic) |
| BSC | Same (CREATE2 deterministic) |
| Monad | Same (CREATE2 deterministic) |

**Testnets (5):**
| Network | Contract Addresses |
|---------|-------------------|
| Ethereum Sepolia | `0x8004A818BFB912233c491871b3d84c89A494BD9e` / `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| Base Sepolia | Same (deterministic) |
| Polygon Amoy | Same (deterministic) |
| Arbitrum Sepolia | Same (deterministic) |
| Celo Sepolia | Same (deterministic) |

#### Files Changed

| File | Change |
|------|--------|
| `src/erc8004/mod.rs` | Added 9 new network contracts, updated functions |
| `static/index.html` | Updated ERC-8004 section with 12 networks |

#### SDK Updates

- **Python SDK v0.8.0**: Added all 12 networks to `Erc8004Network` and `ERC8004_CONTRACTS`
- **TypeScript SDK v2.19.0**: Added all 12 networks with shared address constants

#### New Skill

Added `/add-erc8004-network` skill for automated ERC-8004 network integration.

---

## [1.25.0] - 2026-02-04

### Added - ERC-8004 Base Mainnet Support

This release enables ERC-8004 (Trustless Agents) reputation contracts on Base Mainnet.
The ERC-8004 contracts are now deployed on Base using CREATE2 deterministic addresses,
meaning the same addresses work across all supported chains.

#### Supported Networks for ERC-8004

| Network | IdentityRegistry | ReputationRegistry |
|---------|------------------|-------------------|
| Ethereum Mainnet | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| Ethereum Sepolia | `0x8004A818BFB912233c491871b3d84c89A494BD9e` | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| Base Mainnet | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |

#### Cross-Chain Reputation

With this update, AI agents can now:
- Make payments on Base Mainnet (via x402 protocol)
- Submit reputation feedback on Base Mainnet (via ERC-8004)
- Use the same agent identity across Ethereum and Base

The `ProofOfPayment` returned from `/settle` can be used to submit feedback on any
ERC-8004 supported network, enabling cross-chain reputation flows.

#### Files Changed

| File | Change |
|------|--------|
| `src/erc8004/mod.rs` | Added `BASE_MAINNET_CONTRACTS` with official addresses |
| `src/erc8004/mod.rs` | Updated `get_contracts()` to return Base contracts |
| `src/erc8004/mod.rs` | Added Base to `supported_networks()` and `supported_network_names()` |
| `static/index.html` | Updated ERC-8004 section with Base Mainnet support |
| `static/index.html` | Added BaseScan contract links |

#### Reference

- ERC-8004 Specification: https://eips.ethereum.org/EIPS/eip-8004
- Official Contracts: https://github.com/erc-8004/erc-8004-contracts
- BaseScan ReputationRegistry: https://basescan.org/address/0x8004BAa17C55a88189AE136b182e5fdA19dE9b63

---

## [1.24.0] - 2026-02-03

### Added - x402r PaymentOperator Escrow Scheme

This release adds the x402r escrow payment scheme, enabling advanced escrow flows
(authorize/charge/release/refund) via the PaymentOperator contract on Base Mainnet.
This is the first payment scheme beyond "exact" supported by the facilitator.

#### New Payment Scheme: `escrow`

- **`Scheme::Escrow`** enum variant added to payment schemes
- `/supported` endpoint now advertises escrow support on Base Mainnet (CAIP-2: `eip155:8453`)
- Gated by `ENABLE_PAYMENT_OPERATOR=true` environment variable
- Escrow contract info exposed in `/supported` response:
  ```json
  {
    "x402Version": 2,
    "scheme": "escrow",
    "network": "eip155:8453",
    "extra": {
      "escrow": {
        "escrowAddress": "0x320a3c35f131e5d2fb36af56345726b298936037",
        "operatorAddress": "0xa06958d93135bed7e43893897c0d9fa931ef051c",
        "tokenCollector": "0x32d6ac59bce8dfb3026f10bcadb8d00ab218f5b6"
      }
    }
  }
  ```

#### Base Mainnet Contract Addresses

| Contract | Address |
|----------|---------|
| PaymentOperator | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` |
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| PaymentOperatorFactory | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |

#### Security Fixes

- **Address validation**: Client-provided contract addresses (operatorAddress,
  tokenCollector, escrowAddress) are now validated against hardcoded known
  deployments before any on-chain transaction is submitted. This prevents gas
  drain attacks where an attacker could specify arbitrary target addresses.
- **`encode_collector_data` fix**: Changed from ABI-encoding `(bytes, bytes)` to
  raw signature bytes, matching what the `ERC3009PaymentCollector` contract expects.
  The old encoding would have caused on-chain reverts.

#### Files Changed

| File | Change |
|------|--------|
| `src/types.rs` | New `Scheme::Escrow`, `EscrowSupportedInfo` struct |
| `src/facilitator_local.rs` | Escrow scheme in `/supported` (gated by feature flag) |
| `src/payment_operator/operator.rs` | Address validation, raw signature encoding |
| `src/payment_operator/addresses.rs` | `PAYMENT_OPERATOR` address, `OperatorAddresses` update |
| `src/chain/*.rs` | `escrow: None` field on all chain providers |
| `static/index.html` | PaymentOperator section with contract links |
| `terraform/*/main.tf` | `ENABLE_PAYMENT_OPERATOR=true` |
| `.env.example` | Updated PaymentOperator docs |

#### Protocol Team Notes (Ali Abdoli, 2026-02-03)

- **$100 USDC deposit limit**: Enforced by PaymentOperator contract per deposit
- **`refundPostEscrow`**: NOT functional in production (requires `tokenCollector`
  contract not yet implemented by protocol team)
- **Recommended approach**: Use refund-in-escrow (keep funds locked until arbiter
  decides release or refund) instead of post-escrow refund
- **ERC-8004 reputation gating**: Future feature under consideration - could add
  condition contracts that check ERC-8004 scores before allowing authorize/charge

#### Related Changes (Other Repos)

This release was part of a coordinated update across 3 repositories:

1. **Chamba MCP Server** (`chamba` repo, commit `0ee2cf4`):
   - 8 new MCP tools for AI agents: `chamba_escrow_authorize`, `chamba_escrow_release`,
     `chamba_escrow_refund`, `chamba_escrow_charge`, `chamba_escrow_partial_release`,
     `chamba_escrow_dispute`, `chamba_escrow_status`, `chamba_escrow_recommend_strategy`
   - Agent guide: `mcp_server/docs/ESCROW_AGENT_GUIDE.md`
   - Integration layer: $100 limit, arbiter escrow pattern

2. **Python SDK** (`uvd-x402-sdk-python`, commit `835e9f6`):
   - `DEPOSIT_LIMIT_USDC = 100_000_000` constant
   - `refund_post_escrow()` marked NOT FUNCTIONAL

3. **TypeScript SDK** (`uvd-x402-sdk-typescript`, commit `10b6e89`):
   - `DEPOSIT_LIMIT_USDC = '100000000'` constant
   - `refundPostEscrow()` marked NOT FUNCTIONAL

---

## [1.19.1] - 2026-01-06

### Fixed - Aggregator ISO8601 Timestamp Parsing

Fixed a bug where the discovery aggregator failed to parse responses from Coinbase and other facilitators that return `lastUpdated` as an ISO8601 string instead of a Unix timestamp.

#### Changes

- **Flexible timestamp parsing**: `lastUpdated` field now accepts both:
  - Unix timestamp (u64): `1767737779`
  - ISO8601 string: `"2026-01-06T20:22:59.724Z"`

- **Added 11 new facilitator sources**:
  - PayAI: `https://facilitator.payai.network`
  - Thirdweb: `https://api.thirdweb.com/v1/payments/x402`
  - QuestFlow: `https://facilitator.questflow.ai`
  - AurraCloud: `https://x402-facilitator.aurracloud.com`
  - AnySpend: `https://mainnet.anyspend.com/x402`
  - OpenX402: `https://open.x402.host`
  - x402.rs: `https://facilitator.x402.rs`
  - Heurist: `https://facilitator.heurist.xyz`
  - Polymer: `https://api.polymer.zone/x402/v1`
  - Meridian: `https://api.mrdn.finance`
  - Virtuals: `https://acpx.virtuals.io`

- **`FacilitatorConfig::all()`**: New method returns all 12 known facilitators

---

## [1.19.0] - 2026-01-06

### Added - Meta-Bazaar Discovery Aggregation

This release implements Phase 1 of the unified Bazaar architecture, enabling the facilitator to aggregate discoverable resources from external facilitators (like Coinbase). This transforms the Ultravioleta facilitator into a "Meta-Bazaar" that indexes services from across the x402 ecosystem.

#### New Features

- **Discovery Source Tracking**: Resources now track their origin
  - `DiscoverySource` enum: `SelfRegistered`, `Settlement`, `Crawled`, `Aggregated`
  - `source_facilitator` field identifies origin facilitator (e.g., "coinbase")
  - `first_seen` timestamp for when resource was discovered
  - `settlement_count` for tracking payment activity

- **Discovery Aggregator**: Background task that fetches from external facilitators
  - Fetches from Coinbase CDP Bazaar (1,700+ services)
  - Converts v1 network names to CAIP-2 format
  - Runs periodically (default: every hour)
  - Configurable via `DISCOVERY_AGGREGATION_INTERVAL`

- **Enhanced Filtering**: Query resources by source
  - `GET /discovery/resources?source=aggregated` - Show only aggregated resources
  - `GET /discovery/resources?source_facilitator=coinbase` - Show Coinbase resources
  - Combines with existing filters (category, network, provider, tag)

- **Bulk Import API**: Efficient resource ingestion
  - `DiscoveryRegistry::bulk_import()` for batch upserts
  - Smart deduplication by URL
  - Only updates if newer `last_updated` timestamp

#### Environment Variables

```bash
# Enable/disable aggregation (default: true)
DISCOVERY_ENABLE_AGGREGATION=true

# Aggregation interval in seconds (default: 3600 = 1 hour)
DISCOVERY_AGGREGATION_INTERVAL=3600
```

#### Architecture

```
External Facilitators          Ultravioleta Facilitator
+------------------+          +-------------------------+
| Coinbase Bazaar  |--fetch-->| DiscoveryAggregator     |
| 1,700+ services  |          |   |                     |
+------------------+          |   v                     |
                              | Convert to v2 format    |
+------------------+          |   |                     |
| Other Facilitator|--fetch-->|   v                     |
+------------------+          | DiscoveryRegistry       |
                              | (source: Aggregated)    |
                              +-------------------------+
```

#### API Changes

- `DiscoveryResource` struct now includes:
  - `source: DiscoverySource` (default: `self_registered`)
  - `source_facilitator: Option<String>`
  - `first_seen: Option<u64>`
  - `settlement_count: Option<u32>`

- `DiscoveryFilters` struct now supports:
  - `source: Option<String>`
  - `source_facilitator: Option<String>`

### Added - Settlement Tracking (Phase 2)

This update implements Phase 2 of the unified Bazaar architecture: automatic settlement tracking. Resources can now be auto-registered in the Bazaar discovery registry when payments are settled.

#### How It Works

When a payment is settled via `POST /settle`:
1. Check if `discoverable=true` in `paymentRequirements.extra`
2. If true, auto-register the resource in the Bazaar (if new) or increment its settlement count (if existing)
3. Resources are tagged with `source: Settlement` to distinguish from self-registered or aggregated resources

#### Usage

Resource providers can opt-in to discovery by adding `discoverable: true` to their payment requirements:

```json
{
  "paymentRequirements": {
    "scheme": "exact",
    "network": "eip155:8453",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "amount": "100000",
    "payTo": "0x...",
    "resource": "https://api.example.com/premium-data",
    "description": "Premium market data API",
    "extra": {
      "discoverable": true
    }
  }
}
```

#### Benefits

- **Zero-effort discovery**: Resources are automatically indexed when payments succeed
- **Settlement metrics**: `settlement_count` tracks payment activity per resource
- **Trust signals**: Resources with high settlement counts demonstrate active usage
- **Backward compatible**: Existing integrations work unchanged (discoverable defaults to false)

#### Technical Details

- Settlement tracking runs asynchronously (non-blocking)
- Uses `DiscoveryRegistry::track_settlement()` for upsert logic
- Resources created via settlement are tagged with `source: Settlement`
- Settlement count is incremented for existing resources

### Added - Discovery Crawler (Phase 3)

This update implements Phase 3 of the unified Bazaar architecture: the well-known endpoint crawler. The crawler periodically fetches `/.well-known/x402` from configured seed URLs to discover x402-enabled resources.

#### How It Works

1. Configure seed URLs via `DISCOVERY_CRAWL_URLS` environment variable
2. Crawler fetches `/.well-known/x402` from each domain
3. Parses the JSON response containing resource definitions
4. Imports discovered resources with `source: Crawled`

#### Well-Known Format

Resource providers should serve `/.well-known/x402` with this format:

```json
{
  "x402Version": 2,
  "resources": [
    {
      "url": "https://api.example.com/premium",
      "type": "http",
      "description": "Premium API endpoint",
      "accepts": [
        {
          "scheme": "exact",
          "network": "eip155:8453",
          "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
          "amount": "100000",
          "payTo": "0x...",
          "maxTimeoutSeconds": 300
        }
      ],
      "metadata": {
        "category": "finance",
        "provider": "Example Corp",
        "tags": ["market-data", "real-time"]
      }
    }
  ]
}
```

#### Environment Variables

```bash
# Enable/disable crawler (default: false)
DISCOVERY_ENABLE_CRAWLER=true

# Crawl interval in seconds (default: 86400 = 24 hours)
DISCOVERY_CRAWL_INTERVAL=86400

# Comma-separated list of seed URLs to crawl
DISCOVERY_CRAWL_URLS=https://api.example.com,https://data.service.io
```

#### Technical Details

- Uses `reqwest` for HTTP requests with 30s timeout
- Crawl runs in background task (non-blocking)
- Resources tagged with `source: Crawled` and `source_facilitator: <domain>`
- 404 responses are silently ignored (domain doesn't support x402)
- Invalid URLs are skipped with warning log

#### Complete Bazaar Architecture

With all three phases complete, the Bazaar now has four resource sources:

| Source | Description | Trigger |
|--------|-------------|---------|
| `self_registered` | Direct POST to `/discovery/register` | Manual registration |
| `settlement` | Auto-registered on successful `/settle` | `discoverable: true` in payment requirements |
| `aggregated` | Fetched from external facilitators (Coinbase) | Background task (hourly) |
| `crawled` | Discovered from `/.well-known/x402` endpoints | Background task (daily) |

---

## [1.10.0] - 2025-12-19

### Added - Multi-Stablecoin Support

This release adds support for 6 stablecoins with EIP-3009 `transferWithAuthorization` capability across 14 EVM networks.

#### Supported Tokens

| Token | Networks | Decimals | Description |
|-------|----------|----------|-------------|
| **USDC** | All 14 networks | 6 | USD Coin by Circle (default) |
| **EURC** | Ethereum, Base, Avalanche | 6 | Euro Coin by Circle |
| **AUSD** | Ethereum, Polygon, Arbitrum, Avalanche | 6 | Agora USD (CREATE2 - same address all chains) |
| **PYUSD** | Ethereum | 6 | PayPal USD by Paxos |
| **GHO** | Ethereum, Arbitrum, Base | 18 | Aave stablecoin |
| **crvUSD** | Ethereum, Arbitrum | 18 | Curve Finance stablecoin |

#### New Features

- **TokenType Enum**: New enum in `src/types.rs` for token identification
  - Values: `usdc`, `eurc`, `ausd`, `pyusd`, `gho`, `crvusd`
  - Default: `usdc` for backward compatibility
  - Methods: `decimals()`, `symbol()`, `all()`

- **Token Deployment Registry**: Comprehensive token contract addresses
  - `get_token_deployment(network, token_type)` - Get deployment info
  - `is_token_supported(network, token_type)` - Check availability
  - `supported_tokens_for_network(network)` - List tokens per network
  - `supported_networks_for_token(token_type)` - List networks per token

- **Dynamic EIP-712 Validation**: Per-token domain separator calculation
  - Extracts token type from payment payload
  - Uses correct token name/version for typed data signing
  - Handles different decimal places (6 vs 18)

- **Enhanced `/supported` Endpoint**: Token information in response
  - New `tokens` field with token addresses and decimals per network
  - `SupportedTokenInfo` struct with token metadata

- **Frontend Token Badges**: Visual token support display
  - Token pills with per-token colors on network cards
  - JavaScript-based dynamic rendering
  - Shows which stablecoins each network supports

#### Contract Addresses

```
EURC:
  Ethereum: 0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c
  Base:     0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42
  Avalanche: 0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD

AUSD (CREATE2 - same on all chains):
  0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a

PYUSD:
  Ethereum: 0x6c3ea9036406852006290770BEdFcAbA0e23A0e8

GHO:
  Ethereum: 0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f
  Arbitrum: 0x7dfF72693f6A4149b17e7C6314655f6A9F7c8B33
  Base:     0x6Bb7a212910682DCFdbd5BCBb3e28FB4E8da10Ee

crvUSD:
  Ethereum: 0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E
  Arbitrum: 0x498Bf2B1e120FeD3ad3D42EA2165E9b73f99C1e5
```

#### Backward Compatibility

- **No breaking changes** - USDC remains the default token
- Existing clients work without modification
- `tokenType` is optional in payment payloads (defaults to `usdc`)
- Non-EVM chains (Solana, NEAR, Stellar) continue with USDC only

#### Test Coverage

- 39 new unit tests for multi-stablecoin functionality
- TokenType enum serialization/deserialization
- Token deployment lookups and validation
- Decimal handling (6 vs 18)
- Network/token mapping verification

---

## [1.8.0] - 2025-12-12

### Added - x402 Protocol v2 Support

This release adds full support for the x402 Protocol v2 specification, enabling CAIP-2 chain-agnostic network identifiers while maintaining complete backward compatibility with v1 clients.

#### New Features

- **CAIP-2 Network Identifiers**: Networks can now be specified using the [CAIP-2 standard](https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-2.md)
  - EVM chains: `eip155:{chainId}` (e.g., `eip155:8453` for Base)
  - Solana: `solana:{genesisHash}` (e.g., `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp`)
  - NEAR: `near:mainnet` / `near:testnet`
  - Stellar: `stellar:pubnet` / `stellar:testnet`

- **Dual Protocol Support on `/verify` and `/settle`**:
  - Auto-detects v1 vs v2 request format from request body
  - V1 requests use network strings: `"network": "base-mainnet"`
  - V2 requests use CAIP-2: `"network": "eip155:8453"`
  - Both formats processed identically after parsing

- **Enhanced `/supported` Endpoint**:
  - Returns both v1 and v2 entries for each network
  - V1 entry: `{ "x402Version": 1, "network": "base-mainnet", "scheme": "exact" }`
  - V2 entry: `{ "x402Version": 2, "network": "eip155:8453", "scheme": "exact" }`
  - Clients can filter by `x402Version` to find their preferred format

#### New Files

- `src/caip2.rs` - CAIP-2 parsing and validation
  - `Namespace` enum: `Eip155`, `Solana`, `Near`, `Stellar`, `Fogo`
  - `Caip2NetworkId` struct with parsing, display, and serde support
  - Validation rules per namespace (chain ID, genesis hash, network name)

- `src/types_v2.rs` - v2 protocol types
  - `ResourceInfo` - Separated resource metadata
  - `PaymentRequirementsV2` - Requirements with CAIP-2 network
  - `PaymentPayloadV2` - Payload with extensions support
  - `VerifyRequestEnvelope` / `SettleRequestEnvelope` - Dual v1/v2 request handling
  - Conversion traits between v1 and v2 types

#### Modified Files

- `src/network.rs` - Added `FromStr`, `to_caip2()`, `from_caip2()` methods
- `src/handlers.rs` - Updated verify/settle handlers for dual protocol support
- `src/facilitator_local.rs` - `/supported` returns both v1 and v2 entries
- `src/lib.rs` - Exported new modules

#### Backward Compatibility

- **No breaking changes** - All existing v1 clients continue to work unchanged
- V1 network strings (`base-mainnet`) still fully supported
- V1 response formats unchanged
- Existing integrations require no modifications

#### Example Requests

**V1 Request (unchanged):**
```json
{
  "x402Version": 1,
  "paymentPayload": {
    "network": "base-mainnet",
    ...
  }
}
```

**V2 Request (new):**
```json
{
  "x402Version": 2,
  "paymentPayload": {
    "network": "eip155:8453",
    ...
  }
}
```

---

## [1.7.9] - 2025-12-11

### Fixed
- Removed emojis from Rust log messages to prevent CloudWatch encoding issues

---

## [Unreleased] - 2025-10-28

### Updated - 2025-10-28 (Evening)
- **Network badges updated** to show all 4 supported networks:
  - Avalanche Fuji (testnet) + Avalanche C-Chain (mainnet)
  - Base Sepolia (testnet) + Base (mainnet)
- **Network descriptions updated** in both English and Spanish
  - English: "Supports Avalanche (Fuji testnet and C-Chain mainnet) and Base (Sepolia testnet and mainnet)."
  - Spanish: "Soporta Avalanche (testnet Fuji y mainnet C-Chain) y Base (testnet Sepolia y mainnet)."

### Added - Interactive Landing Page

#### New Features
- **Interactive landing page** at root endpoint (`/`)
  - Animated grid background with cyberpunk aesthetic
  - Bilingual support (English/Spanish) with instant switching
  - Gradient hero title with color-shifting animation
  - Prominent network badges for all supported networks (2 testnets + 2 mainnets)
  - Interactive stats cards (hover to scale)
  - Feature cards with glow effects on hover
  - Syntax-highlighted code example (JetBrains Mono font)
  - Animated endpoint list with slide effects
  - Scroll-based fade-in animations
  - Network-colored glows (Avalanche red, Base blue)

- **Logo support** at `/logo.png` endpoint
  - Embedded at compile time
  - Graceful fallback if logo not provided
  - Shows in header with pulse animation

#### Design System
- **Fonts**: Inter (UI) + JetBrains Mono (code)
- **Colors**:
  - Avalanche: `#e84142` (red)
  - Base: `#0052ff` (blue)
  - Accent: `#00d4ff` (cyan)
- **Animations**:
  - Moving grid background (20s loop)
  - Hero glow pulse (4s)
  - Gradient text shift (8s)
  - Fade-in on scroll
  - Hover transforms and glows

#### Files Modified
- `static/index.html` - Complete landing page (NEW)
- `static/logo.png` - Placeholder logo (NEW)
- `static/README.md` - Static assets documentation (NEW)
- `static/SETUP.md` - Setup guide (NEW)
- `src/handlers.rs` - Added `get_index()` and `get_logo()` handlers
- `src/main.rs` - Added routes for `/` and `/logo.png`
- `LANDING_PAGE.md` - Complete documentation (NEW)

#### Technical Details
- HTML/CSS/JS embedded at compile time via `include_str!()`
- Logo embedded via `include_bytes!()`
- Zero external dependencies (fonts via Google Fonts CDN only)
- Responsive design (mobile, tablet, desktop)
- Intersection Observer API for scroll animations
- Network badges with sweep animation on hover

### API Compatibility
- All existing API endpoints unchanged
- `/health` - Health check
- `/supported` - Payment schemes
- `/verify` - Payment verification
- `/settle` - On-chain settlement

### Networks Supported
- ✅ Avalanche Fuji (testnet)
- ✅ Avalanche C-Chain (mainnet)
- ✅ Base Sepolia (testnet)
- ✅ Base (mainnet)

### Browser Support
- Chrome/Edge 90+
- Firefox 88+
- Safari 14+
- Mobile browsers (iOS Safari, Chrome Mobile)

### Performance
- Initial page load: ~50ms
- Language switch: Instant (client-side)
- Zero API calls on page load
- Total page size: ~30KB (including fonts)

---

## Previous Releases

### [0.1.0] - Previous
- Initial x402 facilitator implementation
- EIP-3009 meta-transaction support
- Multi-network provider support
- Health and supported endpoints
- Verify and settle endpoints
