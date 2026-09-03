# Give your agent four tools that pay for things

*This document is English only, on purpose: it is an agent surface. The same
guide for a human reader, in English or Spanish, is the HTML at
<https://facilitator.ultravioletadao.xyz/mcp>.*

This facilitator answers MCP on the same URL it answers HTTP. Four tools, one
stateless endpoint, nothing to install and no API key. A tool call is dispatched
through the very same REST router as the request it names, so a tool and a curl
cannot disagree about anything.

| | |
|---|---|
| Endpoint | `https://facilitator.ultravioletadao.xyz/mcp` |
| Transport | Streamable HTTP, stateless, POST |
| Authentication | none — the payer's signature is the only authority |
| Price | 0% — this endpoint never answers `402` |
| Server card | `/.well-known/mcp/server-card.json` |

## What you get here, and what you do not

What is being offered is settlement, not information. This service verifies an
x402 payment authorization against the chain it names and then broadcasts it,
paying the gas out of its own wallet, on seven chain families. The buyer signs;
the buyer never holds native tokens for gas.

Four things this server is **not**:

- **Not a wallet.** It holds no key of yours, signs nothing on your behalf and
  cannot move a coin you did not authorize. The signature inside the payload is
  the only authority there is, which is also why there is nothing to log in to.
- **Not a paid API.** The MCP endpoint charges nothing and never answers `402`.
  The only money that moves is the buyer's payment going to the seller.
- **Not a second implementation.** Each tool runs the request through the
  facilitator's own REST router — the same handler, the same rate limiter, the
  same writer lease. No code path here could answer a different truth from the
  HTTP door.
- **Not an admin console.** Revoking feedback, suppressing a bazaar resource,
  minting an identity and the DX402 writes are all deliberately absent from the
  tool list. An MCP client is a language model holding a menu, and a tool that
  erases somebody else's reputation does not belong on one.

## One transport, one server

Streamable HTTP, and only that. The server is stateless: every JSON-RPC message
is an independent POST, no `mcp-session-id` is issued, and `tools/call` works
without a previous `initialize`. Nothing lives in memory between two calls.

Claude Code:

```bash
claude mcp add --transport http x402-facilitator \
  https://facilitator.ultravioletadao.xyz/mcp
```

Claude Desktop, and any client with a config file:

```json
{
  "mcpServers": {
    "x402-facilitator": {
      "type": "streamable-http",
      "url": "https://facilitator.ultravioletadao.xyz/mcp"
    }
  }
}
```

By hand. The handshake is optional on a stateless server, but it is the shortest
call that proves the endpoint is reachable and says which release is answering:

```bash
curl -sS https://facilitator.ultravioletadao.xyz/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"my-agent","version":"1.0"}}}'
```

```json
{"jsonrpc":"2.0","id":1,"result":{
  "protocolVersion":"2025-06-18",
  "capabilities":{"tools":{}},
  "serverInfo":{"name":"x402-facilitator","version":"<the running release>"}}}
```

**Both `Accept` types, or `406`.** The transport rejects a request whose
`Accept` does not name *both* `application/json` and `text/event-stream`, even
though this server never opens a stream and always answers JSON.

**A `Host` outside the allowlist is `403`**, checked before anything else runs.

**`GET /mcp` is the human guide** (this document with `Accept: text/markdown`,
the HTML page otherwise). A client that asks for `application/json` or
`text/event-stream` still gets the `405` telling it to POST.

## The decision loop

```
x402_supported()            <- can you settle THIS scheme on THIS network?
      |                        the only authoritative answer. no arguments.
x402_accepts(accepts)       <- narrow the seller's 402 offer to what this
      |                        facilitator settles, enriched with feePayer,
      |                        token list and escrow addresses. moves nothing.
      |     the buyer signs an EIP-3009 authorization for one of them
x402_verify(payload, reqs)  <- would this settle? signature, nonce, amount,
      |                        timestamps, token and network. SUBMITS NOTHING.
x402_settle(payload, reqs)  <- broadcast. real funds. irreversible.
                               send idempotencyKey on every retry.
```

