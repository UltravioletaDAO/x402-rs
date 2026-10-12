# Portable facilitator receipts v1

Exact payments on every EVM network where production serves `exact`, in each of
its ERC-3009 tokens (x402 v1/v2; Base since 2.40.0, every such network since
2.51.0), and native Hedera USDC (mainnet and testnet, v2) return an additive
`receipt` beside `/verify` and `/settle` results. Other networks keep their
existing behavior. `GET /receipts` and `/supported.facilitatorReceipts` list the
networks; the tokens are the ones `/supported` lists for each network's `exact`
kind. See "EVM coverage" below.

The receipt contains `network` (CAIP-2), `asset`, atomic-string `amount`,
`decimals`, `payTo`, `payer`, `requestHash`, `authorizationId`,
`paymentRequestHash`, `settlement`, `refusalReason`, `status`, and retry guidance.
The public schema is `/schemas/facilitator-receipt-v1.json`. It is a proprietary
facilitator attestation, separate from the x402 merchant `offer-receipt`
extension. A confirmed payment never proves delivery of the purchased resource.

## States and transport

| State | Meaning | Buyer action |
| --- | --- | --- |
| verified | Read-only authorization check passed | Settle that authorization |
| pending | Transaction identified/prepared, outcome not final | Poll, or resend the same request with its context or Idempotency-Key |
| confirmed | Provider returned successful chain settlement | Retrieve the merchant response; do not pay again |
| rejected | Explicit validation rejection or failed chain settlement | Inspect refusalReason; do not silently create another purchase |
| rejected, `refusalReason: reservation_abandoned` | The admission was released before any transaction existed; nothing was sent | Resend the same request after `retry.afterSeconds`; it is admitted again under the same receipt |
| unknown | Missing outcome, including HTTP/RPC/storage uncertainty | Retain context and authorization; poll or replay exactly |

`settlement.id` is an EVM transaction hash or native Hedera transaction ID.
It may exist while pending/unknown; its presence alone is not confirmation.
`refusalReason` is null for uncertainty. Diagnostic errors live in
`diagnosticCode`. Revisions are monotonic for an admitted settlement; verify
receipts are ephemeral and are not private lookup records.

Merchant integrations propagate the facilitator result in base64 JSON through
`PAYMENT-RESPONSE` and `X-PAYMENT-RESPONSE`, expose both for CORS, and disable
caching. Preserve the original HTTP response and body, including HTTP 500 after
payment. Python FastAPI and TypeScript Express/Hono provide this propagation;
other integrations can use `payment_response_headers` / `paymentResponseHeaders`.
Configure proxies to forward those headers and `X-UVD-Purchase`. The SDK limits
response headers to 32 KiB; an intermediary can impose a smaller limit.

## Restart-safe purchase context

The buyer creates a random `purchaseId` and a 256-bit `accessToken`, then binds
method, full URL, and SHA-256 of the exact HTTP request body. It sends base64 JSON
in `X-UVD-Purchase`:

```json
{"purchaseId":"opaque-order-id","accessToken":"<64 lowercase hex characters>","method":"GET","url":"https://merchant.example/data","bodySha256":"<SHA-256 hex of empty bytes>"}
```

The merchant validates the descriptor against the actual request, including
query and body bytes, and forwards it unchanged to verify/settle. URL must match
the payment resource. Express requires `rawBody` for non-GET/HEAD requests.
Behind a proxy, configure the merchant's public URL correctly.

Persist the context **before** sending the signed payment request. It contains
a reusable payment authorization and a private receipt lookup capability:
keep it in private durable application storage, not logs, analytics, a public
report, or a shared cache. Resume the same context after a restart or lost
response. Neither SDK creates a new signature for a resumed purchase.

The capability scopes purchase uniqueness. A human order label alone does not.
A new capability represents a new purchase, even if its label matches. Keep one
context per order in merchant/buyer storage; this API cannot deduplicate two
independently created purchases. Merchant fulfillment requires its own durable
order handling; the facilitator prevents duplicate admission of payment.

