# Portable facilitator receipts v1

Arc exact USDC/EURC (mainnet and testnet, x402 v1/v2), Base exact USDC/EURC
(mainnet, x402 v1/v2, since 2.40.0) and native Hedera USDC (mainnet and testnet,
v2) return an additive `receipt` beside `/verify` and `/settle` results. Other
networks keep their existing behavior. Networks are added one at a time;
discover runtime availability in `/supported.facilitatorReceipts` or
`GET /receipts`.

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
to implement arbitrary JCS. `paymentRequestHash` uses domain
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
authorization, purchase capability and optional scoped Idempotency-Key together.
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
| Same capability or same Idempotency-Key | Original answer replayed | Stored verdict and receipt |
| No binding, payment `confirmed` | `409 authorization_already_settled` with the receipt | `isValid: false`, `invalidReason: authorization_already_settled` |
| No binding, payment `pending` or `unknown` | `409 authorization_in_flight` with the receipt; a resend without the binding learns the final outcome by resending later | `isValid: false`, `invalidReason: authorization_in_flight` |
| No binding, payment `rejected` | Original rejection replayed | Stored rejection |
| Admission released, `reservation_abandoned` (any Idempotency-Key or none) | Admitted again under the same receipt; see "Admissions that sent nothing" | Verified as before, by simulation |
| Another capability, or none for a payment made with one | `409 receipt_request_conflict`, no receipt | `isValid: false` with the reasons above, no receipt; a rejected payment is verified as before |

These 409s never carry `success: true`, a top-level `transaction` or
`Idempotent-Replayed`; the receipt inside proves the payment to whoever holds
the payment itself. A receipt made under a purchase context is never returned
without that context. `/verify` answers from the stored receipt and does not
simulate the consumed authorization again. If receipt storage fails while the
binding is being resolved, both answer `503 receipt_store_unavailable`, never a
verdict. Recovery by reconciliation and rebroadcast of saved bytes is unchanged
for every resend.
Requests that `/settle` routes to their own settlement paths never enter
admission, even when their inner requirements say `exact`: the `upto`,
`escrow`/`commerce` and `fhe-transfer` schemes and the x402r `refund` extension.
There is **no TTL on these rows**: legacy cache expiry cannot admit another
payment. Existing legacy cache hits are replayed before checking a consumed
nonce, preserving their original success/conflict semantics. They are not
fabricated into portable receipts, and their previous retention policy remains.

EVM networks (Arc, Base) save the exact signed transaction and hash before
broadcast. A POST retry can rebroadcast only those bytes under the writer lease,
while the authorization is valid; it never signs a replacement or changes the
nonce. An admission prepares one transaction: if the node refuses it on nonce
grounds, no second transaction is signed for that admission, the receipt stays
`unknown`, and later retries reconcile or rebroadcast the saved one. Hedera links the
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
`Retry-After`, `retryable: true` and `safeToRetry: true`. A reservation whose
store write could not be confirmed is never run; if it landed anyway it is
released as above. Admission and re-admission writes carry a fresh idempotency
token each, so a write the store client resends after losing its answer gets
its original success back.

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
monitor. Receipt records have no automatic archival job yet. Key rotation
requires retaining the previous public keys externally. An exact request
conflicting with an existing purchase is refused with 409, without exposing
that purchase's private receipt.

## Base

Since 2.40.0 plain `exact` payments on Base (`base` / `eip155:8453`, USDC and
EURC, x402 v1 and v2) are admitted through receipts. Base Sepolia and the other
EVM networks are unchanged; so are the Base requests that belong to other
settlement paths (see above), which carry no `receipt`.

What a Base `exact` caller observes compared with 2.39:

