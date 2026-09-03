# DX402 — `durable-evidence` Extension Specification v0.2

**Status:** Implemented and running in production (`x402-rs` ≥ 2.10.0). Describes
**only what is shipped**; anything not yet built is listed in §17 as out of
scope, not described in the future tense.
**Extension key:** `durable-evidence`
**Authors:** Ultravioleta DAO
**Supersedes:** v0.1 (2026-08-14). **Date:** 2026-09-03.

---

## 1. Motivation

x402 settles payment durably on-chain but delivers the purchased resource
exactly once, in the body of a `200 OK`, and retains nothing. Settlement is
permanent; **delivery is not**. A buyer who did not persist the response at the
moment of delivery cannot recover it, and afterwards neither party can prove
**what** was delivered — only **that** payment occurred.

`durable-evidence` closes that gap with four properties:

1. **Durable** — the delivered body survives the session.
2. **Private** — the body is encrypted; no third party, the facilitator and the
   storage backend included, can read it.
3. **Coupled** — no registration, no extra round trip. The buyer's encryption
   key is produced by the act of paying.
4. **Bidirectional** — the same envelope can be opened by the seller (and an
   auditor), so *"you sent me something else"* is checkable from both sides.

## 2. Core insight

> A payment authorization is a digital signature, and a signature yields the
> signer's **public key**, not merely their address. The resource server can
> therefore encrypt to the payer using key material the payment itself produced.

Paying *is* publishing your encryption key.

| Family | Curve | Source of the payer's public key |
|---|---|---|
| EVM | secp256k1 | ECDSA recovery over the EIP-712 / EIP-3009 signature |
| Solana / Fogo | ed25519 | The address **is** the key; mapped to X25519 for ECDH |
| NEAR | ed25519 | Account access key |
| Stellar | ed25519 | The address encodes the key |
| Algorand | ed25519 | The address is key + checksum |
| Sui | ed25519 / secp256k1 | The signature envelope carries the key |
| XRPL | secp256k1 / ed25519 | `SigningPubKey` of the signed transaction |

**Small-order ed25519 points MUST be rejected** (RFC 7748 §6.1, constant time).
`VerifyingKey::from_bytes` in common libraries accepts non-canonical and
small-order encodings; unchecked, ECDH collapses to a constant shared secret.
Conformance is tested against libsodium's 7-value blacklist, not against
vectors an implementation invented for itself.

## 3. Terminology

- **Resource Server (RS)** — the seller. Holds the plaintext, encrypts, anchors.
- **Facilitator** — verifies and settles payment; acts as **notary and index**
  for evidence. Never holds plaintext or key material in `direct` mode.
- **Evidence Store** — key- or content-addressed durable storage.
- **CEK** — Content Encryption Key: random, per-response, 256 bits.
- **Pointer** — a URI locating the ciphertext.
- **paymentId** — `keccak256(caip2Network ‖ txHash)`. A pure function of public
  data, so any party can derive it. Used as the AEAD associated data.

## 4. Declaration, and the buyer's opt-in

### 4.1 Per-route configuration

```jsonc
{
  "mode": "direct",           // "direct" is the only mode served (see §17)
  "backend": "s3",            // "s3" | "ipfs" | "arweave"
  "retention": "90d",         // "90d" | "1y" | "permanent"
  "maxBodyBytes": 33554432    // above this, skip (never fail the payment)
}
```

All fields optional. `maxBodyBytes` is a **memory** bound on the seller —
sealing holds plaintext and ciphertext at once — and an implementation that
raises it MUST bound concurrency alongside it (§7.3 `busy`).

### 4.2 The buyer chooses through `accepts`

x402 already returns an **array** of `PaymentRequirements` in the 402. The
buyer's opt-in uses nothing else: the seller lists the same resource twice,
once plain and once carrying the declaration, priced as it sees fit.