**For how long.** The facilitator keeps a purchase's binding and its receipt
until the rows expire: 90 days after the authorization's `validBefore` (see
"Expiry"). Once DynamoDB deletes them, usually within days of expiry, a resume
or a lookup finds no receipt; the authorization itself is long dead and is
refused by verification, and a new signature for the same capability or
Idempotency-Key is a new payment. Until the deletion the rows still answer as
before. Finish or abandon a purchase within that window, and keep fulfillment
records of your own beyond it.

`GET /receipts/{receiptId}` with `Authorization: Bearer <accessToken>` returns the
latest receipt and performs read-only chain reconciliation. Missing or incorrect
capabilities get 404. Receipt IDs and tokens never appear in a public listing.
An authorized lookup returns HTTP 200 even when the payment is unknown or its
original POST returned an error. Inspect the signed `status`; HTTP 200 here
means the receipt was retrieved, not that the payment succeeded.
Without context, receipts still protect authorization replay, but private lookup
is unavailable: resend the original settlement request to learn its outcome (see
"Replays of an admitted authorization").

## Hashing and proof

The request descriptor contains exactly `purchaseId`, `method`, `url`,
`bodySha256`, `network`, `scheme`, `asset`, `amount`, and `payTo`. Missing context
fields are null; URL defaults to the payment resource. EVM addresses are lowercase;
Hedera IDs retain native spelling. Hash input is UTF-8:

`uvd-x402-request-v1` + newline + canonical JSON descriptor.

Canonical JSON sorts ASCII object keys, uses compact JSON strings without ASCII
escaping, preserves array order, and accepts only null, booleans, strings, and
integers from zero through the JavaScript safe-integer maximum. Floats and
non-ASCII keys are rejected. This is a constrained canonical profile, not a claim
to implement arbitrary JCS. A request the rail admits whose terms cannot be
written this way (free-form `extra` or `outputSchema` may carry a float) gets no
receipt and runs as before, unless its authorization was already admitted under
another request: then `/settle` answers `409 receipt_request_conflict` and
`/verify` `isValid: false` with that admission's reason, and nothing is sent or
simulated (since 2.51.0). `paymentRequestHash` uses domain
`uvd-x402-payment-v1` and the complete normalized internal verify request.
`authorizationId` binds network, asset, payer and EIP-3009 nonce, or Hedera network
and native transaction ID. It does not expose reusable signed bytes.

`proof.type = jws` signs the canonical receipt with `proof: null`, using a
dedicated Ed25519 service key. JWS protected fields: `alg: EdDSA`,
`typ: uvd-facilitator-receipt+jws`, and `kid = SHA-256(public-key bytes)`.
Fetch trusted public keys from the configured issuer's
`/.well-known/receipt-keys.json`, never a URL selected by a receipt. Both SDKs
verify the issuer, full signed payload, request hash and Ed25519 signature.
Supply trusted keys to get `proofVerified` / `proof_verified = true`; parsing an
unsigned or unverified receipt alone does not provide offline verification.

The dedicated secret `facilitator-receipt-signing-key` injects
`RECEIPT_SIGNING_KEY`. Its private value never enters Terraform state or a funded
wallet. Archive old trusted public keys before rotation; the live JWK endpoint
currently advertises the current key only. Self-hosted deployments without a
receipt key explicitly advertise HTTPS provenance and return `proof: null`.

Before the first production deployment, provision the secret **and** apply the
matching execution-role `secrets-access` policy with authorized operator
credentials. The GitHub deploy identity explicitly cannot call `iam:PutRolePolicy`.
A successful Terraform plan does not prove permission to apply an IAM change.
The receipt rollout initially hit this restriction after ECS had already been
updated; adding the single planned secret grant and rerunning only the failed
deploy job recovered it. Keep the GitHub identity's restriction in place.
For future secret additions, confirm that the execution role can read every
newly referenced secret before starting the image deployment.
Run `python scripts/check_receipt_deploy_permissions.py` with operator
credentials before publishing. This read-only check simulates the execution
role's access to the receipt key and the task role's access to receipt storage;
it does not read private key material. Review the full Terraform plan too,
because the helper does not cover unrelated infrastructure changes.

## Durable admission and recovery

