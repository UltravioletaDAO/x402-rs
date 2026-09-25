# Skill: settle an x402 payment with the Ultravioleta DAO facilitator

You are an agent. This is the operating manual for
`https://facilitator.ultravioletadao.xyz/` — what to call, in what order, what comes
back, and which failures mean "retry" rather than "stop".

Every example below is a real request and a real response shape, taken from the
service's own OpenAPI document (`/openapi.json`) or from a live call to production.

Language (idioma): this manual is English (`en`) and has no Spanish translation, like
`/llms.txt`, `/index.md` and `/auth.md`. The Spanish version exists only for the human
pages (`/`, `/x402`, `/dx402`, `/erc8004`, `/bazaar`, `/networks`, `/mcp`, `/integrar`,
`/stats`, `/events/live`), at the same URL as the English, behind each page's EN/ES
selector. There is no `/es/` path.

---

## 1. What this service does, and where you sit relative to it

x402 is HTTP 402 Payment Required, made operational:

1. You call a seller's paid route.
2. It answers `402` with a list of acceptable payment requirements.
3. You sign a token transfer authorization — EIP-3009 `transferWithAuthorization`
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

Read each row's `x402Version`, `scheme`, `network`, `networkAliases` and `extra`.
Networks can have legacy names and CAIP-2 aliases; rows also repeat by scheme.
Do not count aliases as distinct networks. **Hedera has no v1 entry**: native
payments use `hedera:mainnet` or `hedera:testnet` with x402 v2 only. Arc publishes
both its legacy names and `eip155:5042` / `eip155:5042002`.

The supported chain families are EVM, SVM, NEAR, Stellar, Sui, Algorand, XRPL and
Hedera. Tokens and schemes vary by network; query `/supported` rather than
assuming a global scheme or extension applies to each network.

---

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
escrow, `upto` or Gateway on that network; ERC-8004 is served on both Arc networks,
not on Hedera. Native Hedera also rejects
durable-evidence and other unsupported extensions.

**Native Hedera v2 request:**
```json
{
  "x402Version": 2,
  "paymentPayload": {
    "x402Version": 2,
    "resource": {
      "url": "https://example.com/protected",
      "description": "One API call",
      "mimeType": "application/json"
    },
    "accepted": {
      "scheme": "exact",
      "network": "hedera:testnet",
      "asset": "0.0.429274",
      "amount": "1000",
      "payTo": "0.0.10576387",
      "maxTimeoutSeconds": 180,
      "extra": {
        "feePayer": "0.0.10576385"
      }
    },
    "payload": {
      "transaction": "BASE64_BUYER_SIGNED_TRANSACTION_LIST"
    }
  },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "hedera:testnet",
    "asset": "0.0.429274",
    "amount": "1000",
    "payTo": "0.0.10576387",
    "maxTimeoutSeconds": 180,
    "extra": {
      "feePayer": "0.0.10576385"
    }
  }
}
```

