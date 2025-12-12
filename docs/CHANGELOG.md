# Changelog

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