```jsonc
{
  "accepts": [
    { "scheme": "exact", "maxAmountRequired": "10000", "resource": "…", "extra": { "name": "USD Coin", "version": "2" } },
    { "scheme": "exact", "maxAmountRequired": "12000", "resource": "…",
      "extra": { "name": "USD Coin", "version": "2",
                 "extensions": { "durable-evidence": { "retention": "1y" } } } }
  ]
}
```

On v1 requirements the declaration lives at `extra.extensions["durable-evidence"]`,
mirroring the top-level `extensions` map of v2 requirements so that the v1→v2
conversion is a rename. Existing keys in `extra` (the EIP-712 domain) MUST be
preserved.

The resource server sees which offer was paid — the satisfied
`PaymentRequirements` travels in the payload — and decides from that:

| Route offers a declared entry? | Paid entry declares it? | Behaviour |
|---|---|---|
| yes | yes | anchor, on the **paid entry's** `mode`/`retention` |
| yes | no | deliver, no evidence, header `{"skipped":"not_selected"}` |
| no | — | anchor on the route's config (pre-offer behaviour, unchanged) |

Rules that follow:

- **No change to the x402 core.** A client that does not know the extension
  takes the plain entry and everything degrades cleanly.
- **The paid entry's terms win.** A buyer who paid for `1y` and was anchored
  for `90d` holds a receipt that contradicts what they bought.
- **Order-independent.** A conformant client MUST NOT let listing order decide;
  a client that has not asked for evidence takes the entry without it, and one
  that has takes the entry with it, wherever the seller placed them.
- **A malformed declaration reads as "not declared"**, never as an error. One
  seller's typo must not make its route unpayable.

## 5. Encryption

```
CEK          := random 32 bytes
ciphertext   := AES-256-GCM(key=CEK, nonce=random 12B, plaintext=body, aad=paymentId)
for each recipient:
  eph        := ephemeral keypair on the recipient's curve
  shared     := ECDH(eph.private, recipientPubKey)
  wrapKey    := HKDF-SHA256(ikm=shared, salt=paymentId, info="DX402-v1-wrap")
  wrappedCEK := AES-256-GCM(key=wrapKey, nonce=random 12B, plaintext=CEK)
```

secp256k1 recipients use secp256k1 ECDH; ed25519 recipients have their key
mapped to X25519 (birational equivalence). `keyAlg` records which.

**`paymentId` is the AEAD associated data.** Derive it differently on either
side and decryption fails with no cause visible.

## 6. Envelope format

The body ciphertext is stored **once**; every recipient unwraps the same CEK.
Adding a recipient costs ~98 bytes, not a second copy of the payload — which is
what makes bidirectional evidence practically free.

```
v1 (one recipient, the payer):
  "DX402" | 0x01 | alg | eph_len | eph | cek_nonce(12) | wrapped_len(2) | wrapped | body_nonce(12) | ciphertext

v2 (several):
  "DX402" | 0x02 | count | count × ( role | alg | eph_len | eph | cek_nonce | wrapped_len | wrapped ) | body_nonce | ciphertext
```

`role` ∈ { `payer`, `seller`, `auditor` }. **A single-payer envelope MUST be
emitted as v1, byte-for-byte**, so every reader already deployed keeps working
and a v2 blob is a positive signal that someone besides the payer can open it.
Roles are readable without decrypting.

A holder tries every slot: in a multi-recipient envelope the payer is not
necessarily first.

**The seller's encryption key is not its payment key.** Reusing the key that
receives funds to decrypt evidence turns *"someone read my evidence"* into
*"someone emptied my wallet"*. This is also what lets a seller in custody take
part: the decrypt-only key can be local while the payment key lives elsewhere.

## 7. Wire format

### 7.1 `X-Durable-Evidence` response header

Base64url JSON, emitted alongside `X-Payment-Response`:

