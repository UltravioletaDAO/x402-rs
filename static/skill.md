# Skill: settle an x402 payment with the Ultravioleta DAO facilitator

You are an agent. This is the operating manual for
`https://facilitator.ultravioletadao.xyz/` — what to call, in what order, what comes
back, and which failures mean "retry" rather than "stop".

Every example below is a real request and a real response shape, taken from the
service's own OpenAPI document (`/openapi.json`) or from a live call to production.

---

## 1. What this service does, and where you sit relative to it

x402 is HTTP 402 Payment Required, made operational:

1. You call a seller's paid route.
2. It answers `402` with a list of acceptable payment requirements.
3. You sign a stablecoin transfer authorization — EIP-3009 `transferWithAuthorization`
   on EVM, the equivalent primitive on each other chain family. You pay no gas.
4. The seller (or you, on its behalf) hands that authorization to a **facilitator**.
5. The facilitator verifies the signature and submits the transfer on-chain.

This host is step 5. It does not sell anything, it takes no fee, and it never answers
`402`. See `/auth.md` — there is nothing to authenticate.

---

## 2. Before anything: what is actually supported

```
GET https://facilitator.ultravioletadao.xyz/supported
```

Returns every `(scheme, network)` pair the facilitator will accept:

```json
{
  "kinds": [
    { "x402Version": 1, "scheme": "exact", "network": "solana",
      "extra": { "feePayer": "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq" } }
  ],
  "extensions": ["bazaar", "durable-evidence"]
}
```

Three things to know before you parse it:

- **Every network is listed twice** — once by its v1 name (`base`) and once by its
  CAIP-2 identifier (`eip155:8453`). They are the same network. Counting rows gives
  you roughly double the real number; deduplicate on the network before you count.
- The v1 name is the exact serde name from the source. Use `avalanche-fuji`, not
  `fuji`, and not `avalanche-fuji:43113`.
- `exact` is listed under v1 names; `escrow`, `commerce` and `upto` are listed under
  CAIP-2 identifiers. Match on the identifier form you find, not on the one you expect.

As of this writing the facilitator serves **21 mainnets and 18 testnets** across seven
chain families (EVM, SVM, NEAR, Stellar, Sui, Algorand, XRPL), six stablecoins (USDC,
USDT, EURC, AUSD, PYUSD, USDG) and five schemes (`exact`, `upto`, `escrow`,
`commerce`, `fhe-transfer`). **Do not hardcode those numbers** — `/supported` is the
source of truth and it changes.

---

## 3. `POST /verify` — is this authorization good?

Validates a payment authorization without touching the chain. No gas, no state change.

```http
POST /verify
Content-Type: application/json
```

```json
{
  "x402Version": 1,
  "paymentPayload": {
    "signature": "0x...",
    "payload": {
      "scheme": "exact",
      "network": "base",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "from": "0x...",
      "to": "0x...",
      "amount": "1000000",
      "validAfter": 1700000000,
      "validBefore": 1700100000,
      "nonce": "0x..."
    }
  },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "base",
    "maxAmountRequired": "1000000",
    "payTo": "0x...",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  }
}
```

What the facilitator checks: payload structure, the EIP-712 signature, nonce validity,
the amount against `maxAmountRequired`, the `validAfter`/`validBefore` window, and that
the token and network are supported.

**A rejected payment is still HTTP 200.** The verdict lives in the body:

```json
{ "isValid": false, "invalidReason": null, "payer": "0x0000000000000000000000000000000000000001" }
```

A valid one answers `{"isValid": true}`. Branch on `isValid`, not on the status code.
A `400` means the *request* was malformed — you sent something the facilitator could
not read — which is a different bug from a payment that does not check out.

---

## 4. `POST /settle` — put it on-chain

Same request body as `/verify`. The facilitator re-verifies, then calls
`transferWithAuthorization` on the token contract and returns the transaction hash.

Success:

```json
{ "success": true, "transaction": "0x...", "network": "base", "payer": "0x..." }
```

Failure:

```json
{ "success": false, "errorReason": "insufficient_balance", "payer": "0x...", "network": "base" }
```

**Settlement is not idempotent from your side and a timeout is not a failure.** If the
connection drops after you sent `/settle`, the transaction may still land. Do not
re-sign and re-send blindly: re-check on-chain, or look for the operation on `/events`
or `/transactions`, before deciding it did not happen.