Receipt rows and aliases use a new `receipt:*` namespace in the existing
idempotency table. A DynamoDB transaction conditionally reserves the record,
authorization, purchase capability and optional Idempotency-Key together (see
"Idempotency-Key scope").
Caller-supplied Idempotency-Key values beginning with `receipt:` are rejected
across all networks, preventing legacy cache writes from replacing receipt rows.
Signature verification happens before admission. Revision updates use CAS.

### Replays of an admitted authorization

The original answer goes back only to the binding that admitted the payment:
the same `X-UVD-Purchase` capability, or the same `Idempotency-Key`. That
request gets today's replay (the original status and body with
`Idempotent-Replayed: true`, or `202 settlement_in_progress` while the payment
is in flight), which is how a lost response is recovered. The Idempotency-Key
must be the same value on `/verify` and `/settle`; a key that differs per
operation binds only the call that admitted the payment. Possession of the
signed payment alone is not a purchase binding:

| Resend | `/settle` | `/verify` |
| --- | --- | --- |
| Same capability or same Idempotency-Key (until the rows expire, 90 days after `validBefore`) | Original answer replayed | Stored verdict and receipt |
| No binding, payment `confirmed` | `409 authorization_already_settled` with the receipt | `isValid: false`, `invalidReason: authorization_already_settled` |
| No binding, payment `pending` or `unknown` | `409 authorization_in_flight` with the receipt; a resend without the binding learns the final outcome by resending later | `isValid: false`, `invalidReason: authorization_in_flight` |
| No binding, payment `rejected` | Original rejection replayed | Stored rejection |
| Admission released, `reservation_abandoned` (any Idempotency-Key or none) | Admitted again under the same receipt; see "Admissions that sent nothing" | Verified as before, by simulation |
| Another capability, or none for a payment made with one | `409 receipt_request_conflict`, no receipt | `isValid: false` with the reasons above, no receipt; a rejected payment is verified as before |

These 409s never carry `success: true`, a top-level `transaction` or
`Idempotent-Replayed`; the receipt inside proves the payment to whoever holds
the payment itself. A receipt made under a purchase context is never returned
without that context. `/verify` answers from the stored receipt and does not
simulate the consumed authorization again. If receipt storage fails, while the
authorization or the binding is being read, both answer
`503 receipt_store_unavailable` with `Retry-After`, never a verdict (`/verify`
since 2.51.0; before, it fell back to simulation when the authorization could
not be read). Recovery by reconciliation and rebroadcast of saved bytes is
unchanged for every resend.

**Idempotency-Key scope (since 2.51.0).** A key binds the request that admitted
a payment (the replays above) and refuses any other: a new signature from the
same payer under a key it already used is `409 receipt_request_conflict` and
nothing is sent, on any network, token or `payTo` of the rail, until the rows
expire; this refusal compares the key trimmed, the replay binding compares it
as sent. A payment that settles in the answer to a request carrying a key it
was admitted with (the first one, or a resend with that key) is also filed in
the 24-hour Idempotency-Key cache that `/settle` keeps for every network, as
that path files what it settles, unless the key already has a live row there,
which is never replaced. That cache is read first, so for that day any other
body under the key, from any payer and on any network, is
`409 idempotency_key_conflict`, the same payer included. Filing is best effort,
and a payment confirmed by a lookup or by a resend without its key is not
filed. Use a fresh key per purchase.
Requests that `/settle` routes to their own settlement paths never enter
admission, even when their inner requirements say `exact`: the `upto`,
`escrow`/`commerce` and `fhe-transfer` schemes and the x402r `refund` extension.
**Expiry (since 2.51.0).** Every receipt row, the record and each of its
aliases, carries the table's TTL attribute `expires_at`: 90 days after the
later of the authorization's `validBefore` and the write. The rows therefore
always outlive the authorization they protect: until `validBefore` they are
what stops it being admitted twice, and after it the authorization can no
longer move funds (an EIP-3009 transfer past `validBefore` reverts). The 90
days that follow keep the purchase binding a client resumes or retries with
(`X-UVD-Purchase`, `Idempotency-Key`) and the private lookup, the retention of
the transaction index and of DX402 evidence; a test keeps it at 30 days or
more. Native Hedera keeps no `validBefore` here (its transactions are valid for
180 s at most) and expires 90 days after the write. The record row is rewritten
at every revision, so it never expires before its aliases; an expired alias
whose record DynamoDB already swept reads as no admission. Rows written before
2.51.0 carry no `expires_at` and stay until removed by hand. An admission from
before 2.51.0 that is revised afterwards (a resend that reconciles it, the
operator command) gets an expiry on its record only: 90 days later its old
aliases point at nothing, and a `/settle` or `/verify` that reaches them
answers `503 receipt_store_unavailable`, failing closed, until they are removed
by hand. An expired alias that DynamoDB has not swept yet does not block a new
admission: the reservation overwrites it. Existing legacy
cache hits are replayed before checking a consumed nonce, preserving their
original success/conflict semantics. They are not fabricated into portable
receipts, and their previous retention policy remains.

