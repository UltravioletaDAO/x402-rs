# DX402 — `durable-evidence` Extension Specification v0.1

**Status:** Draft. Implemented in `x402-rs` before proposal upstream.
**Extension key:** `durable-evidence`
**Product name:** DX402
**Authors:** Ultravioleta DAO
**Date:** 2026-08-14

---

## 1. Motivation

x402 settles payment durably on-chain but delivers the purchased resource
exactly once, in the body of a `200 OK`, and retains nothing. Settlement is
permanent; **delivery is not**.

This asymmetry is documented in the literature — *Five Attacks on x402 Agentic
Payment Protocol* observes that "the HTTP response has already been sent, so RR
cannot claw the resource back", and explicitly does not address non-repudiation
or dispute resolution.

The consequence: a buyer who did not persist the response at the moment of
delivery has no way to recover it, and neither party can later prove **what** was
delivered — only **that** payment occurred.

Existing extensions (`offer-receipt`) and the IETF receipt drafts sign *metadata
about* the exchange. None persist the delivered content.

`durable-evidence` closes that gap with three properties:

1. **Durable** — the delivered body survives the session.
2. **Private** — the body is encrypted to the payer; no third party, including
   the facilitator and the storage backend, can read it.
3. **Coupled** — no prior registration and no extra round trip. The encryption
   key material is produced by the act of paying.

## 2. Core insight

> A payment authorization is a digital signature. A digital signature yields the
> signer's **public key**, not merely their address. Therefore the resource
> server can encrypt to the payer using key material the payment itself
> produced.

Paying *is* publishing your encryption key.

### 2.1 Payer public key availability

| Family | Curve | Source of payer public key |
|---|---|---|
| EVM | secp256k1 | ECDSA public-key recovery over the EIP-712/EIP-3009 signature |
| Solana / Fogo | ed25519 | Address **is** the public key; map ed25519 → X25519 for ECDH |
| NEAR | ed25519 | Account access key (`ed25519:…`) |
| Stellar | ed25519 | Address (`G…`) is the encoded public key |
| Algorand | ed25519 | Address is public key + checksum |
| Sui | ed25519 / secp256k1 | Address is a *hash* of the key; the **signature** carries the key |
| XRPL | secp256k1 / ed25519 | `SigningPubKey` field of the signed transaction |

All seven families expose the payer's public key without an additional request.

## 3. Terminology

- **Resource Server (RS)** — the seller. Holds the plaintext body. Encrypts and
  anchors.
- **Facilitator** — verifies and settles payment. Acts as **notary and index**
  for evidence. In `direct` mode it never holds plaintext or key material.
- **Evidence Store** — content-addressed or key-addressed durable storage.
- **CEK** — Content Encryption Key. Random, per-response, 256-bit.
- **Pointer** — a URI locating the ciphertext.
- **paymentId** — stable identifier for the payment, per the `payment-identifier`
  extension where present; otherwise `keccak256(network ‖ txHash ‖ nonce)`.

## 4. Declaration

Declared per route, keyed by the extension key, following the registry
convention:

```jsonc
{
  "extensions": {
    "durable-evidence": {
      "mode": "direct",            // "direct" | "escrowed"
      "backend": "s3",             // "s3" | "ipfs" | "arweave"
      "retention": "90d",          // "90d" | "1y" | "permanent"
      "maxBodyBytes": 33554432,    // above this, skip (never fail the payment)
      "paidBy": "seller"           // "seller" | "buyer"
    }
  }
}
```

All fields optional. Defaults: `mode=direct`, `backend=s3`, `retention=90d`,
`maxBodyBytes=33554432` (32 MiB), `paidBy=seller`.

`maxBodyBytes` is a **memory** bound on the seller, not a storage bound: sealing
holds the plaintext and the ciphertext at once. An implementation that raises it
has to bound concurrency alongside it, or the same setting that promises bigger
evidence delivers an OOM. See `skipped: "busy"` below.

## 5. Encryption

### 5.1 `direct` mode (default, end-to-end)

```
CEK          := random 32 bytes
ciphertext   := AES-256-GCM(key=CEK, nonce=random 12B, plaintext=body, aad=paymentId)
payerPubKey  := recover(payment signature)          // §2.1
eph          := ephemeral keypair on the payer's curve
shared       := ECDH(eph.private, payerPubKey)
wrapKey      := HKDF-SHA256(ikm=shared, salt=paymentId, info="DX402-v1-wrap")
wrappedCEK   := AES-256-GCM(key=wrapKey, nonce=random 12B, plaintext=CEK)
```

The anchored artifact is `ciphertext ‖ wrappedCEK ‖ eph.public ‖ nonces`. Only
the holder of the payer's private key can derive `wrapKey`.

**Curve handling.** secp256k1 payers use secp256k1 ECDH directly. ed25519 payers
have their key mapped to X25519 (birational equivalence) and use X25519 ECDH; the
receipt records `keyAlg` so a verifier knows which was used.

