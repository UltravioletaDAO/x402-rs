# Facilitator website maintenance

The production site is embedded from `static/` into the Rust server. Human pages
share English/Spanish translations; agent documents have their own Markdown
routes. Rebuild and deploy to publish changes.

## Network capabilities

Use `/supported` as the runtime source for networks, protocol versions, assets,
schemes and network-specific fee payers. The landing cards, `/networks` table,
family summary and native wallet links derive enabled state from that response.

- Arc mainnet: `arc` / `eip155:5042`; testnet: `arc-testnet` / `eip155:5042002`.
  Direct EOA USDC exact payments, x402 v1/v2. USDC payments use 6 decimals.
- Native Hedera: `hedera:mainnet` / `hedera:testnet`, x402 v2/exact only.
  HBAR `0.0.0` uses 8 decimals; USDC `0.0.456858` / `0.0.429274` uses 6.
  Sponsor IDs currently mainnet `0.0.10868300`, testnet `0.0.10576385`.
  Keep displayed IDs bound to the advertised fee payer, never the bootstrap account.

A payment network does not automatically gain escrow, ERC-8004 or other schemes.
Keep those capability lists separate and derived from their own sources.

## Public surfaces to update together

- `index.html`, `networks.html`, integration/MCP/payment guides and both translations.
- `src/openapi.rs` for Swagger and its native request example.
- `index.md`, `skill.md`, `llms.txt`, `mcp.md`, A2A agent card and README.
- Regenerate `llms-full.txt` with `scripts/build_llms_full.sh`.
- `og-arc-hedera.svg` is the editable social-card source; render it to the
  1200x630 `og-arc-hedera.png`, inspect the image and serve its explicit Rust route.
  Open Graph and Twitter metadata use the PNG route.
- Run `scripts/verify_landing_canonical.py --offline` and the affected Rust checks,
  then verify the deployed version, live discovery, real UI filter clicks and links.

Operational instructions and receipts: [Arc](../docs/networks/arc.md),
[native Hedera](../docs/guides/hedera-native.md).