| Situation | 2.39 | 2.40.0 |
| --- | --- | --- |
| Any verify/settle response | No `receipt` | Additive `receipt`, `Cache-Control: no-store`; all previous fields unchanged |
| Request carrying `X-UVD-Purchase` | `/settle` answers `400 receipt_request_not_supported`; `/verify` ignores the header | Admitted; its receipt is private to that capability (`GET /receipts/{receiptId}`) |
| Same authorization after it settled, with the Idempotency-Key or `X-UVD-Purchase` that admitted it | With the key, the cached 200 with `Idempotent-Replayed: true` for 24 hours; after that it runs again and fails on the consumed nonce | Original 200 replayed with `Idempotent-Replayed: true`; receipt rows do not expire |
| Same authorization after it settled, without that binding | Runs again; fails on the consumed nonce | `409 authorization_already_settled`, with the receipt when the payment carried no `X-UVD-Purchase`; a payment admitted with `X-UVD-Purchase` answers `409 receipt_request_conflict` without it. Verify `isValid: false`, `authorization_already_settled` |
| Same authorization while its settlement runs | Second attempt runs | `202 settlement_in_progress` to the admitting binding. Without it, `409 authorization_in_flight`, with the receipt when the payment carried no `X-UVD-Purchase`; a payment admitted with `X-UVD-Purchase` answers `409 receipt_request_conflict` without it |
| New signature, same `Idempotency-Key` | Runs, unless an earlier attempt succeeded in the last 24 hours (then `409 idempotency_key_conflict`) | `409 receipt_request_conflict` once the first one was admitted |
| New signature, same `X-UVD-Purchase` | `400 receipt_request_not_supported` | `409 receipt_request_conflict` once the first one was admitted |
| New signature without key or purchase context | New payment | New payment (unchanged) |
| Settlement ends before anything is sent (writer lease moved, a read or gas estimate failed, signing or storing the bytes failed) | That failure's own answer | `503` with `Retry-After` and `safeToRetry: true`; receipt `rejected` with `refusalReason: reservation_abandoned`; the same request is admitted again under the same receipt |
| The node refuses the transaction on nonce grounds | Up to two more attempts, each with a new nonce and a new signed transaction | No second transaction: `502 upstream_nonce_or_mempool` with the receipt `unknown` naming the stored one; the purchase's resends get the same status and body (without `Retry-After`) and reconcile or rebroadcast it (see below) |
| Receipt storage unreachable | Settles without a key; `503 idempotency_store_unavailable` with one | `503 receipt_store_unavailable` with `Retry-After` and `safeToRetry: true`, with or without a key; nothing is sent |

Idempotency-Key responses cached before the upgrade are still replayed for their
remaining lifetime.

**A nonce refusal under an admission.** The node refused the stored bytes: this
send did not queue them, and nothing else is signed or sent for the admission.
The rail cannot tell whether the same bytes are held elsewhere, so the answer is
read like any failure after the send: the receipt, not the status code, decides.
Resend the same request with the binding that admitted it, or poll the receipt;
never sign a replacement. The signer is left with no nonce gap: its next
settlement takes the nonce the node reports, at once.

Measured with the published SDKs against local stand-ins (no network): Python
0.90.1 (`FastAPIX402` error mapping, `fetch_with_receipt`) and TypeScript 2.98.0
(`createPaymentMiddleware` on Express, `fetchWithReceipt`). The merchant answers
`503` with `Retry-After` and the receipt in `PAYMENT-RESPONSE`, never `402`. The
buyer ends in `unknown` with that receipt, and two resumes of the same purchase
resend the same authorization: the facilitator saw one. A body that also carries
`retryable: false` and the transaction makes both merchants answer `500` instead,
with the same outcome for the buyer.

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
store clients. Base runs through the same admission as Arc (concurrency, EVM
address normalization, requests that belong to other settlement paths,
DynamoDB), and a test fails if Base leaves `GET /receipts` or admission. EVM
tests drive a nonce refusal under an admission: one broadcast, no second nonce,
and the signer's next settlement takes the node's nonce for a taken slot and for
a node behind the signer; a receipt test pins the answer and its resends. SDK tests cover
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