### 5.2 `escrowed` mode (fallback)

Identical, except `wrappedCEK` is encrypted to the **facilitator's** key and
released via `POST /dx402/recover` against a payer signature. The facilitator can
technically decrypt. This MUST be recorded as `"mode": "escrowed"` in the receipt
so no verifier confuses its guarantee with `direct`.

## 6. Wire format

### 6.1 `X-Durable-Evidence` response header

Base64url-encoded JSON, emitted by the RS alongside `X-Payment-Response`:

```jsonc
{
  "v": 1,
  "paymentId": "0x…",
  "pointer": "s3+https://evidence.ultravioletadao.xyz/0xabc…",
  "backend": "s3",
  "contentHash": "0x…",       // keccak256 of the PLAINTEXT body
  "cipher": "AES-256-GCM",
  "keyAlg": "ECIES-secp256k1", // or "ECIES-X25519"
  "mode": "direct",
  "retention": "90d",
  "receipt": "0x…"             // EIP-712 signature by the facilitator
}
```

`contentHash` is over the **plaintext**. This lets the buyer verify that the
anchored ciphertext decrypts to exactly the body they were served — detecting a
seller that anchors something other than what it delivered.

### 6.2 `SettleResponse.extensions`

The facilitator echoes the same object under
`extensions["durable-evidence"]`, so clients that only read the settle response
still receive the pointer.

### 6.3 Skip signalling

When evidence is not produced, the header carries a reason instead and the
payment proceeds normally:

```jsonc
{ "v": 1, "skipped": "too_large" }   // too_large | busy | anchor_failed | no_payer_key | disabled
```

**A failure to anchor MUST NOT fail the payment.**

## 7. `EvidenceReceipt` (EIP-712)

Signed by the facilitator, verifiable offline by any third party.

```
domain = {
  name:    "DX402 Evidence",
  version: "1",
  chainId: <settlement chain id>
}

EvidenceReceipt {
  bytes32 paymentId;
  bytes32 contentHash;
  string  pointer;
  address payer;
  address payee;
  bytes32 txHash;
  uint8   mode;        // 0 = direct, 1 = escrowed
  uint64  anchoredAt;
  uint64  retentionUntil;   // 0 = permanent
}
```

## 8. Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/dx402/anchor` | RS registers evidence metadata. Never carries plaintext. |
| `GET` | `/dx402/evidence/{paymentId}` | Pointer + contentHash + receipt |
| `GET` | `/dx402/receipt/{paymentId}` | Signed receipt alone, for offline verification |
| `POST` | `/dx402/recover` | `escrowed` mode only: release CEK against payer signature |

### 8.1 Recovery challenge (`escrowed` only)

EIP-712, replay-protected:

```
domain = { name: "DX402 Recovery", version: "1", chainId: <chain> }

RecoveryRequest {
  bytes32 paymentId;
  address payer;
  bytes32 nonce;      // server-issued, single-use
  uint64  issuedAt;
  uint64  expiresAt;  // issuedAt + 300s max
}
```

The facilitator MUST verify: signature recovers to the recorded payer; `nonce`
is unused and was issued by this facilitator; `now` within
`[issuedAt, expiresAt]`; `paymentId` matches a settled payment whose payer is the
signer. A nonce is consumed on first successful use.

## 9. Error codes

| Code | Meaning |
|---|---|
| `dx402_disabled` | Extension not enabled on this facilitator |
| `dx402_unknown_payment` | No settled payment for `paymentId` |
| `dx402_not_payer` | Signature does not recover to the recorded payer |
| `dx402_challenge_expired` | Outside the validity window |
| `dx402_challenge_replayed` | Nonce already consumed |
| `dx402_direct_mode` | `/recover` called for a `direct`-mode payment (no key held) |
| `dx402_evidence_expired` | Past `retentionUntil` |
| `dx402_store_unavailable` | Backend unreachable (retryable) |
| `dx402_proof_rejected` | The anchor gate refused it: no proof, a proof that did not check out, or evidence sealed to somebody who did not pay (402) |
| `dx402_already_anchored` | This payment already has evidence and the incoming anchor is not more authoritative (409) |
| `dx402_signature_not_verified` | A `sellerSignature` was supplied and did not verify against the payee, so it could not supersede the record holding this payment (422) |

An implementation MUST NOT answer `dx402_already_anchored` when a
`sellerSignature` was supplied and failed to verify. Both statements are true,
but only the second names the cause, and the first is *plausible* — it sends the
integrator to audit idempotency, where suspects always exist, and never to the
signature. A correct error that explains the wrong thing costs more than a vague
one.

An implementation MUST NOT refuse to anchor solely because a supplied
`sellerSignature` did not verify. If nothing else holds the slot the record is
written **provisionally**, so a seller-side signing bug never costs the buyer its
evidence. `verified: false` on the response is how the seller learns.

`dx402_store_unavailable` is retryable and MUST carry `"retryable": true`.
Following the rule established for `/identity/:network/owner/:address`, callers
MUST NOT persist a retryable failure as a permanent "no evidence" answer.

