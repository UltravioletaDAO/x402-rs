# Changelog

## [2.36.4] - 2026-09-22

- ERC-8004 writes that send a transaction (`/register`, `/feedback` and the `/feedback/*` submits) are limited per network per UTC day, counted per task. Past the limit a write answers 429 with code `erc8004_daily_write_limit` and a `Retry-After` that runs to 00:00 UTC, without touching the chain. A write only keeps its place in the count if a transaction was broadcast. Limits: `ERC8004_DAILY_WRITE_CAP` for every network and `ERC8004_DAILY_WRITE_CAP_<NETWORK>` for one; `ENABLE_ERC8004_WRITES` still turns every write off.
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