```jsonc
{
  "v": 1,
  "paymentId": "0x…",
  "pointer": "ipfs+https://…/dx402/blob/0x…#bafk…",
  "backend": "ipfs",
  "contentHash": "0x…",          // keccak256 of the PLAINTEXT body
  "cipher": "AES-256-GCM",
  "keyAlg": "ECIES-secp256k1",   // or "ECIES-X25519"
  "mode": "direct",
  "retention": "90d",
  "receipt": "0x…",              // EIP-712 signature by the facilitator (§12)
  "verified": false,             // §9 — the chain confirmed authorship
  "signed": false,               // §9 — the declared payee signed
  "notVerifiedReason": "dx402_proof_missing"
}
```

`contentHash` is over the **plaintext**. Over the ciphertext it would only prove
the blob was not corrupted; over the plaintext it proves the anchor decrypts to
what was actually delivered.

### 7.2 `SettleResponse.extensions`

The facilitator echoes the same object under `extensions["durable-evidence"]`.

### 7.3 Skip signalling

```jsonc
{ "v": 1, "skipped": "too_large" }
```

`too_large` | `busy` | `anchor_failed` | `no_payer_key` | `disabled` | `not_selected`.

`busy` is distinct from `anchor_failed` on purpose: nothing broke, the
deployment refused to buffer one more large body. `not_selected` is the buyer's
own choice (§4.2). Clients MUST treat `skipped` as an open set.

**A failure to anchor MUST NOT fail the payment.** Every error path yields a
skip; the buyer receives exactly the bytes the handler produced.

## 8. Anchoring: `POST /dx402/anchor`

Metadata only; never plaintext.

| Field | Required | Meaning |
|---|---|---|
| `paymentId`, `network`, `txHash` | yes | `network` accepts v1 names and CAIP-2 |
| `payer` | yes | The address the envelope is **sealed to** |
| `payee` | yes | Who the caller says got paid (diagnostic only — see §9) |
| `pointer` **or** `sealed` | one | Supply a pointer to your own store, or the ciphertext for the facilitator to host |
| `backend`, `contentHash`, `keyAlg`, `mode`, `retention` | yes | |
| `proofOfPayment` | for certification | The settle response's proof; verified on-chain |
| `sellerSignature` | for certification | EIP-712 (§11) or ed25519 signature by the payee |
| `escrowRelease` | on the escrow rail | §10 |

**`backend` is measured, not declared.** The facilitator records the backend
that actually took the bytes; a composed store may write to its fallback.

## 9. The gate and the authority ladder

Every anchor is judged; **phase 1** (default) verifies and reports, **phase 2**
(`DX402_REQUIRE_PROOF=true`) refuses. Verdicts:

| Verdict | Enforceable in phase 2 |
|---|---|
| `dx402_proof_missing`, `dx402_proof_invalid` | yes |
| `dx402_payer_is_not_recipient` — the payer is not the address sealed to | yes |
| `dx402_seller_signature_missing` / `_invalid` | yes |
| `dx402_payment_id_not_bound` — the proof is for a different transaction | yes |
| `dx402_escrow_release_missing` / `_invalid` / `_ambiguous` (§10) | yes |
| `dx402_replayed` | yes |
| `dx402_unverifiable_chain` — non-EVM, no receipt to read | **never** |
| `dx402_rpc_unavailable` — no verdict was reached | **never** |

The two that never block exist so an implementation's blind spots cannot erase
somebody's evidence: an outage is not a fraud verdict, and an unchecked chain is
not a rejected one.

**Freshness** is measured against the **block's** timestamp, default **900 s**.
A DX402 anchor happens in the same handler as the settle; a wider window only
widens the attack surface.

**Authority ladder.** A record is one of:

| Rung | `verified` | `signed` | Meaning |
|---|---|---|---|
| 0 provisional | false | false | Anyone could have written this. Holds the slot so a squatter cannot |
| 1 signed | false | true | The signature matches the payee **the caller declared** — a diagnostic, proves nothing |
| 2 verified | true | — | The **chain** says this address got paid and it signed |

A weaker claim never locks out a stronger one; a stronger one supersedes. Only
rung 2 is final. This asymmetry is what turns anti-replay from a weapon (whoever
anchored first owned the payment forever) into a guarantee.