The base64 value is a placeholder for the buyer-signed protobuf TransactionList;
build it with the Hedera client. The inner `accepted` and outer
`paymentRequirements` must match exactly. The generic EVM convenience rule about
omitting inner `accepted` does not apply to native Hedera. Unknown requirements,
extra fields and unsupported extensions are rejected. The buyer signs every frozen
node variant; the facilitator adds its signature only after inspection. Retry the
same payload and transaction ID after uncertainty. A successful settlement returns
a native ID such as `0.0.10576385@1789609553.483480778`; it is not an EVM hash.

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
    "x402Version": 1,
    "scheme": "exact",
    "network": "base",
    "payload": {
      "signature": "0x111111111111111111111111111111111111111111111111111111111111111122222222222222222222222222222222222222222222222222222222222222221b",
      "authorization": {
        "from": "0x0000000000000000000000000000000000000001",
        "to": "0x0000000000000000000000000000000000000002",
        "value": "1000000",
        "validAfter": "1700000000",
        "validBefore": "1700100000",
        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
      }
    }
  },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "base",
    "maxAmountRequired": "1000000",
    "resource": "https://example.com/protected",
    "description": "One API call",
    "mimeType": "application/json",
    "payTo": "0x0000000000000000000000000000000000000002",
    "maxTimeoutSeconds": 60,
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  }
}
```

**That body is runnable, not a sketch.** The signature and the nonce are well-formed
placeholders -- `r` all `0x11`, `s` all `0x22`, a one-valued nonce -- not a real
authorization, so copied verbatim it answers HTTP 200 with `"isValid": false`. Swap in
your signed values and it answers on the merits. Curl it before you write any code:

```bash
curl -sS -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H 'Content-Type: application/json' -d @body.json
```

Five things in that shape are load-bearing, and each one has produced a `400`:

- `paymentPayload` carries its OWN `x402Version`, `scheme` and `network` at its root.
  They are not inherited from the envelope.
- The signed data sits under `payload.authorization`, not directly under `payload`.
- The amount field inside the authorization is `value`. `amount` is the name in the
  *requirements* (`maxAmountRequired`), not in the authorization.
- `validAfter` and `validBefore` are **strings**, not numbers. `1700000000` is
  rejected; `"1700000000"` is accepted. So are `value` and `maxAmountRequired`.
- `paymentRequirements` needs `resource`, `description`, `mimeType` and
  `maxTimeoutSeconds`. They have no defaults; omit one and the whole body fails to
  parse.

`network` may be written either way, in both objects: `"base"` or `"eip155:8453"`.
That is what lets an offer taken straight out of `/discovery/resources` -- which is
CAIP-2 -- be paid without rewriting it. Mixing the two spellings inside one body is
also accepted, though matching the offer is the sane thing to do.

### The same payment in the x402 v2 shape

If your body says `"x402Version": 2`, the envelope is a **different shape**, not the
one above with a `2` in it. There is no `paymentRequirements` in v2. The requirements
split in two: `resource` carries what is being sold, `accepted` carries what is being
charged.

```json
{
  "x402Version": 2,
  "paymentPayload": {
    "x402Version": 2,
    "payload": {
      "signature": "0x111111111111111111111111111111111111111111111111111111111111111122222222222222222222222222222222222222222222222222222222222222221b",
      "authorization": {
        "from": "0x0000000000000000000000000000000000000001",
        "to": "0x0000000000000000000000000000000000000002",
        "value": "1000000",
        "validAfter": "1700000000",
        "validBefore": "1700100000",
        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
      }
    }
  },
  "resource": {
    "url": "https://example.com/protected",
    "description": "One API call",
    "mimeType": "application/json"
  },
  "accepted": {
    "scheme": "exact",
    "network": "eip155:8453",
    "amount": "1000000",
    "payTo": "0x0000000000000000000000000000000000000002",
    "maxTimeoutSeconds": 60,
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  }
}
```

That is the **same payment** as the v1 body above — same signature, same chain, same
amount, same recipient — written the other way. The facilitator reduces both to one
internal request, and a test asserts that they land on the identical one.

What moved, field by field:

| v1 | v2 |
|---|---|
| `paymentRequirements` | `accepted` + `resource`, both at the top level |
| `paymentRequirements.maxAmountRequired` | `accepted.amount` |
| `paymentRequirements.resource` (a URL string) | `resource.url` |
| `paymentRequirements.description` / `.mimeType` | `resource.description` / `resource.mimeType` |
| `paymentPayload.scheme` / `.network` | `accepted.scheme` / `accepted.network` |

`accepted` takes exactly `scheme`, `network`, `asset`, `amount`, `payTo`,
`maxTimeoutSeconds` and an optional `extra`. Extra keys are ignored rather than
rejected, so a `402` offer that also carries `maxAmountRequired`, `resource`,
`description` or `mimeType` can be passed through unedited. `resource` needs all three
of `url`, `description` and `mimeType`.

**`accepted.network` is CAIP-2 only.** This is the one place the two spellings are
*not* interchangeable: the v1 `paymentRequirements.network` and
`paymentPayload.network` take `"base"` or `"eip155:8453"`, but `accepted.network`
takes `"eip155:8453"` and refuses `"base"`. Measured — a v2 body with `"base"` there
answers `400 data did not match any variant of untagged enum VerifyRequestEnvelope`.
An offer copied out of a `402` or out of `/discovery/resources` is already CAIP-2, so
this only bites if you rewrote it.

**On the duplication you may have seen.** Older facilitator builds *also* required
`resource` and `accepted` to be repeated **inside** `paymentPayload`, and refused the
body above with `400 data did not match any variant of untagged enum
VerifyRequestEnvelope`. That is fixed: the inner copy is optional. If you already send
it, keep sending it — the duplicated envelope is still accepted, byte for byte, and
reduces to exactly the same payment. If you do not, the facilitator fills the inner
pair in from the outer one.

```json
{ "paymentPayload": { "x402Version": 2, "resource": { "...": "same object" },
                      "accepted": { "...": "same object" }, "payload": { "...": "as above" } },
  "resource": { "...": "" }, "accepted": { "...": "" }, "x402Version": 2 }
