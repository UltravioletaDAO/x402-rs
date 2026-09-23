# Changelog

## [2.37.1] - 2026-09-22

- Link previews are network-agnostic again. The landing's `og:description` goes back to its text from before #70 (83ac6d07), without the chain-family count: "Gasless x402 verify and settle. No fee, no account, no API key." Every page (`/`, `/x402`, `/dx402`, `/erc8004`, `/bazaar`, `/networks`, `/mcp`, `/integrar`, `/stats`, `/events/live`) goes back to `og:image` = `logo.png`, and loses the Arc/Hedera card's `og:image:width`, `og:image:height` and `og:image:alt` and the `twitter:card`/`twitter:image` tags that #70 added.
- The facilitator's own description no longer singles out Arc or Hedera: the A2A agent card (`/.well-known/agent-card.json` and `/.well-known/agent.json`), the opening of `/index.md` (and so of `/llms-full.txt`), `/mcp.md` and the "What it is" paragraph of `/mcp`, in English and Spanish. Network lists and per-network instructions are unchanged.
- No page references `og-arc-hedera.png`/`.svg` any more. The files stay, and the `/og-arc-hedera.png` route still serves the PNG.

## [2.37.0] - 2026-09-22

- ERC-8004 identity and reputation on Arc mainnet (`arc`) and Arc testnet (`arc-testnet`), using the canonical registries: identity `0x8004A169…a432`, reputation `0x8004BAa1…9b63` and validation `0x8004Cc84…AB58` on mainnet; `0x8004A818…BD9e`, `0x8004B663…8713` and `0x8004Cb1B…4272` on testnet. All six were read with `eth_getCode` against the RPCs the facilitator deploys (`rpc.mainnet.arc.io`, `rpc.testnet.arc.io`): 130-byte proxies whose EIP-1967 implementation is the one Base and Base Sepolia run, `getVersion()` = `2.0.0`, `ownerOf(1)` answered on both. The ERC-8004 set grows from 21 to 23 networks (13 mainnets + 10 testnets). Arc payments are unchanged: still `exact` only, with no `upto`, escrow or relayed (EIP-7702) feedback, because no feedback delegate is deployed on Arc yet.
- `test_supported_networks_list` runs again. Its `#[test]` attribute had been duplicated onto the test above it, so it never ran and still asserted 20 networks while the list held 21.
- The OpenAPI ERC-8004 prose names every network in the set, and a test now fails when a network is missing from it or the stated count drifts.

## [2.36.5] - 2026-09-22

- The ERC-8004 Solana senders count a write against its network's daily limit before the transaction goes out, and give the place back only if the RPC refuses the transaction in preflight.
- `POST /register` on an EVM network refuses a recipient that is not an EVM address with 400, before anything is minted.
- The built-in daily ERC-8004 write limit for `arc` and `arc-testnet` is 100.
- Terraform: the Arc mainnet low-balance alert gets its own threshold in place of the default.
- A test pins the ERC-8004 write rate-limit period to exactly 12 seconds.

## [2.36.4] - 2026-09-22

- ERC-8004 writes that send a transaction (`/register`, `/feedback` and the `/feedback/*` submits) are limited per network per UTC day. Past the limit a write answers 429 with code `erc8004_daily_write_limit` and a `Retry-After` that runs to 00:00 UTC, without touching the chain. A write only keeps its place in the count if a transaction was broadcast. Limits: `ERC8004_DAILY_WRITE_CAP` for every network and `ERC8004_DAILY_WRITE_CAP_<NETWORK>` for one; `ENABLE_ERC8004_WRITES` still turns every write off.
- A test pins the period of the ERC-8004 write rate limit.

## [2.36.3] - 2026-09-22

- The ERC-8004 write routes (`/register`, `/feedback` and `/feedback/*`) draw on a per-IP budget of their own: 1 token every 12s, burst 30. `/discovery/register` and the bazar admin routes keep theirs (1 token every 12s, burst 250).
- The per-IP rate limiter accepts an IPv6 address in square brackets without a port, the form the load balancer appends.
- The MCP server copies every `X-Forwarded-For` line onto the request it forwards, not only the first.
- Terraform declares the load balancer's `xff_header_processing_mode = "append"`, the value it already runs with.

## [2.36.2] - 2026-09-22

- Per-IP rate limits key on the client address the load balancer appends to `X-Forwarded-For` (the header's last entry), and on the TCP peer when the header is absent; `X-Real-IP` and `Forwarded` are no longer read. The server now carries `ConnectInfo`, so a direct connection without the header is keyed on its peer instead of answering 500 `rate_limit_key_unavailable`. A test fails if any governor in `src/` keys on anything else.
- Docs: EURC on Arc mainnet is no longer described as pending. A funded 0.01 EURC x402 v2 payment was verified and settled on 2026-09-22 (tx `0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128`); Arc testnet funded acceptance remains pending.

## [2.36.1] - 2026-09-17

- Return HTTP 200 for authorized receipt lookups even when the original payment returned an HTTP error; preserve its signed status and the original POST response.
- Raise the Hedera testnet daily reservation ceiling to 12 HBAR for the expanded acceptance matrix; retain the mainnet ceiling of 10 HBAR.
- Document the required execution-role secret grant before deploying a new receipt signing key.

## [2.36.0] - 2026-09-17

- Add portable signed facilitator receipts for Arc exact USDC/EURC and Hedera USDC, including both mainnet and testnet.
- Bind receipts to purchase and authorization; preserve payment state independently of the merchant HTTP result.
- Persist and resume the original authorization, expose private receipt lookup, and verify Ed25519 provenance with trusted issuer keys.
- Document recovery limits, merchant propagation and the pending live EURC acceptance.
- Shuffle mainnet/testnet blockchain cards once per page load; filters and language changes retain that visit's order.