**`payee` on the request is diagnostic only.** Finality comes from the payee
the gate reads **off the chain**. A signature over a self-declared payee proves
"I control the address I typed", which any observer of a settlement can do.

**Two facts a resource server must know:** (a) the proof must declare the
transfer the payee **actually received** — a marketplace release carries a fee
transfer too; (b) a `sellerSignature` that does not verify is answered
`dx402_signature_not_verified` (422), never `dx402_already_anchored` (409). The
second is true and plausible and sends the integrator to audit idempotency; the
first names the cause.

## 10. The escrow rail

On plain x402 the buyer is the ERC-20 `from`. On an x402r escrow release the
tokens leave the operator's **TokenStore**, and the buyer never appears as a
`from` — measured 2026-09-02 on 23 of 23 sampled live Execution Market releases.
Without this section, phase 2 rejects the whole rail as fraudulent.

Such an anchor carries:

```jsonc
"escrowRelease": {
  "paymentInfo": { "operator", "receiver", "token", "maxAmount" /* string */,
                   "preApprovalExpiry", "authorizationExpiry", "refundExpiry",
                   "minFeeBps", "maxFeeBps", "feeReceiver", "salt" },
  "payer": "0x…"      // who funded the escrow
}
```

Nothing is trusted. The facilitator calls `getHash(paymentInfo)` **on the
escrow contract** and requires the answer to equal a `paymentInfoHash` that
**this transaction** captured (`PaymentCaptured` / `PaymentCharged`), from the
escrow address it knows for that network. That binds three things at once: the
authorization is authentic (any edited field changes the hash), it belongs to
this payment, and it came from the real escrow. `paymentInfo.receiver` MUST
equal the payee the proof was checked against.

A transaction that settles **more than one** escrow payment answers
`dx402_escrow_release_ambiguous`: `paymentId` is a function of `(network, txHash)`,
so batched payments collide on it and certifying either would be a guess.

**Who signs.** The payee of the release — on a marketplace, the seller who got
paid, not the platform. A platform that is neither payer nor payee anchors
provisionally; the seller supersedes it with a signed anchor, reusing the
existing `pointer` and `contentHash` without re-uploading.

## 11. Anchor authorization (what the seller signs)

```
domain = { name: "DX402 Anchor", version: "1", chainId: <settlement chain> }

Dx402AnchorAuthorization {
  bytes32 paymentId;
  bytes32 contentHash;
  string  pointer;      // "" when `sealed` is sent and the facilitator issues the pointer
  address payee;        // the signer's address; the ZERO address for ed25519 payees
}
```

Every field is bound: a signature over `paymentId` alone could be lifted onto
different content. ed25519 payees (Solana, Stellar) sign the **same** EIP-712
digest with a raw ed25519 signature — one canonical message across curves —
and the binding to the payee is established by which key verifies.

Signing is a **callable** in the reference SDKs (`signer(digest) -> signature`),
so a custodian can sign the 32-byte digest without the seed leaving it. A
custodian that applies an EIP-191 prefix produces a valid signature over another
message; conformant clients SHOULD recover locally before posting.

## 12. Evidence receipt (what the facilitator signs)

```
domain = { name: "DX402 Evidence", version: "1", chainId: <settlement chain> }

Dx402EvidenceReceipt {
  bytes32 paymentId;
  bytes32 contentHash;
  string  pointer;
  address payer;
  address payee;
  bytes32 txHash;
  uint8   mode;             // 0 = direct
  uint64  anchoredAt;
  uint64  retentionUntil;   // 0 = permanent
}
```

Field order is normative (part of the type hash). Verifiable offline by any
third party. If the stored pointer differs from the predicted one (a fallback
store), the receipt is **re-signed** over the real pointer; a signed receipt
naming an object that does not exist is worse than none.