EVM networks save the exact signed transaction and hash before
broadcast. A POST retry can rebroadcast only those bytes under the writer lease,
while the authorization is valid; it never signs a replacement or changes the
nonce. An admission prepares one transaction: if the node refuses it on nonce
grounds, no second transaction is signed for that admission, the receipt stays
`unknown`, and later retries reconcile or rebroadcast the saved one. Reconciliation
refuses an EVM admission (`rejected`, `settlement_failed_on_chain`) only once its
authorization can no longer move the payer's funds: the saved transaction
reverted, and at the latest block the node serves (not behind the reverted
one), stamped at or past `validBefore`, the token reports the authorization
unused (`authorizationState`, read at that block). A revert alone is no
refusal: another transaction may have executed the same authorization first,
and a mined transaction publishes the signed authorization, which can be
executed while blocks are stamped before `validBefore`. Until then, or while
the token cannot be read, the receipt stays as it was. If the token reports the
authorization used (executed by another transaction, which pays the same
`payTo`, or cancelled by the payer), the receipt stays `unknown` with
`diagnosticCode: authorization_used_elsewhere` and `retry.action: none`, and its
saved bytes are dropped; never sign a replacement (all since 2.51.0). Records
written before 2.51.0 do not keep the authorization's nonce and are judged by
neither. Hedera links the
native ID before its existing store can persist co-signed bytes and recover them.
Terminal receipt updates remove private EVM signed bytes. Native byte retention
continues under the existing Hedera store policy.

### Admissions that sent nothing

A settlement can end after its admission but before anything leaves the
facilitator: the EVM writer lease moved between routing and signing, a read or
gas estimate failed, the transaction could not be filled or signed, or its bytes
could not be stored. That is not uncertainty, and it is no longer left
`unknown`. The admission is released in place only when all of these hold:

- the provider ended its settlement before its send latch (EVM: every failure of
  the exact settle before `send_transaction_from` is about to broadcast; Hedera:
  the save of the native transaction ID failed, before its own store or the
  network). No failure after the latch can release anything;
- no transaction bytes or ID were prepared, and the answer names no transaction;
- the durable record is still at the revision this settlement admitted
  (compare-and-set). Bytes stored by a write that looked failed, or any other
  writer, make the close fail and the receipt stays as it was.

The caller gets `503` with `Retry-After`, the provider's `error`, and
`retryable: true`, `safeToRetry: true`, `success: false`. The receipt is
`rejected` with `refusalReason: reservation_abandoned`, the provider's code in
`diagnosticCode`, no `settlement`, and `retry: {"action": "resend",
"afterSeconds": N}`. Resend the same request: it is verified again and admitted
again under the SAME receipt at its next revision, so a client that stored the
first receipt sees the same `receiptId` with a higher revision. Only the request
that was admitted takes it back: the same purchase capability, or none if it
had none; any Idempotency-Key it brings is bound as well, and the key that
admitted it first keeps its binding. Another capability is still
`409 receipt_request_conflict`. Concurrent resends race on the revision and
exactly one is admitted; the rest are answered as resends of an admission in
flight. Never sign a replacement for a released admission.

