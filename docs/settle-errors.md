# `/settle` failures, for a seller

What each failure of `POST /settle` means for the one decision a seller has to
make: may the buyer be asked to sign a new authorization, or not.

The interactive reference is `/docs` (`POST /settle`); the agent-facing version
is section 9 of `/skill.md`. This page is the one that groups every answer by
that decision.

## The rule

A failure produced after the transaction may have left the facilitator says so
in its body: `"retryable": false`, no `Retry-After` header, and `transaction`
with its `paymentId` whenever the facilitator knows them.

- **`retryable: false`, or a `transaction` in a failure: do not ask for a new
  signature.** The payment may be mined. Look the transaction up (on the
  receipt rail, poll the receipt) and deliver if it settled. When there is no
  hash, check the payer's transfer to `payTo` on chain. Only a transaction that
  failed on chain, or is not found after that chain's finality window, frees
  the buyer to sign a new one.
- **Neither: nothing was sent.** The failure was reached before the transaction
  left. Resend the same request when the answer invites it (`Retry-After`,
  `safeToRetry`), or fix the request when it is a refusal.

A new authorization for a payment that did land is a second, perfectly valid
payment: the token's own nonce check does not stop it. That is why the body, not
the status code, carries the answer: several `502`s mean "may be on chain" and
two mean "nothing was sent".

## May be on chain: never ask for a new signature

| Answer | Where it comes from |
|---|---|
| `502` `settlement_unconfirmed` + `transaction` + `paymentId` | The transaction was sent and no verdict came back: the receipt or confirmation did not arrive, the node's answer to the send was lost (a timeout, a dropped connection, a gateway error, an answer that does not parse), or the node says it already holds or already processed the transaction. Every network family, with the hash in its own chain's encoding. |
| `502` `settlement_unconfirmed` + the sweep's signature | Solana settlement account (`settleSecretKey`): the sweep of the deposit to `payTo` was sent and no verdict came back. The deposit is already recorded as used, so resending the same payload does not sweep again: look the sweep up. |
| `502` `broadcast_uncertain (ref: …)` | EVM: the send was refused on nonce grounds after the signer's transaction count moved, or the count could not be read. No hash can be attributed to this payment. |
| `502` `receipt_pending (ref: …)` | Broadcast succeeded and the receipt has not arrived, where no hash survives to be named. |
| `502` `success: false`, `error: settlement_unconfirmed` or `broadcast_uncertain` | The `upto`, `escrow` and `refund` routes, same causes, same fields. |
| `502` `success: false`, `retryable: false` | `fhe-transfer`: the FHE facilitator settles on its own side, and its answer did not come back cleanly. |
| `502` `reason: forward_unconfirmed` | A task that does not hold the EVM signer forwarded the settle to the one that does, and the answer was lost after it was sent. |
| `502` `receipt_response_unreadable`, or any failure carrying a `receipt` with `status: unknown` | The receipt rail (Arc, native Hedera): a failure after the send latched or its bytes were prepared. The body carries the prepared `transaction` and its `paymentId`. |

## Nothing was sent: the same request may be resent

| Answer | Meaning |
|---|---|
| `502` `upstream_rpc_unavailable (ref: …)` + `Retry-After: 30` | The node could not answer before anything was sent. |
| `502` `upstream_nonce_or_mempool (ref: …)` + `Retry-After: 30` | The node refused on nonce or mempool grounds; the transaction never queued. |
| `503` `upstream_rate_limited (ref: …)` + `Retry-After: 60` | The RPC provider is rate limiting the facilitator. |
| `503` `facilitator_signer_unfunded (ref: …)` + `Retry-After` ≈ 300 | The facilitator's signer cannot pay gas on this network. |
| `503` `writer_lease_unavailable` or `reason: forward_failed` + `Retry-After: 5` | This task may not sign, and the task that may was not reached. |
| `503` with `safeToRetry: true` | The receipt rail sent nothing: storage or signing failed before admission, or the admission was released before any transaction existed. |

## Refused: fix the request

| Answer | Meaning |
|---|---|
| `400` `contract_call_failed (ref: …)` | The chain executed the call and rejected it, or the failure could not be classified. |
| `200` `success: false` + `errorReason` | Settlement was attempted and refused, or the transaction was mined and failed (then `transaction` names it and nothing moved). |
| `409` | A conflicting `Idempotency-Key`, purchase or authorization; see `/docs`. |

## Until 2.39.6

- EVM `broadcast_uncertain` and `receipt_pending` answered `502` without
  `retryable: false`. A send whose answer was lost answered
  `upstream_rpc_unavailable` with `Retry-After: 30`, and a node already holding
  the transaction `upstream_nonce_or_mempool` with `Retry-After: 30`.
- NEAR, Stellar, Algorand, Sui and XRPL answered a submission whose answer was
  lost `200` with `success: false` and no transaction. Solana answered a failed
  confirmation read with `contract_call_failed`, or with a retryable `502`/`503`.
- `upto` and `refund` answered `400` with the error text; the escrow scheme
  answered without the hash.
- The Solana settlement-account sweep answered `400 contract_call_failed`,
  without `retryable` or its signature, whatever became of the sweep.
- A forwarded settle whose answer was lost answered `503` with
  `Retry-After: 5`.
- The receipt rail answered `receipt_response_unreadable` with
  `retryable: true`, and a provider failure after the send kept its
  `Retry-After`.
