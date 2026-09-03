# x402 Payment Facilitator — Ultravioleta DAO

The settlement service behind the Ultravioleta DAO stack. It verifies x402 payment
authorizations and submits them on-chain, so a buyer signs a stablecoin payment and
never pays gas.

This host is a **facilitator**, not a paid API. It charges nothing for its own routes;
the money that moves is the buyer's payment going to the seller.

- **Networks:** 21 mainnets + 18 testnets (39 identifiers) across 7 chain families
- **Stablecoins:** USDC, USDT, EURC, AUSD, PYUSD, USDG
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
- Auth metadata (RFC 9728): `/.well-known/oauth-protected-resource`
- Workflow manifest: `/workflows.json`
- LLM context: `/llms.txt`, `/llms-full.txt`

## Links

- Website: https://facilitator.ultravioletadao.xyz/
- Source: https://github.com/UltravioletaDAO/x402-rs
- Operator: Ultravioleta DAO — https://ultravioletadao.xyz/
