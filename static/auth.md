# auth.md

You are an agent. This document describes how to authenticate against the Ultravioleta
DAO x402 payment facilitator at `https://facilitator.ultravioletadao.xyz/`.

## The short answer: you do not

**The facilitator has no accounts, no API keys, no OAuth, and no registration.** There
is no client_id to obtain, no token to refresh, and no identity to prove before
calling it. `POST /verify` and `POST /settle` are open to anyone, and so is every read
endpoint on this host.

That is deliberate, and it follows from what the service is. A facilitator does not
sell anything — it checks a signature that someone else's buyer produced and lands the
resulting transfer on-chain. The thing being authorized is the *payment*, and the
payment carries its own authorization: the payer's signature over an EIP-3009
`transferWithAuthorization` (or the equivalent primitive on Solana, NEAR, Stellar,
Sui, Algorand and XRPL). The facilitator verifies that signature against the token's
EIP-712 domain, the nonce, the amount and the validity window. Nothing about *your*
identity as the caller changes the answer.

So there is nothing to send. No `Authorization` header, no `X-API-Key`, no cookie.

### The MCP endpoint is the same door

`POST /mcp` authenticates nobody either. It carries no OAuth flow, no session
token and no client registration — the `/.well-known/oauth-protected-resource`
document above describes it as well as it describes the HTTP routes. Its four
tools are dispatched through the very same handlers as `/verify`, `/settle`,
`/supported` and `/accepts`, so an MCP client holds exactly the privilege an HTTP
client holds and no more. In particular `x402_settle` settles a payment the payer
signed; it does not let a caller spend anything of their own or of ours.

That parity is of *privilege*, not of capability. A tool call cannot set headers,
so a few HTTP-only inputs have no MCP equivalent — the v2 `PAYMENT-SIGNATURE`
transport among them. The one that mattered is exposed as an argument instead:
`x402_settle` takes an optional `idempotencyKey`, lifted out of the body and sent
as `Idempotency-Key`, so an MCP client can ask for exactly-once on a retry.

The transport has one requirement worth knowing before you write a client by
hand: `Accept` must name **both** `application/json` and `text/event-stream`, or
the request is refused with `406`.

## This service does not charge

`/verify` and `/settle` never answer `402 Payment Required`. The facilitator takes no
fee and pays the settlement gas from its own wallets. If you receive a `402` from this
host, something is wrong — report it rather than paying it.

If you are looking for the x402 *payment* flow, you are one layer up: the seller's API
issues the 402 challenge, and you point your x402 client at this facilitator to have
the resulting authorization verified and settled. See `/skill.md` for that flow and
`/.well-known/x402` for the discovery document.

## What is actually enforced

### Rate limits, per IP

The facilitator applies a GCRA (token bucket) limit per client IP, taken from
`X-Forwarded-For` / `X-Real-IP` / `Forwarded` before falling back to the peer address.
Exceeding it returns **429**, never 401 or 403.

| Route group | Sustained | Burst |
|---|---|---|
| `POST /verify`, `POST /settle`, `POST /mcp` | 1 token every 2s (about 30 req/min) | 30 |
| `POST /discovery/register` | 1 token every 12s | 250 |
| Bazaar reads (`/discovery/resources`, `/discovery/stats`) | 1 token every 200ms | 120 |

A `429` is a back-off signal, not a rejection of your request's contents. Retry with
the delay the limiter implies; do not re-sign the payment.

### CORS

`Access-Control-Allow-Origin: *`, methods `GET` and `POST`. Browser-side x402 clients
can call the facilitator directly.

### Admin routes, which are not for you

A small number of destructive routes are gated by a static bearer token held by the
operator — `POST /feedback/revoke` (ERC-8004 reputation) and the Bazaar admin routes
(`DELETE /discovery/resources`, `POST /discovery/admin/suppress`,
`POST /discovery/admin/release`). They are **fail-closed**: when the operator has not
configured a token, the route answers `404`, indistinguishable from a route that does
not exist. There is no way for an agent to obtain one of these tokens, and no
self-service path that issues them. Treat those routes as absent.

## Identity, when you want one anyway

You do not need an identity to use the facilitator, but the stack it belongs to has
one, and the facilitator serves it:

- **ERC-8004 Trustless Agents** — on-chain agent identity and reputation, live on 12
  mainnets (21 networks counting testnets). `GET /identity/{network}/{agentId}`,
  `GET /identity/{network}/owner/{address}`, `GET /reputation/{network}/{agentId}`.
- `POST /register` mints an agent identity. It spends gas, so it carries the tight
  write-route rate limit; pass `Prefer: respond-async` to get a `202` and a `jobId` you
  poll at `GET /register/status/{jobId}` instead of holding the connection open.
- `POST /feedback` records reputation, and since v1.74.0 the payment proof attached to
  it is verified server-side against the chain. Authorship matters there: on the plain
  `/feedback` path the registry records the *facilitator* as the author. To be recorded
  as the rater yourself, use the prepare/submit pairs — `POST /feedback/evm/prepare`
  then `POST /feedback/evm/submit` (EIP-7702 relayed, served only where a verified
  delegate is deployed), or `POST /feedback/solana/prepare` then
  `POST /feedback/solana/submit` (you sign as `client`, the facilitator stays fee payer).

`GET /identity/{network}/owner/{address}` distinguishes **404** ("this address owns no
agent") from **503** ("the lookup reached no verdict", carrying `"retryable": true`).
Do not collapse them: persisting "not registered" from a 503 turns a transient RPC
failure into a permanent wrong answer, and on a registration path it mints a duplicate
agent for someone who already has one.

## Machine-readable form

RFC 9728 metadata is published at `/.well-known/oauth-protected-resource`. It declares
an empty `authorization_servers` array on purpose: the document exists so an
OAuth-capable agent can discover that this resource is **not** OAuth-protected,
without having to try and fail.

## Links

- Agent manual: https://facilitator.ultravioletadao.xyz/skill.md
- x402 discovery: https://facilitator.ultravioletadao.xyz/.well-known/x402
- OpenAPI: https://facilitator.ultravioletadao.xyz/openapi.json
- Operator: Ultravioleta DAO — https://ultravioletadao.xyz
