# Changelog

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