```

`/settle` takes the v2 envelope on the same terms as `/verify`; the two endpoints share
one parser.

What the facilitator checks: payload structure, the EIP-712 signature, nonce validity,
the amount against `maxAmountRequired`, the `validAfter`/`validBefore` window, and that
the token and network are supported.

**A rejected payment is still HTTP 200.** The verdict lives in the body:

```json
{ "isValid": false, "invalidReason": "invalid_signature", "payer": "0x0000000000000000000000000000000000000001" }
```

A valid one answers `{"isValid": true}`. Branch on `isValid`, not on the status code.

`invalidReason` is a snake_case token naming the cause, and each cause has its own:

| Token | What to change |
|---|---|
| `invalid_signature` | The EIP-712 signature does not recover to `authorization.from` |
| `invalid_timing` | Now is outside the `validAfter` / `validBefore` window |
| `insufficient_funds` | The payer's on-chain balance is below the amount |
| `insufficient_value` | The signed `value` is below `maxAmountRequired` |
| `receiver_mismatch` | `authorization.to` is not the requirements' `payTo` |
| `invalid_network` | The network is unsupported, or the two halves disagree |
| `invalid_scheme` | The payload's `scheme` is not the requirements' `scheme` |
| `unexpected_settle_error` | Settlement failed for a reason none of the above covers |

Treat the list as open: a token you do not recognise still means "rejected", so
switch on it but keep a default arm. Before 2.13.0 this field was always `null` —
if you see `null`, you are talking to an older facilitator and the cause is not
recoverable from the response.
A `400` means the *request* was malformed — you sent something the facilitator could
not read — which is a different bug from a payment that does not check out.

---

## 4. `POST /settle` — put it on-chain

Same request body as `/verify`. The facilitator re-verifies, then calls
`transferWithAuthorization` on the token contract and returns the transaction hash.

Success:

```json
{ "success": true, "transaction": "0x...", "transactionHash": "0x...",
  "paymentId": "0x...", "network": "base", "payer": "0x..." }
```

The hash is emitted under three names — `transaction`, `transactionHash` and
`transaction_hash` — because clients in the wild read all three. They are one value.

`paymentId` is this payment's canonical identifier: `keccak256(caip2 ‖ txHash)`,
so it is reproducible by anyone holding the network and the hash. It is the key
DX402 evidence is stored under, so it is what you pass to
`/dx402/evidence/{paymentId}` and `/dx402/receipt/{paymentId}`.

Failure:

```json
{ "success": false, "errorReason": "insufficient_funds", "payer": "0x...", "network": "base" }
```

`errorReason` uses the same token vocabulary as `invalidReason` above.

**A timeout is not a failure.** If the connection drops after you sent `/settle`,
the transaction may still land. Do not re-sign and re-send blindly: re-check
on-chain, or look for the operation on `/events` or `/transactions`, before
deciding it did not happen.

**When the facilitator itself times out waiting, it hands you the hash.** As of
2.14.0, a transaction we broadcast and never saw confirmed answers `502` with:

```json
{ "error": "settlement_unconfirmed",
  "transaction": "0x...", "paymentId": "0x...", "retryable": false }