## 10. Security considerations

1. **Anchoring is publishing.** Content anchored with `retention: permanent` is
   irrevocable. Deployments MUST default to a bounded retention.
2. **Harvest-now-decrypt-later.** ECDH over secp256k1 is not post-quantum. A blob
   anchored permanently today may be readable later. Do not anchor permanently
   what must not survive.
3. **`escrowed` centralises risk.** Compromise of the facilitator's key exposes
   every `escrowed` payload. `direct` mode has no such key.
4. **Replay.** §8.1 is mandatory; a captured old signature must not open a blob.
5. **Cache leakage mitigation.** Attack III of the referenced paper measured 100%
   cache leakage of paid responses through nginx. Bodies encrypted to the payer
   are useless to an intermediary that caches them. This is a beneficial
   side effect, not the primary goal.
6. **`contentHash` binds delivery to evidence.** Buyers SHOULD verify it.

## 11. Test vectors

Fixed vectors live in `tests/dx402/vectors.json` and are pinned against
**independent implementations**, not against this implementation's own output.

> This rule exists because three fabricated SHA-256 variants of ERC-8004 SEAL v1
> passed CI for months by being compared only to themselves. A self-referential
> vector proves nothing.

Each vector fixes: payer private key, body, CEK, ephemeral key, and the expected
ciphertext, wrapped CEK, and `contentHash`.

## 12. Relationship to existing extensions

| Extension | Proves |
|---|---|
| `offer-receipt` | Terms were agreed and something was delivered |
| `payment-identifier` | A stable handle for the payment |
| **`durable-evidence`** | **What** was delivered, retrievable later, readable only by the payer |

`durable-evidence` composes with both; it reuses `payment-identifier` for
`paymentId` when present.

## 13. Status of the v0.2 items

Design notes: **[04-BACKLOG-MONETIZACION.md](04-BACKLOG-MONETIZACION.md)** and
**[05-DISENO-v0.2.md](05-DISENO-v0.2.md)**.

### Done — both former upstream blockers are closed

1. **Multi-recipient envelopes** — shipped in v1.79.0. The envelope carries
   `payer`, optionally `seller`, optionally an `auditor`. The body is encrypted
   once; only the content key is wrapped per recipient. A single-payer envelope
   is still emitted as format v1 byte-for-byte, so nothing already anchored
   becomes unreadable, and roles are readable from the blob without decrypting.
2. **The anchor gate** — shipped in v1.78.0. Every anchor is checked against the
   chain, the payer must be the address the evidence was sealed to, the payee
   must have signed the anchor, and one payment anchors once. Phase 1 by default
   (`DX402_REQUIRE_PROOF=false`: verify and report).

### Done — the escrow rail (v2.10.0)

The gate read the buyer off the ERC-20 `Transfer`. That is correct for plain
x402 and **wrong for x402r escrow**, where a release moves tokens out of the
operator's TokenStore and the buyer never appears as a `from`. Measured
2026-09-02: 23 of 23 sampled live Execution Market releases, across Avalanche,
Optimism and Monad, reported a payer that was not the buyer. Phase 2 would have
rejected the entire escrow rail as fraudulent — 690 of the 699 anchors then in
production.

An anchor on that rail now carries `escrowRelease` (the authorization plus the
funder). The facilitator asks the escrow to `getHash` it and requires the answer
to be a `paymentInfoHash` **that same transaction** captured, which makes the
buyer an on-chain fact: any edited field changes the hash. Verified against
Optimism `0x5a2822cc…`, where `getHash` returns exactly the `0xb54c89bf…` the
transaction emitted, and the `payer` inside is the buyer the seller had sealed
to.

Two honest limits, both refusals rather than guesses:

- A transaction settling **more than one** escrow payment is refused
  (`dx402_escrow_release_ambiguous`). `paymentId` is `keccak256(caip2 || txHash)`,
  so batched payments collide on it and certifying either would be a coin flip.
- The 900-second freshness window still applies, so a supersede must arrive
  promptly. Certifying anchors already in production means widening
  `DX402_ANCHOR_MAX_AGE_SECS` deliberately and temporarily — not changing the
  default.

### Still open

- **Buyer opt-in through the existing `accepts` array.** A seller offers the same
  resource twice — plain, and with `durable-evidence` at a higher price — and the
  buyer picks. Needs no change to the x402 core, which is worth stating
  explicitly in the proposal.
- **The anchor gate on non-EVM chains.** `verify_payment_facts` reads an EVM
  receipt; Solana, NEAR, Stellar and Algorand report `unverifiable_chain`, which
  is never enforced. Closing it means writing a per-family verification, not
  reusing one.

### Not blocking

- `derived` mode for browser wallets (deterministic EIP-712 signature → HKDF →
  X25519). Blocked on validating RFC 6979 determinism across wallet vendors.
- On-chain anchoring of receipt digests.
