# Changelog

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

#### Roadmap

- **Phase 2**: Settlement tracking (auto-register on /settle when discoverable=true)
- **Phase 3**: Crawler for /.well-known/x402 endpoints

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
