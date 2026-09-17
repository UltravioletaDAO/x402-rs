# Portable facilitator receipts v1

Arc exact USDC/EURC (mainnet and testnet, x402 v1/v2) and native Hedera USDC
(mainnet and testnet, v2) return an additive `receipt` beside `/verify` and
`/settle` results. Other networks keep their existing behavior. Discover runtime
availability in `/supported.facilitatorReceipts` or `GET /receipts`.

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
| pending | Transaction identified/prepared, outcome not final | Poll or resend the same authorization |
| confirmed | Provider returned successful chain settlement | Retrieve the merchant response; do not pay again |
| rejected | Explicit validation rejection or failed chain settlement | Inspect refusalReason; do not silently create another purchase |
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
is unavailable: resend the original settlement request to retrieve its receipt.

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
There is **no TTL on these rows**: legacy cache expiry cannot admit another
payment. Existing legacy cache hits are replayed before checking a consumed
nonce, preserving their original success/conflict semantics. They are not
fabricated into portable receipts, and their previous retention policy remains.

Arc saves the exact signed transaction and hash before broadcast. A POST retry
can rebroadcast only those bytes under the writer lease, while the authorization
is valid; it never signs a replacement or changes the nonce. Hedera links the
native ID before its existing store can persist co-signed bytes and recover them.
Terminal receipt updates remove private EVM signed bytes. Native byte retention
continues under the existing Hedera store policy.

Known limits: an abandoned reservation before transaction preparation stays
unknown and needs operator investigation; it does not automatically admit a
replacement. Confirmed receipts reflect the provider's confirmation policy and
are not a continuous reorg monitor. Receipt records have no automatic archival
job yet. Key rotation requires retaining the previous public keys externally.
An exact request conflicting with an existing purchase is refused with 409,
without exposing that purchase's private receipt.

## Validation scope

Shared offline signed vectors cover Arc USDC/EURC and Hedera USDC on both ledgers.
Rust tests cover concurrent admission, restart, invalid signatures, conflicts,
private lookup, storage outages and lost responses; an explicit local DynamoDB
test exercises transaction/CAS behavior using two store clients. SDK tests cover
signature tampering, request binding, restart without a new signature, and a
confirmed receipt alongside a merchant HTTP 500. These fixtures are not payments.
Live EURC payment acceptance remains deferred by user instruction. Publication
and live USDC acceptance are recorded separately in the release evidence.

Protocol reference: x402 v2 and offer-and-receipt at upstream commit
`c8c71f244c0d45a6a4fd990a96c69aa34781cd05`.


Published and chain-verified: facilitator 2.36.1, Python 0.88.0 and TypeScript
2.96.0. The [release evidence](reports/2026-09-17-facilitator-receipts-release.md)
contains eight USDC payments (both SDKs, both Arc and Hedera networks), signed
receipts, independent chain checks and same-authorization recovery after a
merchant HTTP 500. A ninth attempt was quota-blocked; its unchanged `unknown`
receipt remains recoverable with both SDKs. EURC live settlement is still deferred.