```

This is *not* `success: false` — it is not a verdict at all. The transaction may
be mined. `retryable` is `false` and means it: retrying re-signs a **fresh**
authorization for the same purchase, which is a new, perfectly valid payment the
token's own nonce check cannot stop, so a retry here is how you pay twice. The
hash is there to be **looked up**, and `paymentId` is the same identifier a
successful `/settle` prints, so once you find the transaction confirmed you can
tie the two together (and reach `/dx402/evidence/{paymentId}`). Before 2.14.0
this branch answered `contract_call_failed (ref: <uuid>)` with no hash at all.

Since 2.39.6 the same answer also covers a send whose own answer was lost (a
timeout, a dropped connection, a gateway error) and a node that says it already
holds the transaction, on every network family, each hash in its own chain's
encoding. NEAR, Stellar, Algorand, Sui and XRPL used to report that case as
`200` with `success: false` and no transaction.

**The rule behind it: `retryable: false` means "do not sign again".** Every
`/settle` failure produced after the transaction may have left carries
`"retryable": false` and no `Retry-After`, plus `transaction` and `paymentId`
when the facilitator knows them: `settlement_unconfirmed`,
`broadcast_uncertain`, `receipt_pending`, the `upto` / `escrow` / `refund`
schemes' `502`, a settle forwarded to the signing task whose answer was lost
(`reason: forward_unconfirmed`), and the receipt rail's failures after its send
latched. Look the transaction up and deliver if it settled; only one not found
after the chain's finality window is gone. A failure with neither
`retryable: false` nor a `transaction` was produced before anything was sent.

**Send an `Idempotency-Key` and the retry is safe.** Choose one opaque string per
intended purchase, keep it across retries and restarts, and send it as a header:

```
Idempotency-Key: 26dece19-37e0-431c-95d4-10b4e44fef98
```

- **Same key, same body** → the first response is replayed, carrying
  `Idempotent-Replayed: true`. No second transaction. Read that header rather
  than comparing bodies: after a restart you no longer hold the first one.
- **Same key, different body** → `409 idempotency_key_conflict`. This is the one
  that catches the dangerous mistake: re-signing an authorization with a fresh
  nonce and reusing the key means you are trying to pay twice for one purchase,
  and it is refused instead of settled.
- **Store unreachable** → `503 idempotency_store_unavailable`. It fails closed:
  no settlement happens that we could not have deduplicated.

Without a key there is nothing to deduplicate against, and a retry is simply a
second settle attempt. On EVM the token's own `authorizationState` still rejects
a replay of the *same* nonce, but nothing stops a *newly signed* one.

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
  "rejected": [],
  "error": ""
}
```

### What it could not serve, and why

`rejected` is always there -- empty when everything matched. It used to not
exist, and a requirement the facilitator could not serve was dropped in silence:
an unknown chain, a scheme we do not run there, and a body we could not read all
came back as the same `{"accepts": [], "error": ""}` with `HTTP 200`. There was
nothing to branch on.

```json
{
  "x402Version": 1,
  "accepts": [],
  "rejected": [{
    "index": 0,
    "scheme": "exact",
    "network": "cosmos:hub-4",
    "reason": "network_unknown",
    "detail": "`cosmos:hub-4` names no chain this facilitator knows..."
  }],
  "error": ""
}
```

`reason` is a closed set -- switch on it. `detail` is prose and may change.

| `reason` | What it means | Your move |
|---|---|---|
| `malformed` | `scheme` or `network` missing or not a string | Fix the requirement |
| `network_unknown` | That string names no chain, in either spelling | Read `/supported` |
| `scheme_unknown` | Not a scheme this facilitator implements | Read `/supported` |
| `network_unsupported` | A chain it knows but this deployment does not serve | Offer another chain |
| `scheme_unsupported_on_network` | Both known, but not that pair | `detail` names what IS served there |

**A negotiation where nothing matched is still `HTTP 200`** -- the request was
well formed, the answer is just empty. Branch on `accepts.length` and on
`rejected`, never on the status code.

Omitting the `accepts` array is a `400` with
`{"error": "Missing or invalid 'accepts' array"}`.

---

## 6. The trap that costs the most time: EIP-712 domain names

The same stablecoin uses **different EIP-712 domain names on different chains**, and a
wrong domain produces a signature that verifies against nothing.