Failures before admission say the same: `receipt_store_unavailable`,
`receipt_signing_unavailable` and `receipt_reservation_uncertain` are `503` with
`Retry-After`, `retryable: true` and `safeToRetry: true`. A reservation or
re-admission whose store write could not be confirmed is never run, and it is
closed as above whether it landed or not: the close writes the record if there
is none yet (since 2.51.0), so a write that lands after it finds the record
taken and writes nothing. Admission and re-admission writes carry a fresh
idempotency token each, so a write the store client resends after losing its
answer gets its original success back. When the store that refused the bytes
also refuses the release, nothing was sent and the answer is the same `503
receipt_store_unavailable` with `safeToRetry: true` (since 2.51.0; before, the
provider's own `400`), with `receiptWarning: receipt_persistence_unavailable`
and the receipt as stored: `unknown`, and resends of it are answered as an
admission in flight until the operator command below releases it (after a
store outage, run its read-only pass); or `pending` with the transaction, if a
write of the bytes that looked failed did land, and then a resend broadcasts
those same bytes.

### Failures after the send

The opposite case. Once the provider latched its send, prepared bytes or named a
transaction, a failure that reaches `finish` with no chain verdict is answered,
and stored for a bound resend, with `retryable: false` and without
`Retry-After`, whatever the provider's own body said; when the answer names no
transaction, the admission's prepared one is added as `transaction` with its
`paymentId`. The receipt stays `unknown` with `retry.action: poll`, as before.
A settlement whose answer cannot be read at all (`502
receipt_response_unreadable`) is answered the same way, with the receipt. Until
2.39.6 both could carry `retryable: true`, and a `502` from the provider kept
its `Retry-After: 30`, which clients read as "resend": after a payment that did
mine, that resend is what ends with the buyer signing a second one.

Poll the receipt, or resend the same request with the binding that admitted it;
never sign a replacement. Failures before the send keep their answers.

### Operator: admissions stranded by earlier releases

Earlier releases left such admissions `unknown` with nothing prepared, and a
process that dies between admission and send can still leave one. The
facilitator binary closes them exactly as the settle path does, signed with the
service key and with the same revision compare-and-set, so a settlement still
holding the admission can no longer store bytes and never sends:

```bash
x402-rs receipts release-abandoned                 # read-only (also --dry-run)
x402-rs receipts release-abandoned --write --receipt-id <uuid> [--receipt-id <uuid>]...
```

It selects settle records that are `unknown`, have no prepared bytes and no
settlement, and are older than `--min-age-secs` (default 900) or whose
authorization has expired. It prints one JSON line per record (`would_release`,
`released`, or a `skip_*` reason) and a summary; nothing private. Without
`--receipt-id` it scans the table (`dynamodb:Scan`, which the service role does
not have): run the read-only pass with operator credentials, for example
`IDEMPOTENCY_TABLE_NAME=idempotency_records AWS_REGION=us-east-2 cargo run --locked -- receipts release-abandoned`.
Writing needs `RECEIPT_SIGNING_KEY` and refuses a record signed by another key,
so run `--write` with the listed ids as a one-off task of the service's own task
definition (container `facilitator`, `command` override
`["receipts","release-abandoned","--write","--receipt-id","<uuid>"]`), where the
key never leaves the task; its output goes to the service's log group. A
released authorization that has expired simply fails verification when resent.

Known limits: a process that dies between admission and send leaves its
admission `unknown` until the operator command closes it. Confirmed receipts
reflect the provider's confirmation policy and are not a continuous reorg
monitor. Receipt rows expire by TTL (see "Expiry" above); rows written before
2.51.0 have no expiry and are not archived. Key rotation
requires retaining the previous public keys externally. An exact request
conflicting with an existing purchase is refused with 409, without exposing
that purchase's private receipt.

## EVM coverage

Since 2.51.0 the EVM side of the rail is one matrix, read in one place
(`receipts::evm_exact_tokens`) for what `GET /receipts` announces, what is
admitted and how a receipt is normalized:

- **Networks**: every EVM network for which production publishes an `exact`
  entry in `/supported`, as `config/supported_tokens.json` declares it
  (`exactServed`). Ethereum Sepolia has a USDC deployment but no RPC in
  production, so no `exact` entry, and stays out; so do the EVM networks the
  facilitator does not serve at all. Read the live list with
  `curl -s https://facilitator.ultravioletadao.xyz/receipts | jq .networks`.
- **Tokens**: the ones `/supported` lists for that network's `exact` kind
  (ERC-3009 deployments only). BSC's USDC has no ERC-3009, so on BSC only AUSD
  is admitted.
- **Admission**: a plain `exact` request whose payment payload and terms name
  the same network, one of the matrix's, in one of that network's tokens.
  Anything else runs as before, without a receipt; carrying `X-UVD-Purchase`, a
  `/settle` of it answers `400 receipt_request_not_supported`. The
  requests that belong to other settlement paths stay out on every network.
- **Normalization**: `network` in CAIP-2, `asset` and `payTo` lowercase,
  `decimals` from the token (every EVM receipt carries them).

Every network that joined in 2.51.0 changes for its callers exactly as Base did
in 2.40.0: the table in "Base" below reads the same with the release before
2.51.0 in place of 2.39 and 2.51.0 in place of 2.40.0.
On Arc and Base nothing changes for a payment the facilitator can settle. A
request in a token that is not one of the network's `exact` tokens, or whose
payload and terms name different networks, was refused by verification before
and still is, now without a `rejected` receipt. `networks` lists every entry it
listed before; their order follows the network list instead of the release
that added them.

What the rail costs on each network it covers, as on Arc and Base: a `/settle`
verifies once before admission and the provider verifies again, and admission
and each revision are writes to the receipt store; a `/verify` reads the
authorization's row. Receipt rows expire 90 days after the authorization's
`validBefore` (see "Expiry"). With the store unreachable an `exact` settle on
any of these networks answers `503 receipt_store_unavailable` and sends nothing,
where before 2.51.0 only Arc, Base and Hedera did. Two alarms on the
facilitator's SNS topic watch the store (`alerts-receipts-store.tf`), both on
DynamoDB's own metrics: `SystemErrors` and `ThrottledRequests` on the table
in two five-minute periods running, and a p99 latency above one second for
fifteen minutes. The 503s themselves reach the load balancer's 5xx alarm,
already on the same topic, and it is the only one that sees a request refused
for access (DynamoDB counts those only in its account-wide `UserErrors`, not per
table) or one that never reached DynamoDB. The facilitator's own count of store
requests that
failed (log line `receipt_store_failure`, with `op` and `kind`) is a metric to
read during an incident, not a page: it is counted from log lines, and the
facilitator logs text taken from requests, so one request could write the
token.

Joining the matrix takes no receipts code: an EVM network with an ERC-3009
token and `exactServed` in the catalog is announced and admitted. **Leaving it
is not a configuration change.** A resend of an authorization whose network or
token is no longer admitted skips its stored receipt and runs as an ordinary
settle; if the first one mined, that answer names no receipt and a buyer can
end up signing again. So the pairs the rail admits are written down in a test
(`every_network_and_token_on_the_rail_stays_on_it`) that must equal the matrix
both ways: a pair that joins is added there, and removing one means editing that
test on purpose and deciding what happens to the receipts already stored.

The same holds for what the test cannot see:

- **Rolling back to a release before 2.51.0** takes 23 networks off the rail at
  once, and that release reads none of their receipts. Their payments are then
  resent down the ordinary path for as long as a client resumes them, not only
  until each authorization's `validBefore`. One settled under an
  Idempotency-Key in the previous 24 hours is answered from the Idempotency-Key
  cache (2.51.0 files it there), as is another body under that key. Any other
  resend of a settled payment fails on its consumed nonce, an answer that
  invites a new signature, and an admission still in flight can have its
  authorization sent a second time (the token executes it once). That release
  also rewrites the receipt records it revises without `expires_at`. Treat a
  rollback below 2.51.0 as a change that moves money, not a routine
  operation; prefer a forward fix.
- Changing a token's address or a network's chain id in `src/network.rs`
  changes what the rail admits and the `authorizationId` it looks receipts up
  by.
- Without `IDEMPOTENCY_TABLE_NAME` there is no receipt service at all, on every
  network.

## Base

Since 2.40.0 plain `exact` payments on Base (`base` / `eip155:8453`, USDC and
EURC, x402 v1 and v2) are admitted through receipts. Base Sepolia and the other
EVM networks stayed unchanged until 2.51.0 (see "EVM coverage"); the Base
requests that belong to other settlement paths (see above) carry no `receipt`.

What a Base `exact` caller observes compared with 2.39:

| Situation | 2.39 | 2.40.0 |
| --- | --- | --- |
| Any verify/settle response | No `receipt` | Additive `receipt`, `Cache-Control: no-store`; all previous fields unchanged |
| Request carrying `X-UVD-Purchase` | `/settle` answers `400 receipt_request_not_supported`; `/verify` ignores the header | Admitted; its receipt is private to that capability (`GET /receipts/{receiptId}`) |
| Same authorization after it settled, with the Idempotency-Key or `X-UVD-Purchase` that admitted it | With the key, the cached 200 with `Idempotent-Replayed: true` for 24 hours; after that it runs again and fails on the consumed nonce | Original 200 replayed with `Idempotent-Replayed: true`; receipt rows did not expire until 2.51.0, and since then expire 90 days after the authorization's `validBefore` |
| Same authorization after it settled, without that binding | Runs again; fails on the consumed nonce | `409 authorization_already_settled`, with the receipt when the payment carried no `X-UVD-Purchase`; a payment admitted with `X-UVD-Purchase` answers `409 receipt_request_conflict` without it. Verify `isValid: false`, `authorization_already_settled` |
| Same authorization while its settlement runs | Second attempt runs | `202 settlement_in_progress` to the admitting binding. Without it, `409 authorization_in_flight`, with the receipt when the payment carried no `X-UVD-Purchase`; a payment admitted with `X-UVD-Purchase` answers `409 receipt_request_conflict` without it |
| New signature, same `Idempotency-Key` | Runs, unless an earlier attempt succeeded in the last 24 hours (then `409 idempotency_key_conflict`) | `409 receipt_request_conflict` once the first one was admitted, until its rows expire; since 2.51.0 on any network, token or `payTo` for the same payer, and for 24 hours after the first one settled under that key `409 idempotency_key_conflict` from any payer, the same payer included (see "Idempotency-Key scope") |
| New signature, same `X-UVD-Purchase` | `400 receipt_request_not_supported` | `409 receipt_request_conflict` once the first one was admitted, until its rows expire (since 2.51.0) |
| New signature without key or purchase context | New payment | New payment (unchanged) |
| Settlement ends before anything is sent (writer lease moved, a read or gas estimate failed, signing or storing the bytes failed) | That failure's own answer | `503` with `Retry-After` and `safeToRetry: true`; receipt `rejected` with `refusalReason: reservation_abandoned`; the same request is admitted again under the same receipt |
| The node refuses the transaction on nonce grounds | Up to two more attempts, each with a new nonce and a new signed transaction | No second transaction. A failure after the send: `502 upstream_nonce_or_mempool` with `retryable: false`, the stored `transaction` and its `paymentId`, no `Retry-After`, and the receipt `unknown`; the purchase's resends get the same answer and reconcile or rebroadcast it (see below) |
| Receipt storage unreachable | Settles without a key; `503 idempotency_store_unavailable` with one | `503 receipt_store_unavailable` with `Retry-After` and `safeToRetry: true`, with or without a key; nothing is sent |

Idempotency-Key responses cached before the upgrade are still replayed for their
remaining lifetime.

**A nonce refusal under an admission.** The node refused the stored bytes: this
send did not queue them, and nothing else is signed or sent for the admission. A
node that says it already holds them is answered `502 settlement_unconfirmed`
with the hash instead. The rail cannot tell whether the same bytes are held
elsewhere, so the refusal is answered as a failure after the send (see "Failures
after the send"): the receipt, not the status code, decides. Resend the same
request with the binding that admitted it, or poll the receipt; never sign a
replacement. The signer is left with no nonce gap: its next settlement takes the
nonce the node reports, at once.

Measured with the published SDKs against local stand-ins (no network): Python
0.90.1 (`FastAPIX402` error mapping, `fetch_with_receipt`) and TypeScript 2.98.0
(`createPaymentMiddleware` on Express, `fetchWithReceipt`). With this answer both
merchants answer `500` with the receipt in `PAYMENT-RESPONSE`, never `402`. The
buyer ends in `unknown` with that receipt, and two resumes of the same purchase
resend the same authorization: the facilitator saw one. The same failure without
`retryable: false` and with `Retry-After: 30`, as it read before 2.39.6, made both
merchants answer `503` instead, with the same outcome for the buyer.

## Validation scope

Shared offline signed vectors cover Arc USDC/EURC and Hedera USDC on both ledgers,
and Base USDC/EURC; a Rust test rebuilds every vector from its synthetic inputs.
`GET /receipts` remains the list of admitted networks.
Rust tests cover concurrent admission, restart, invalid signatures, conflicts,
private lookup, storage outages and lost responses; replays with and without the
admitting binding (settled, in flight, uncertain, concurrent) run on every
admitted network, and so do released admissions: the same request taken back
under the same receipt, only by its own binding, and one winner among
concurrent resends, which a store double holds at one revision so that only
the compare-and-set can pick the winner. A test for each guard fails when a release follows prepared
bytes, the send latch or a named transaction; an EVM test drives the real send
path under an admission (lease lost: nothing sent or latched; sent: bytes stored
first, then latched). Explicit local DynamoDB tests exercise transaction/CAS
behavior, including readmission and a write resent with its token, using two
store clients, and that every row they write carries `expires_at`. Offline
tests check that every row of a reservation, a readmission and a revision
carries it, 90 days past the later of `validBefore` and the write and never
under 30, that the Terraform table expires on that attribute and the store's
metric counts the line the store writes, that every failed request writes that
line, and how an alias whose record is gone reads; `terraform test` pins the TTL
and the alarms, which read DynamoDB's metrics only. Every network and token of the EVM matrix runs through two
replicas racing one purchase over a shared store (every copy reaches the
reservation, so only its atomicity picks the one that sends), a lost response,
a restart with an unknown outcome and the storage outages: one send, the same
receipt on every resend, nothing broadcast without stored bytes. The replays run on every
network of it, and so does the exclusion of the requests that belong to other
settlement paths. A test checks the matrix against the tokens
`config/supported_tokens.json` presents and admits every one of them in v1 and
v2 with its decimals; another fails if anything outside it (Ethereum Sepolia,
Sei, XDC, BSC's USDC, a body that names two networks, Solana) is announced or
admitted. A test still fails if Base leaves `GET /receipts` or admission. EVM
tests drive a nonce refusal under an admission: one broadcast, no second nonce,
and the signer's next settlement takes the node's nonce for a taken slot and for
a node behind the signer; a node that already holds the bytes comes out
`settlement_unconfirmed` first; a receipt test pins the answer and its resends. SDK tests cover
signature tampering, request binding, restart without a new signature, and a
confirmed receipt alongside a merchant HTTP 500. These fixtures are not payments.
Live EURC acceptance was proven on Arc mainnet on 2026-09-22 with a `confirmed` x402 v2
receipt (tx `0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128`); Arc testnet is still pending. Publication
and live USDC acceptance are recorded separately in the release evidence.

Protocol reference: x402 v2 and offer-and-receipt at upstream commit
`c8c71f244c0d45a6a4fd990a96c69aa34781cd05`.


Published and chain-verified: facilitator 2.36.1, Python 0.88.0 and TypeScript
2.96.0. The [release evidence](reports/2026-09-17-facilitator-receipts-release.md)
contains eight USDC payments (both SDKs, both Arc and Hedera networks), signed
receipts, independent chain checks and same-authorization recovery after a
merchant HTTP 500. A ninth attempt was quota-blocked; its unchanged `unknown`
receipt remains recoverable with both SDKs. EURC live settlement is still deferred.

> **Update 2026-09-22:** a live EURC payment settled on Arc mainnet (tx
> `0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128`, block 22114558). The paragraph above is the
> 2026-09-17 campaign record.