## The four tools, and only four

| Tool | Is | Arguments | Moves money |
|---|---|---|---|
| `x402_supported` | `GET /supported` | none | no |
| `x402_accepts` | `POST /accepts` | `accepts`, `x402Version`, `error` | no |
| `x402_verify` | `POST /verify` | `x402Version`, `paymentPayload`, `paymentRequirements` | no |
| `x402_settle` | `POST /settle` | the same three, plus optional `idempotencyKey` | **yes, irreversibly** |

The arguments of each tool *are* the JSON body of the request it stands for, and
the result is that request's response body verbatim, in a single text content
block. Everything `/skill.md` says about `/verify` and `/settle` is true here.

```bash
curl -sS https://facilitator.ultravioletadao.xyz/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"x402_supported","arguments":{}}}'
```

`result.content[0].text` is the exact body of `GET /supported`: every
`(scheme, network)` pair, each network spelled twice — the x402 v1 name
(`"base"`) and the CAIP-2 form (`"eip155:8453"`) — with the token list for each.
Both spellings are accepted everywhere a network is named.

### x402_settle

**This moves real funds and cannot be undone.** Call `x402_verify` first.

**A tool call cannot set headers**, and that is the one real gap between this
door and the HTTP one — a gap in *capability*, never in privilege. The input
that mattered got an argument instead: `idempotencyKey` is lifted out of the
body and sent as the `Idempotency-Key` header, so a retry after an ambiguous
failure settles once and not twice. Send a fresh unguessable value per payment
(a UUIDv4 is the right shape) and reuse it on every retry *of that payment*.
Keys share one namespace across all callers, so a predictable one like
`retry-1` can be claimed by somebody else's settle first and yours is refused
with `409`. The v2 `PAYMENT-SIGNATURE` header transport has no MCP equivalent;
put the payload in the body.

## A failed call is a result, not a protocol error

A 4xx/5xx from the underlying REST call comes back as a normal result with
`isError: true` carrying the facilitator's own body verbatim — not as a JSON-RPC
error, which most clients render as an opaque "internal error" and would hide
`invalid signature` behind. A JSON-RPC error is reserved for a call that never
reached a tool:

```json
{"jsonrpc":"2.0","id":8,"error":{"code":-32601,
  "message":"unknown tool: nope",
  "data":{"tools":["x402_supported","x402_accepts","x402_verify","x402_settle"]}}}
```

## The rate limit is shared with /verify and /settle

This endpoint has no budget of its own: it draws on the *same* per-IP bucket as
`POST /verify` and `POST /settle`, because it is the same handlers being called.
Every response carries `x-ratelimit-limit` and `x-ratelimit-remaining`; a
refusal is a `429` with `retry-after` and a JSON body. Throttle on the headers,
not on the failure.

## Measured traps

- **Both Accept types or nothing.** One of the two alone is a `406` with no
  hint, on a server that never streams.
- **A tool call carries no headers.** Anything the HTTP door reads from a header
  has either an argument here or no equivalent at all.
- **An ambiguous settle is not a failed settle.** A timeout means the
  transaction may already be on its way. Retry with the same `idempotencyKey`.
- **`isValid: false` is not always permanent.** A bad signature is; an
  unreachable RPC is not. Read `errorReason`.
- **Do not hard-code a network count** from this document or any other.
  `x402_supported` is the only answer that is true today.
- **`GET /mcp` is not the server.** If a client "connected" but no tool ever
  appears, check that it is POSTing.
- **`serverInfo.version` is stamped at runtime**, not written in any file.

## Everything next to this server

- MCP server card: `/.well-known/mcp/server-card.json`
- Agent manual, the same four calls over HTTP: `/skill.md`
- How paying works: `/auth.md`
- OpenAPI: `/openapi.json` (Swagger UI at `/docs`)
- Networks and schemes right now: `/supported`
- LLM context: `/llms.txt`, `/llms-full.txt`
- Source: <https://github.com/UltravioletaDAO/x402-rs>
