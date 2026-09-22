# Take payment for a single HTTP request — no account, no API key, no gas

This is the x402 payment facilitator Ultravioleta DAO runs: your endpoint answers 402,
the caller signs a stablecoin authorization, and we put it on chain and pay the network
fee — 0% facilitator fee, including Arc and native Hedera mainnet/testnet.

This host is a **facilitator**, not a paid API. It charges nothing for its own routes;
the money that moves is the buyer's payment going to the seller.

- **Networks:** EVM (including Arc), SVM, NEAR, Stellar, Sui, Algorand, XRPL and native Hedera. Availability is read from `/supported`.
- **Stablecoins:** USDC, USDT, EURC, AUSD, PYUSD, USDG, RLUSD — plus native XRP on XRPL. Hedera accepts USDC only.
  `GET /supported` is the only list that is true today; this one is a snapshot.
- **Schemes:** `exact`, `upto`, `escrow`, `commerce`, `fhe-transfer`
- **Release:** `GET /version`
- **Language (idioma):** this page is English (`en`) and has no Spanish translation.
  The HTML landing page at `/` and the other human pages carry English and Spanish
  at the same URL, switched by their EN/ES selector; there is no `/es/` path.

## Arc and native Hedera

| Network | Payment identifier | Asset | Facilitator fee payer |
| --- | --- | --- | --- |
| Arc mainnet | `arc` / `eip155:5042` | USDC/EURC (6 decimals) | `0x103040545AC5031A11E8C03dd11324C7333a13C7` |
| Arc testnet | `arc-testnet` / `eip155:5042002` | USDC/EURC (6 decimals) | `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8` |
| Hedera mainnet | `hedera:mainnet` | USDC `0.0.456858` (6) | `0.0.10868300` |
| Hedera testnet | `hedera:testnet` | USDC `0.0.429274` (6) | `0.0.10576385` |

Discover availability and the current network-specific `extra.feePayer` from
`/supported`. These are facilitator accounts, not merchant destinations. Set
`payTo` to the seller's own account. Arc supports direct EOA `exact` payments in
x402 v1/v2; its ERC-20 USDC address is `0x3600000000000000000000000000000000000000`,
with EIP-712 domain `USDC` / `2`. Its 18-decimal gas view is the same balance;
payment amounts use 6 decimals.

Hedera supports native `CryptoTransfer`, `exact`, **x402 v2 only**. It uses numeric
accounts and native token IDs, not EVM chain IDs 295/296. Buyer and recipient must
be associated with USDC. The sponsor pays HBAR fees without contributing payment
principal. HBAR is not accepted as payment; new HBAR offers are rejected. Neither addition enables
escrow, `upto`, Gateway or ERC-8004 on that network. Native Hedera also rejects
durable-evidence and other unsupported extensions.

## API

Base URL: `https://facilitator.ultravioletadao.xyz/`

- `POST /verify` — validate a payment authorization without settling it
- `POST /settle` — settle a verified authorization on-chain, returns the transaction hash or native Hedera transaction ID
- `GET /supported` — every (scheme, network) pair accepted, with its supported protocol version and identifiers (Hedera is v2 only)
- `POST /accepts` — negotiate payment requirements (Faremeter-compatible)
- `POST /mcp` — MCP server (Streamable HTTP, stateless): `x402_supported`,
  `x402_accepts`, `x402_verify`, `x402_settle`, over the same handlers
- `GET /mcp` — the MCP guide for a reader (HTML, or Markdown with
  `Accept: text/markdown`)
- `GET /health` — `{"status":"healthy"}`
- `GET /version` — the running release
- `GET /events` — SSE, one message per verify/settle
- `GET /transactions`, `GET /api/stats` — recorded operations and aggregates
- `GET /identity/{network}/{agentId}`, `GET /reputation/{network}/{agentId}` — ERC-8004

Full contract: `/openapi.json` (Swagger UI at `/docs`).

## Agent resources

- Agent manual: `/skill.md`
- Authentication guide: `/auth.md`
- A2A agent card: `/.well-known/agent-card.json` (legacy path `/.well-known/agent.json`)
- x402 discovery: `/.well-known/x402`
- API catalog (RFC 9727): `/.well-known/api-catalog`
- Agent skills index: `/.well-known/agent-skills/index.json`
- MCP server card: `/.well-known/mcp/server-card.json` (endpoint: `POST /mcp`)
- MCP guide: `/mcp`
- Network table, built from `/supported`: `/networks`
- x402 guide (verify, settle, escrow, upto, and both counters): `/x402`
- Integration guide, including what is not promised: `/integrar`
- DX402 durable evidence: `/dx402`
- ERC-8004 identity and reputation, and the describe.net boundary: `/erc8004`
- Auth metadata (RFC 9728): `/.well-known/oauth-protected-resource`
- Workflow manifest: `/workflows.json`
- LLM context: `/llms.txt`, `/llms-full.txt`

## Links

- Website: https://facilitator.ultravioletadao.xyz/
- Source: https://github.com/UltravioletaDAO/x402-rs
- Operator: Ultravioleta DAO — https://ultravioletadao.xyz/

## Portable facilitator receipts (Arc and Hedera)

Arc exact USDC/EURC and native Hedera USDC return `receipt` alongside verify/settle.
Discover `/supported.facilitatorReceipts`, `/receipts`,
`/schemas/facilitator-receipt-v1.json` and `/.well-known/receipt-keys.json`.
For private lookup, persist a purchase context and send `X-UVD-Purchase` (base64
JSON: purchaseId, secret accessToken, method, url, bodySha256). The merchant must
validate it against the actual request. Query `/receipts/{receiptId}` with
`Authorization: Bearer <accessToken>`. Never log the context or create a fresh
signature after uncertainty. An unknown receipt means poll/replay the original
authorization, not a new payment. Receipt confirmation does not prove delivery.
Python `fetch_with_receipt` and TypeScript `fetchWithReceipt` return the original
HTTP response plus receipt/payment state. Supply trusted issuer keys for offline
signature verification. Live EURC acceptance was proven on Arc mainnet on
2026-09-22 (x402 v2, receipt `confirmed`, tx 0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128);
Arc testnet is still pending. Other networks
retain their existing responses. Full contract:
https://github.com/UltravioletaDAO/x402-rs/blob/main/docs/facilitator-receipts.md