| Token | Usual name | Exceptions |
|---|---|---|
| USDC | `"USD Coin"` — including Base **mainnet** | `"USDC"` on Celo, HyperEVM, Unichain, Monad and most `-sepolia`/`-testnet` variants (Base Sepolia is `"USDC"`) |
| EURC | `"Euro Coin"` (Ethereum, Avalanche) | `"EURC"` on Base and Arc |

The name can **flip between a chain's mainnet and its testnet** (Base mainnet is
`"USD Coin"`, Base Sepolia is `"USDC"`), and bridged variants differ again
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

Live on 13 mainnets, 23 networks in total. See `/auth.md` for the authorship rules —
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
| `503` + `"code": "overloaded"` | the task is at its ceiling of concurrent requests, for every caller; nothing was done | honour `Retry-After` (1 s) and resend the same request |
| `429` | per-IP rate limit (about 30 req/min on verify/settle) | back off; do not re-sign |
| `502` + `"error": "settlement_unconfirmed"` | we sent the tx, or may have, and never got a verdict | look up the `transaction` on chain; **never** retry (`retryable: false`) |
| any `5xx` + `"retryable": false` | the tx may already be on chain (`broadcast_uncertain`, `receipt_pending`, `forward_unconfirmed`, the alternative schemes, the receipt rail) | **never** sign again; look up `transaction` if present, else the payer's transfer |
| `502` + `"error": "upstream_rpc_unavailable"` | the node could not answer | honour `Retry-After`; the two `502`s are different — branch on `error` |
| timeout on `/settle` | unknown — the tx may have landed | check the chain before retrying |

Every refusal is JSON. A `4xx` or `5xx` carries `{"error": ..., "code": ...,
"hint": ...}` with `content-type: application/json` — including the ones the
framework used to answer with an empty body: a `405` for the wrong method (the
`Allow` header lists what the path accepts), a `404` for a path nothing serves,
and the `429` from the rate limiter itself. Branch on `code`, not on the prose
in `error`.

On `invalid_request_body` the `hint` is written for the version **your body
declares**, so read it: a body saying `"x402Version": 2` is told about `resource`
and `accepted`, a body saying `1` about `paymentRequirements`, and a body too
broken to say gets both shapes rather than a guess. It used to name the v1 fields
to everyone, which is how a correct v2 integration was talked into sending v1.

### Rate limits, and how to stay under them

Limits are per client IP, and they are reported on every response rather than
only on the refusal:

| Header | On | Meaning |
|---|---|---|
| `x-ratelimit-limit` | every rate-limited response, `200` included | the burst size of the bucket this route draws on |
| `x-ratelimit-remaining` | every rate-limited response, `200` included | tokens left in that bucket right now |
| `retry-after` | `429` | seconds to wait before retrying |
| `x-ratelimit-after` | `429` | the same number, under tower_governor's own name |

Read `x-ratelimit-remaining` and slow down before it reaches zero; that is the
whole reason it is on the `200`. The buckets are separate per surface, so
draining `/discovery/resources` does not cost you `/settle`, with one
deliberate exception: **`POST /mcp` shares the `/verify` and `/settle` bucket**,
because an `x402_settle` tool call costs the chain exactly what `POST /settle`
costs it.

A note on the arithmetic, because it reads backwards: the bucket refills one
token every N seconds, so "30 req/min on verify/settle" is a burst of 30 plus
one token every 2 seconds — not 30 tokens handed out each minute. A few routes
that spend no chain quota (`/health`, `/supported`, `/llms.txt` and the other
discovery documents) carry no limit at all and therefore no headers.

The values in force -- every bucket's period and burst, the ceiling of
concurrent requests behind the `503 overloaded`, and the ERC-8004 daily write
limit -- are published at `GET /config`.

---

## 10. MCP: the same four calls, as tools

This facilitator is also an MCP server. Same host, same rate limit, same
handlers -- an MCP tool call is dispatched through the very same code path as
the HTTP request it names, so nothing here is a second implementation that
could answer a different truth.

- **Endpoint:** `https://facilitator.ultravioletadao.xyz/mcp`
- **Transport:** Streamable HTTP, stateless. `POST` only. A `GET` on the same
  path is the human guide (HTML, or this same material as Markdown with
  `Accept: text/markdown`); a caller whose `Accept` names `application/json` or
  `text/event-stream` still gets the `405` naming POST, because there is no
  server-initiated SSE stream to open and no session id to keep.
