# Handoff — Execution Market: listing `/api/v1/tasks` in the curated Bazaar

**From**: Facilitator / Curated Bazaar workstream (x402-rs)
**To**: Execution Market team (and the wider swarm)
**Date**: 2026-07-24 · **Status**: BLOCKED on EM-side decisions — nothing to do in x402-rs
**Facilitator version at time of writing**: v1.55.2

## TL;DR

The Ultravioleta Bazaar is now curated: junk is rejected at ingest, dead endpoints
are swept and quarantined, and our own products rank first. **Execution Market is
listed and pinned `first_party`** via its MCP endpoint. What is NOT listed is the
REST endpoint `POST https://api.execution.market/api/v1/tasks`, because it is not
a fixed-price payable and listing it would require inventing payment terms.

We need three decisions from the EM team (§4) before it can be listed accurately.

## 1. Current state (verified live 2026-07-24)

| Item | Status |
|---|---|
| `https://mcp.execution.market/mcp/` | **LISTED**, `source: self_registered`, tier `first_party`, label "Execution Market" |
| `https://api.execution.market/api/v1/tasks` | **NOT listed** — see §2 |
| EM ERC-8004 identity | agent id **2106** on Base, registry `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432`, owner `0xD3868E1eD738CED6945A574a7c769433BeD5d474` (verified via `ownerOf`) |
| EM `/.well-known/x402` | Live, declares `facilitator: https://facilitator.ultravioletadao.xyz` (us), 9 EVM networks + Solana, escrow operators per chain |

The facilitator now annotates curated listings with a `curation.verification`
object sourced from the ERC-8004 registries. On-chain attestation **writes** are
implemented but deliberately **disabled** (`ENABLE_BAZAAR_ATTESTATIONS=false`) —
that is a separate backlog item on our side, not a blocker for EM.

## 2. Why `/tasks` is not listable as-is

Three findings, each verified against EM's own code and live API:

1. **It is a bounty model, not a fixed price.** EM's own manifest says it plainly:
   `endpoints.tasks.create.pricing.model = "bounty"` — *"Agent sets bounty amount
   (min $0.01 USDC). Paid at task completion, not at creation."* The publisher
   chooses the amount; the platform adds a 13% fee (`fees.platformFeeBps: 1300`).
   A bazaar listing advertises a concrete `(amount, payTo)` pair — there isn't one.

2. **The 402 body carries no `payTo`.** It is a bespoke v1-style error object
   (`required_amount_usd`, `bounty_usd`, `platform_fee_percent`, `x402_info`), not
   an x402 v2 `accepts[]` array. The settlement recipient is resolved server-side
   from `EM_SETTLEMENT_ADDRESS` / `WALLET_PRIVATE_KEY`, so no client-visible
   recipient exists to publish.

3. **Payment is not the first gate.** The real order is:
   `ERC-8128 wallet auth → request body validation → ERC-8004 identity → 402`.
   An anonymous caller never reaches the payment challenge.

We will not publish invented terms (project rule: never fabricate addresses or
prices), so `/tasks` stays unlisted until §4 is resolved.

## 3. Reference: reaching EM's 402 with a wallet (verified working)

Useful for anyone integrating against EM. ERC-8128 = RFC 9421 HTTP Message
Signatures + EIP-191 `personal_sign`. A **fresh ephemeral key passes auth** (it
then hits the ERC-8004 identity gate, which is the expected behavior).

```
1. GET  {api}/api/v1/auth/erc8128/nonce            -> single-use nonce (5 min TTL)
2. Build the RFC 9421 signature base over:
     "@method", "@authority", "@path" [, "@query"], "content-digest"
   plus "@signature-params": (...);created=<now>;expires=<now+300>;
     nonce="<nonce>";keyid="erc8128:8453:<lowercase 0x address>"
3. Sign the base with EIP-191 personal_sign
4. Send headers:
     Signature:       eth=:<base64(65-byte sig)>:
     Signature-Input: eth=(...);created=...;expires=...;nonce="...";keyid="..."
     Content-Digest:  sha-256=:<base64(sha256(exact body bytes))>:
```

- keyid chain **must** be `8453` (Base).
- `Content-Digest` is mandatory whenever there is a body, and must cover the exact
  bytes on the wire.
- Reference implementation: `em-plugin-sdk/em_plugin_sdk/erc8128.py`
  (`sign_request`, `fetch_nonce`). A minimal wallet adapter only needs
  `get_address()` + `sign_message()`.

Observed responses while probing: `401` (unsigned) → `422` (signed, empty body) →
`403 identity_required` (signed, valid body, no ERC-8004 identity). No task was
created and no funds moved at any point.

## 4. What we need from the EM team

**Decision A — should `/tasks` be discoverable at all?**
It is identity-gated and bounty-priced, so it is arguably not a "browse and pay"
resource. Option: list only the MCP endpoint (status quo, accurate) and leave the
REST API documented but unlisted.

**Decision B — if yes, what terms do we publish?**
The bazaar needs a concrete `accepts[]`. Realistic options:
- **Minimum-viable listing**: advertise the floor (`$0.01` USDC + 13% fee) with a
  description making the bounty model explicit. Requires EM to confirm the
  settlement `payTo` per chain that a publisher's payment actually goes to.
- **Discovery-only listing**: list it as `type: http` with `accepts: []` — but our
  own ingestion filter now rejects unpayable resources, so this would need a
  deliberate exception. Not recommended.

**Decision C — upgrade the 402 to x402 v2?**
If `POST /api/v1/tasks` returned a standard v2 `accepts[]` array (even with a
per-request computed amount), it would be listable and machine-consumable by any
x402 client, not just EM's SDK. This is the cleanest fix and benefits EM beyond
our bazaar.

## 5. Adjacent items owned by other teams

- **MeshRelay**: its landing meta tag `agent:payments-endpoint` points at
  `https://api.meshrelay.xyz/turnstile`, which 404s. The real endpoint is
  `/payments/access/{channel}`. All 7 premium channels are registered and pinned
  `first_party`. → MeshRelay owner: fix the meta tag.
- **402Milly**: its live `/purchase` 402 advertises `supportedChains` including
  `998` and `1301`, which look wrong (HyperEVM mainnet is `999`, Unichain is
  `130`). It also answers in v1-style JSON rather than a v2 `accepts[]` array, and
  its bazaar entry omits the non-EVM rails (Solana/NEAR/Stellar/Algorand/Sui) it
  actually supports. → 402Milly owner: fix chain IDs, upgrade to v2, extend the
  listing.

## 6. Facilitator-side API the swarm can use today

- `GET /discovery/resources` — supports `q`, `network`, `source`,
  `sourceFacilitator`, `category`, `provider`, `tag`, `health`
  (`alive|degraded|auth_gated|quarantined|unknown|unprobeable|any`) and `tier`
  (`first_party|vip|verified|listed`). Ordered tier → liveness → recency.
- `GET /discovery/stats` — catalog totals by source / facilitator / network /
  tier / health.
- `GET /bazaar` — the visual explorer.
- `POST /discovery/register` — self-registration (validated; ~5 req/min per IP).
- Full schema: `GET /docs` (Swagger UI) or `/api-docs/openapi.json`.

Contact: the facilitator repo (`x402-rs`), plan set under `docs/plans/bazaar/`.