### The other schemes, on the same endpoint

- **`upto`** — you sign a Permit2 authorization for a *maximum*; the seller settles the
  amount actually used, which must be at most that maximum. If the actual amount is
  zero, no transaction is submitted at all. Only advertised where the Permit2 proxy is
  deployed; check `/supported`.
- **`escrow` / `commerce`** — two names for the same x402r two-phase flow. The `action`
  field drives it: `authorize` (default, locks funds, needs the ERC-3009 signature),
  `release` (sends the locked funds to the receiver, no signature), `refundInEscrow`
  (returns them to the payer, no signature). Query the current state with
  `POST /escrow/state`.
- **`fhe-transfer`** — experimental, one testnet.

---

## 5. `POST /accepts` — build the 402 challenge

If you are the *seller*, this fills in the payment requirements you are about to hand
a buyer, enriching them with the facilitator's fee payer, token list and escrow data.
Faremeter middleware calls it for you.

Request:

```json
{
  "x402Version": 1,
  "accepts": [{
    "scheme": "exact", "network": "base",
    "maxAmountRequired": "10000",
    "resource": "https://example.com/x", "description": "demo",
    "mimeType": "application/json",
    "payTo": "0x0000000000000000000000000000000000000002",
    "maxTimeoutSeconds": 60,
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  }]
}
```

Response (live, trimmed):

```json
{
  "x402Version": 1,
  "accepts": [{
    "scheme": "exact", "network": "base", "maxAmountRequired": "10000",
    "payTo": "0x0000000000000000000000000000000000000002",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "extra": { "tokens": [
      { "token": "usdc", "address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "decimals": 6 },
      { "token": "eurc", "address": "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42", "decimals": 6 }
    ] }
  }],
  "error": ""
}
```

Omitting the `accepts` array is a `400` with
`{"error": "Missing or invalid 'accepts' array"}`.

---

## 6. The trap that costs the most time: EIP-712 domain names

The same stablecoin uses **different EIP-712 domain names on different chains**, and a
wrong domain produces a signature that verifies against nothing.

| Token | Usual name | Exceptions |
|---|---|---|
| USDC | `"USD Coin"` — including Base **mainnet** | `"USDC"` on Celo, HyperEVM, Unichain, Monad and most `-sepolia`/`-testnet` variants (Base Sepolia is `"USDC"`) |
| EURC | `"Euro Coin"` (Ethereum, Avalanche) | `"EURC"` on Base |

The name can **flip between a chain's mainnet and its testnet** (HyperEVM mainnet is
`"USDC"`, HyperEVM testnet is `"USD Coin"`), and bridged variants differ again
(`"Bridged USDC(XDC)"`, `"Bridged USDC (SKALE Bridge)"`). Never infer it.

The facilitator resolves the domain in this order:

1. Its own static table of known deployments — **this wins**, and if you send a
   different value it is logged as a warning and ignored.
2. `paymentRequirements.extra.name` / `.version` — used only for tokens *not* in that
   table.
3. An on-chain `token.name()` / `token.version()` call, as a last resort.

So: for a token the facilitator already knows (USDC, EURC, AUSD…), you do not need to
send `extra` at all. For anything else, you **must**:

```json
{ "paymentRequirements": { "asset": "0x...", "extra": { "name": "EURC", "version": "2" } } }
```

Second trap, cheaper but just as common: **`validAfter` and `validBefore` are Unix
seconds, not milliseconds.**

---

## 7. Watching what happened

- `GET /events` — server-sent events, one message per verify/settle. Returns `503`
  with `Retry-After` when the subscriber cap is reached.
- `GET /transactions` — recent recorded operations. `limit` is capped at 200.
- `GET /api/stats` — aggregated totals per network and asset.
- `GET /api/stats/history` — settlement history reconstructed from the chain. This is
  a *different claim* from `/api/stats`, which is what the facilitator measured; every
  row carries a `source`.

**None of these is a ledger — the chain is.** Rows are written fire-and-forget after
settlement resolves, so an unreachable store loses rows and never blocks a payment.
Quote a number from here only with that caveat attached. And while failure publishing
is off (the default), a 100% success rate means "no failures were recorded", not "no
failures occurred".

---

## 8. Reputation and identity (ERC-8004)

