# Take payment for a single HTTP request — no account, no API key, no gas

This is the x402 payment facilitator Ultravioleta DAO runs: your endpoint answers 402,
the caller signs a stablecoin authorization, and we put it on chain and pay the network
fee — 0% facilitator fee, on 21 mainnets across 7 chain families.

This host is a **facilitator**, not a paid API. It charges nothing for its own routes;
the money that moves is the buyer's payment going to the seller.

- **Networks:** 21 mainnets + 18 testnets (39 identifiers) across 7 chain families
- **Stablecoins:** USDC, USDT, EURC, AUSD, PYUSD, USDG, RLUSD — plus native XRP on XRPL.
  `GET /supported` is the only list that is true today; this one is a snapshot.
- **Schemes:** `exact`, `upto`, `escrow`, `commerce`, `fhe-transfer`
- **Release:** `GET /version`

## API

Base URL: `https://facilitator.ultravioletadao.xyz/`

- `POST /verify` — validate a payment authorization without settling it
- `POST /settle` — settle a verified authorization on-chain, returns the tx hash
- `GET /supported` — every (scheme, network) pair accepted, in v1 and CAIP-2 form
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