## 13. Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/dx402/anchor` | Register evidence (§8) |
| `GET` | `/dx402/evidence/{paymentId}` | Pointer, hashes, receipt, `verified`/`signed` |
| `GET` | `/dx402/receipt/{paymentId}` | Signed receipt alone |
| `GET` | `/dx402/blob/{paymentId}` | The ciphertext, when the facilitator hosts it (private bucket; pointers address the *payment*) |
| `GET` | `/dx402/stats` | Count (a floor) and the backends actually offered, with `revocable`/`public` |
| `POST` | `/dx402/recover` | **501** — `escrowed` mode is not served (§17) |

`/supported` lists `durable-evidence` under `extensions` **only when the
facilitator can actually serve it**. `404` (never existed) and `410` (expired)
are different answers to a dispute and MUST NOT be collapsed.

## 14. Error codes

| Code | HTTP | Meaning |
|---|---|---|
| `dx402_disabled` | 404 | Not enabled on this facilitator |
| `dx402_unknown_payment` | 404 | No evidence for `paymentId` |
| `dx402_evidence_expired` | 410 | Past `retentionUntil` |
| `dx402_proof_rejected` | 402 | The gate refused (phase 2 only; §9 names the verdict) |
| `dx402_already_anchored` | 409 | Not more authoritative than the record held |
| `dx402_signature_not_verified` | 422 | A `sellerSignature` was supplied and did not verify |
| `dx402_store_unavailable` | 503 | Backend unreachable; carries `"retryable": true` |

Callers MUST NOT persist a retryable failure as a permanent "no evidence".

## 15. Security considerations

1. **Anchoring is publishing.** `permanent` is irrevocable; deployments MUST
   default to bounded retention (90 d). The public-IPFS backend is refused
   without explicit buyer opt-in.
2. **Harvest-now-decrypt-later.** ECDH is not post-quantum. Do not anchor
   permanently what must not survive.
3. **The anchor is claimable by anyone who observes a settlement** — `paymentId`
   is public. §9's ladder is the defence; an implementation without it hands the
   real seller's slot to whoever anchors first.
4. **Self-declared finality is a hijack.** Certifying "I control the address I
   typed" lets an observer supersede the real seller. Finality MUST come from
   the payee the chain reports.
5. **Escrow releases hide the buyer** (§10). An implementation that reads the
   buyer off the `Transfer` alone rejects every escrow-mediated payment.
6. **Batched settlements collide on `paymentId`.** Refuse, do not guess.
7. **Memory.** A capture costs ~5× the body (measured). Bound concurrency, and
   deny rather than queue — buffering sits ahead of a delivery already paid for.
8. **`contentHash` binds delivery to evidence.** Buyers SHOULD verify it.

## 16. Test vectors

Vectors are pinned against **independent implementations** (Rust, Python,
TypeScript open each other's envelopes; Rust verifies both SDKs' anchor
signatures), never against an implementation's own output. Three fabricated
hash variants once passed CI for months by being compared only to themselves.

## 17. Out of scope for v0.2

Stated here so nothing above is read as a promise:

- **Non-EVM certification.** Solana, NEAR, Stellar, Algorand anchor and sign,
  but the on-chain half of the gate reads an EVM receipt; they report
  `unverifiable_chain` and are never refused.
- **`escrowed` mode** (facilitator-held CEK, `/recover`). Not served. A
  **declared read key** — the buyer registers a decrypt-only public key, signed
  by the address it claims — covers custody, EIP-7702 and smart accounts without
  centralising decryption, and is the recommended pattern.
- **`derived` mode** for browser wallets (deterministic signature → HKDF),
  pending RFC 6979 determinism across vendors.
- On-chain anchoring of receipt digests.

## 18. Relationship to existing extensions

| Extension | Proves |
|---|---|
| `offer-receipt` | Terms were agreed and something was delivered |
| `payment-identifier` | A stable handle for the payment (reused as `paymentId` when present) |
| **`durable-evidence`** | **What** was delivered, retrievable later, readable only by the parties |