Live on 12 mainnets, 21 networks in total. See `/auth.md` for the authorship rules —
they are the part integrators get wrong.

- `GET /identity/{network}/{agentId}` · `GET /identity/{network}/owner/{address}`
- `GET /reputation/{network}/{agentId}`
- `POST /register` (spends gas; send `Prefer: respond-async` for a `202` + `jobId`,
  then poll `GET /register/status/{jobId}`)
- `POST /feedback`, and the prepare/submit pairs that record *you* as the rater

Two behaviours that surprise people: feedback without a `score` is recorded but never
scored (`had_impact=false`, and it is not retroactive), and the Solana program forbids
self-feedback.

---

## 9. Error handling, in one table

| You see | It means | Do |
|---|---|---|
| `200` + `isValid: false` | the payment does not check out | fix the payload; do not retry as-is |
| `200` + `success: false` + `errorReason` | settlement was attempted and refused | read `errorReason`; most are terminal |
| `400` | your request body was malformed | fix the request, not the payment |
| `404` on an admin route | the operator configured no token; the route is closed | treat as absent |
| `404` from `/identity/.../owner/...` | that address owns no agent | a real negative answer |
| `503` + `"retryable": true` | the lookup reached **no verdict** | retry; never persist this as "not registered" |
| `503` on `/events` | subscriber cap reached | honour `Retry-After` |
| `429` | per-IP rate limit (about 30 req/min on verify/settle) | back off; do not re-sign |
| timeout on `/settle` | unknown — the tx may have landed | check the chain before retrying |

---

## 10. MCP: the same four calls, as tools

This facilitator is also an MCP server. Same host, same rate limit, same
handlers -- an MCP tool call is dispatched through the very same code path as
the HTTP request it names, so nothing here is a second implementation that
could answer a different truth.

- **Endpoint:** `https://facilitator.ultravioletadao.xyz/mcp`
- **Transport:** Streamable HTTP, stateless. `POST` only; `GET /mcp` answers
  `405` with a JSON body, because there is no server-initiated SSE stream to
  open and no session id to keep.
- **Server card:**
  `https://facilitator.ultravioletadao.xyz/.well-known/mcp/server-card.json`
- **Authentication:** none, exactly as in `/auth.md`. The MCP door grants no
  privilege the HTTP door does not: the payer's signature is still the only
  authority.

| Tool | Is | Moves money |
|---|---|---|
| `x402_supported` | `GET /supported` | no |
| `x402_accepts` | `POST /accepts` | no |
| `x402_verify` | `POST /verify` | no |
| `x402_settle` | `POST /settle` | **yes, irreversibly** |

The arguments of each tool are the JSON body of the request it stands for, and
the result is that request's response body verbatim, in a single text content
block. A non-2xx answer comes back as a tool error (`isError: true`) carrying
the facilitator's own message -- not as a JSON-RPC error, which most clients
render as an opaque "internal error" and would hide `invalid signature` behind.

### Handshake

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

The negotiated `protocolVersion` is the highest both sides know; this server
supports `2024-11-05` through `2026-07-28`.

### Calling a tool

```bash
curl -sS https://facilitator.ultravioletadao.xyz/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"x402_supported","arguments":{}}}'
```

The `result.content[0].text` is the exact JSON body of `GET /supported`.

`x402_verify` and `x402_settle` take the same envelope the REST routes take:

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
  "name":"x402_verify",
  "arguments":{
    "x402Version":1,
    "paymentPayload":{ "...": "as in section 3" },
    "paymentRequirements":{ "...": "as in section 3" }}}}
```

Read sections 3 and 4 before calling either: everything they say about
`isValid`, `errorReason`, the EIP-712 domain-name trap and which failures are
retryable is true over MCP too, because it is the same handler answering.

---

## 11. Libraries

- `uvd-x402-sdk` — the house SDK, on npm
  (https://www.npmjs.com/package/uvd-x402-sdk) and PyPI
  (https://pypi.org/project/uvd-x402-sdk/). Point it at this facilitator's URL.
- `x402-axum` — Rust middleware that prices a route and calls this facilitator.
- `x402-reqwest` — Rust client that answers a 402 challenge for you.

Source: https://github.com/UltravioletaDAO/x402-rs
Operator: Ultravioleta DAO — https://ultravioletadao.xyz