- **Guide:** `https://facilitator.ultravioletadao.xyz/mcp`
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
block.

**The body is the only channel — a tool call cannot set headers.** The parity
with HTTP is one of *privilege*, not of capability: nothing here can move funds
an HTTP client could not, but a few HTTP-only inputs have no MCP equivalent. The
one that mattered has an argument instead: `x402_settle` takes an optional
`idempotencyKey`, lifted out of the body and sent as the `Idempotency-Key`
header, so a retry after an ambiguous error settles once and not twice. Send it
on every retry. The v2 `PAYMENT-SIGNATURE` header transport has no equivalent;
put the payload in the body. A non-2xx answer comes back as a tool error (`isError: true`) carrying
the facilitator's own message -- not as a JSON-RPC error, which most clients
render as an opaque "internal error" and would hide `invalid signature` behind.

### Handshake

Both `Accept` types are required — the Streamable HTTP transport answers `406`
without them, even though this server is stateless and always replies with JSON.

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

`x402_verify` and `x402_settle` take the same envelope the REST routes take.
Their `inputSchema` in `tools/list` spells that envelope out in full -- down to
`paymentPayload.payload.authorization.value` and the string-typed timestamps --
and carries the section 3 example verbatim under `examples`, so a client that
reads only the tool list has the whole contract:

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

Retrying a settle:

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
  "name":"x402_settle",
  "arguments":{
    "x402Version":1,
    "paymentPayload":{ "...": "as in section 3" },
    "paymentRequirements":{ "...": "as in section 3" },
    "idempotencyKey":"a-key-you-keep-for-this-payment"}}}
```

Same key and same payment returns the first result instead of settling again;
same key with a different payment is refused with `409`.

---

## 11. Libraries

- `uvd-x402-sdk` — the house SDK, on npm
  (https://www.npmjs.com/package/uvd-x402-sdk) and PyPI
  (https://pypi.org/project/uvd-x402-sdk/). Point it at this facilitator's URL.
- `x402-axum` — Rust middleware that prices a route and calls this facilitator.
- `x402-reqwest` — Rust client that answers a 402 challenge for you.

---

## 12. Durable evidence (DX402): the paid response, retrievable later

x402 delivers a paid response once and keeps nothing. DX402 (extension key
`durable-evidence`) lets the seller seal a copy of the response body to the payer's
own public key and anchor it here; the payer recovers it later with the wallet it
paid with. This facilitator is the notary and the index -- it signs an EIP-712
receipt and records a pointer and a hash. It is not in the response path, and in
`direct` mode it never holds the plaintext or any key that opens it.

**Check that it is on before relying on it.** DX402 is off unless the operator
enables it. `durable-evidence` appears in the `extensions` of `GET /supported` only
when this deployment can actually serve it. Without it the `/dx402/...` routes are
not registered at all, so every one of them answers the generic `404`
(`"code": "not_found"` when you ask for JSON). That means "no DX402 here", not "no
evidence for this payment".

**It can never fail a payment.** Settlement never calls into DX402. When evidence
cannot be produced, the seller's hook still delivers the body and says why in the
`X-Durable-Evidence` response header (base64url JSON, no padding) as
`{"skipped": "<reason>"}`: `too_large`, `busy`, `anchor_failed`, `no_payer_key`,
`disabled`, `not_selected`. Treat the set as open. `not_selected` is not a failure:
you paid for the offer without evidence.

**You opt in by paying the durable offer.** A seller that offers evidence lists the
same resource twice in its `402`, plain and with evidence, priced separately, and
names the evidence offers in `extensions["durable-evidence"].info.acceptIndexes`.
The offer you pay decides; a client that does not choose pays the plain one. Only
EVM offers can be told apart this way today, so on other families a seller lists
one offer per network. The offer's `retention` is `90d` by default, or `1y`, or
`permanent` -- and `permanent` is irrevocable: anchoring is publishing.

| Call | Returns |
|---|---|
| `GET /dx402/evidence/{paymentId}` | `pointer`, `contentHash`, `mode`, `retention`, `retentionUntil`, `receipt`, `verified`, `signed` |
| `GET /dx402/receipt/{paymentId}` | the signed EIP-712 receipt (domain `DX402 Evidence`, version `1`), verifiable offline |
| `GET /dx402/blob/{paymentId}` | the sealed ciphertext; unauthenticated on purpose, since only the payer's key opens it |
| `GET /dx402/stats` | anchors counted (a floor, not a ledger), backend, default retention, receipt signer |
| `POST /dx402/anchor` | the seller's call: metadata only, `201` when recorded |
| `POST /dx402/recover` | `501`: `direct` mode needs no recovery, the payer already holds the key |

`paymentId` is the value `/settle` returns (section 4): `keccak256(caip2 ‖ txHash)`,
`0x` + 64 hex digits. Any casing, with or without `0x`, resolves to the same record.

After decrypting, recompute `contentHash` over the **plaintext** and compare. That
check is what catches a seller that anchored something other than what it served;
`x402-reqwest`'s `recover` does it for you.

Errors are `{"error": "<code>", "retryable": <bool>}` -- branch on `error`:

| You see | It means | Do |
|---|---|---|
| `404` + `"code": "not_found"` | DX402 is not enabled on this deployment | read `/supported`; do not retry |
| `404` + `dx402_unknown_payment` | nothing is anchored for that id | a real negative; the seller may have skipped, so read `X-Durable-Evidence` |
| `410` + `dx402_evidence_expired` | it existed and its retention ran out | a different answer from `404`; do not retry |
| `503` + `dx402_store_unavailable` | the store did not answer (`retryable: true`) | retry later; never record it as "no evidence" |
| `200` + `"verified": false` | provisional: the payee has not proved the anchor is theirs | the bytes still decrypt and the hash still checks, but it is not proof of who produced them |

The anchoring side (`POST /dx402/anchor`, the payee signature, and its `409`/`422`
answers) belongs to the seller integration; the human guide is `/dx402`.

---

## 13. The Bazaar: finding x402-priced resources

The Bazaar is a catalog of x402-priced endpoints: registered here, seen settling
through this facilitator, crawled, or aggregated from other facilitators' feeds.
It takes no part in a transaction and moves no money. The human view is `/bazaar`.

### Reading it

```
GET /discovery/resources?q=weather&network=eip155:8453&limit=20
```

Returns `{"x402Version": 2, "items": [...], "pagination": {"limit", "offset",
"total"}}`. Each item carries `url`, `type`, `description`, `accepts` (CAIP-2 offers
you can pay as they are, see section 3), `source`, and the read-time annotations
`health`, `curation` and `priceFreshness`.

The parameters, and only these: `limit` (default 10, capped at 100), `offset`, `q`
(free text, at most 128 characters), `category`, `network` (CAIP-2), `provider`,
`tag`, `source` (`self_registered`, `settlement`, `crawled`, `aggregated`),
`sourceFacilitator`, `health` (`alive`, `degraded`, `auth_gated`, `quarantined`,
`unknown`, `unprobeable`, `any`), `tier` (`first_party`, `vip`, `verified`, `listed`).

- **An unknown parameter is a `400`, never ignored.** The body lists `supported`
  and, when the intent is obvious, a `hint` (`?search=` gets "did you mean q?").
  A filter that was silently dropped would look like one that matched everything.
- **Quarantined entries are hidden by default**; `health=any` shows everything.
  Order is tier (`first_party`, `vip`, `verified`, `listed`), then alive first,
  then most recently updated.
- **The catalog is a previous reading, not the offer.** `priceFreshness` is
  `fresh`, `stale`, `unknown` or `conflict`, and `priceRevalidation: "pending"`
  means the amounts beside it are the last reading. Before paying, call the
  resource and decide against the `402` it answers now.
- Page with `offset` until it reaches `pagination.total`. Reads draw on their own
  per-IP bucket (see `/auth.md`), separate from `/settle`.
- `GET /discovery/stats` -- aggregate catalog numbers, cached for 60 seconds.
- `GET /discovery/attestation/{hash}` -- a hosted ERC-8004 attestation body, keyed
  by `sha256(url)` as 64 hex characters. Any other key is a `400`; a miss is a `404`.

### Registering a resource

`POST /discovery/register`:

```json
{
  "url": "https://api.example.com/premium-data",
  "type": "http",
  "description": "Premium market data API",
  "accepts": [{
    "scheme": "exact", "network": "eip155:8453",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "amount": "10000",
    "payTo": "0x0000000000000000000000000000000000000002",
    "maxTimeoutSeconds": 60
  }],
  "metadata": { "category": "finance", "provider": "Example Corp", "tags": ["market-data"] }
}
```

`amount` is a string in atomic units (`maxAmountRequired` is accepted as its v1
spelling); a non-integer amount is a `400`, never a zero. Success is `201` with
`{"success": true, "url": ...}`. These refusals carry `error` but no `code`, so
branch on the status and on `error`:

| You see | It means |
|---|---|
| `400` `Invalid URL` | not http(s), credentials in the URL, or a private, loopback or link-local IP literal |
| `400` `Invalid resource type` | `type` must be `http`, `mcp` or `a2a` (`facilitator` also exists, and is the only type allowed an empty `accepts`) |
| `400` `No payment methods specified` | `accepts` is empty |
| `409` `Resource already registered` | that URL is already listed |
| `429` | registration has its own, much slower bucket (see `/auth.md`) |

A registered URL is health-probed like any other entry. `first_party` and `vip` are
curated by hand, `verified` answered a payment challenge correctly, and `listed` is
everything else.

---

Source: https://github.com/UltravioletaDAO/x402-rs
Operator: Ultravioleta DAO — https://ultravioletadao.xyz

## Arc EURC amounts

EURC mainnet is `0xbEf5f6d51CB62b58e6A8f77868681825C6fe21c1`; testnet is
`0x89B50855Aa3bE2F677cD6303Cec089B5F319D72a`. Both use six decimals and
EIP-712 `EURC` / `2`. One EURC is one euro, not one dollar; no automatic FX
conversion is performed. Gas stays USDC. A funded 0.01 EURC payment (10000
atomic units) was verified and settled on Arc mainnet on 2026-09-22, tx
0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128; funded testnet payments remain pending.

## Portable facilitator receipts (Arc, Base and Hedera)

Arc and Base exact USDC/EURC and native Hedera USDC return `receipt` alongside
verify/settle.
Discover `/supported.facilitatorReceipts`, `/receipts`,
`/schemas/facilitator-receipt-v1.json` and `/.well-known/receipt-keys.json`.
For private lookup, persist a purchase context and send `X-UVD-Purchase` (base64
JSON: purchaseId, secret accessToken, method, url, bodySha256). The merchant must
validate it against the actual request. Query `/receipts/{receiptId}` with
`Authorization: Bearer <accessToken>`. Never log the context or create a fresh
signature after uncertainty. An unknown receipt means poll/replay the original
authorization, not a new payment. Receipt confirmation does not prove delivery.
The original answer is replayed only with the `X-UVD-Purchase` or
`Idempotency-Key` that admitted the payment; a resend without it answers
`409 authorization_already_settled` or `409 authorization_in_flight` (verify:
`isValid: false` with the same reason), never a repeated success.
A `503` with `safeToRetry: true` and `Retry-After` sent nothing, and a receipt
`rejected` with `refusalReason: reservation_abandoned` says the same: resend the
same request after the delay; it is admitted again under the same receipt.
A failure after the send latched or its bytes were prepared is the opposite: it
carries `retryable: false`, no `Retry-After`, the prepared `transaction` and its
`paymentId`; poll the receipt, never sign a replacement.
Python `fetch_with_receipt` and TypeScript `fetchWithReceipt` return the original
HTTP response plus receipt/payment state. Supply trusted issuer keys for offline
signature verification. Live EURC acceptance was proven on Arc mainnet on
2026-09-22 (x402 v2, receipt `confirmed`, tx 0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128);
Arc testnet is still pending. Other networks
retain their existing responses. Full contract:
https://github.com/UltravioletaDAO/x402-rs/blob/main/docs/facilitator-receipts.md

Receipts cover plain `exact` payments only. On Base, `upto`, `escrow`/`commerce`
and the x402r `refund` extension keep their own settlement paths and carry no
`receipt`.
