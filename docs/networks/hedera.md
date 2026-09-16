# Hedera — getting paid in HBAR or HTS tokens through this facilitator

> **TEMPLATE. Hedera is not integrated.** This facilitator has no Hedera network today:
> `GET /supported` lists none, and `POST /verify` refuses the scheme's own example
> body with HTTP `400` (both measured 2026-09-16).
> This page is the skeleton the Hedera integration will fill in. It has the same
> sections as [the Arc page](arc.md), in the same order, so the two read alike.
>
> **Every `[GAP: …]` is a value this facilitator has not decided or measured yet.**
> Fill each one from the implementation or from a measurement, with a date, and never
> from memory. The values that ARE written below come from public sources — the
> x402 `exact` scheme for Hedera and the Hedera Mirror Node — and say so. They describe
> the chain and the protocol, not a capability of this facilitator.

**Hedera is not EVM, and nothing from the Arc page carries over by analogy.** A
Hedera x402 payment is not an EIP-3009 authorization: there is no EIP-712 domain, no
`validBefore`/`nonce` pair and no chain id. The buyer signs a native Hedera
`TransferTransaction`, and the facilitator co-signs it as the fee payer. Hedera also
runs an EVM-compatible layer with its own chain ids (`eip155:295` mainnet,
`eip155:296` testnet); **those are not the payment network.** This facilitator
carried them for ERC-8004 from 2026-04-04 and removed them on 2026-05-30 because no
x402 payment ever ran on them.

**Accounts do not look like addresses.** A Hedera account is an entity id of the form
`shard.realm.num` — `0.0.1234` — not a `0x…` address and not a public key. The same
shape names tokens (`0.0.456858`) and HBAR itself (`0.0.0`, which is not a "zero
address"). A dotted string like that is only meaningful next to a `hedera:*` network.

---

## At a glance

| | Hedera testnet | Hedera mainnet |
|---|---|---|
| Network, x402 v2 (CAIP-2) | `hedera:testnet` | `hedera:mainnet` |
| Network, x402 v1 name | [GAP: whether a v1 name exists at all — the reference ecosystem is v2] | [GAP] |
| Chain id | none — not an EVM network | none |
| Family | Hedera (native) | Hedera (native) |
| Scheme | `exact` — a partially signed `TransferTransaction` | `exact` |
| Assets | [GAP: which of HBAR / USDC / other HTS tokens this facilitator accepts] | [GAP] |
| Fee payer (`extra.feePayer`) | [GAP: this facilitator's own Hedera account id — not created yet] | [GAP] |
| Gas | paid by the fee payer, in HBAR. The buyer signs and pays nothing else | same |
| Facilitator fee | [GAP: expected none, as on every other network — confirm] | [GAP] |
| Keys | [GAP: which of Ed25519, ECDSA secp256k1, KeyList and threshold keys are accepted, each with a positive payment] | [GAP] |

---

## 1. Is it live?

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -c '[.kinds[] | select(.network | startswith("hedera")) | {x402Version, scheme, network}]'
```

- `[]` — **not served.** On 2026-09-16 it printed `[]`.
- [GAP: the exact entries a served Hedera network shows, copied from `/supported`
  once it is live — including where `feePayer` is published.]

| Deployment | `POST /verify` with `"network": "hedera:testnet"` |
|---|---|
| **Production today** (`/version` = `2.29.6`, measured 2026-09-16) | HTTP `400` `invalid_request_body` for the scheme document's own example body — the running binary has no Hedera payload shape |
| A release that contains Hedera, switched off | [GAP] |
| Hedera served | answered on the merits |

## 2. The asset, and the trap in its units

Amounts are integers in the asset's smallest unit (x402 `exact` scheme for Hedera):

| Asset | Asset id, testnet | Asset id, mainnet | Decimals | Accepted here |
|---|---|---|---|---|
| HBAR | `0.0.0` | `0.0.0` | 8 — amounts in **tinybars**, 1 HBAR = 100,000,000 | [GAP] |
| USDC (native HTS) | `0.0.429274` | `0.0.456858` | 6 | [GAP] |

The two USDC ids were read from the public Mirror Node on 2026-09-16: both answer
`symbol` `USDC`, `decimals` `6`, `type` `FUNGIBLE_COMMON`, not deleted, and no fixed or
fractional custom fees. Token metadata can change; re-read it before relying on it.

The traps, as far as they are known before the integration exists:

- **HBAR and USDC have different decimals under the same account.** 1 HBAR is
  `"100000000"`; 1 USDC is `"1000000"`. An amount written for the wrong asset is off
  by a factor of 100.
- **HBAR is not a dollar.** A price in USD needs a conversion to HBAR; nothing does it
  implicitly.
- [GAP: an HTS token with custom fees — whether it is refused, and how. The payee
  must receive exactly `amount`.]
- [GAP: a `payTo` that has not associated the HTS token. The scheme names "invalid
  token association" as a failure a facilitator should pre-check; measure what the
  chain answers and what this facilitator reports.]

## 3. What the seller puts in the 402

The shape from the x402 `exact` scheme for Hedera, with this facilitator's values left
open:

```json
{
  "scheme": "exact",
  "network": "hedera:testnet",
  "asset": "[GAP: an accepted asset id]",
  "amount": "[GAP: integer, in that asset's smallest unit]",
  "payTo": "[GAP: the seller's Hedera account id]",
  "maxTimeoutSeconds": "[GAP: an integer; decide it]",
  "extra": {
    "feePayer": "[GAP: this facilitator's Hedera account id]"
  }
}
```

- `extra.feePayer` is **required** here, unlike on EVM networks: the buyer builds the
  transaction around it. [GAP: where a seller reads it from — `/supported`, `/accepts`,
  or both.]
- [GAP: whether `payTo` may be an account alias (an EVM address or a public key). The
  scheme leaves the policy to each facilitator and asks that it be documented: an
  HBAR transfer to an alias can create the account, and the facilitator ends up
  funding that creation. State the policy here.]

## 4. What the buyer signs

Per the scheme, the buyer:

1. builds a `TransferTransaction` moving `amount` of `asset` from their account to
   `payTo`;
2. sets the transaction id's account to `extra.feePayer`;
3. signs it — a **partially signed** transaction, still missing the fee payer's
   signature;
4. sends it Base64-encoded as `payload.transaction`.

```json
{
  "transaction": "[GAP: a real Base64 payload from the official client, once one has been verified here]"
}
```

- [GAP: the client and version this facilitator has been tested against.]
- [GAP: what the facilitator refuses inside that transaction. The scheme's own
  minimum is: anything but a direct `TransferTransaction`, operations other than the
  payment, token ids other than `asset`, and the fee payer as a sender. Name each
  refusal this facilitator makes and its reason token, once it has a test.]

## 5. What the facilitator does, and what comes back

1. `POST /verify` — the scheme requires the facilitator to decode the transaction,
   refuse anything but the expected transfer, and check that the payer's on-chain key
   actually signed it, before sponsoring anything. [GAP: the checks as implemented
   here, each with its test — not as planned.]
2. `POST /settle` — the scheme has the facilitator add the fee payer's signature and
   submit. [GAP: the replay rule — what makes the same payment settle once.]
3. What comes back: [GAP].
   The identifier is a **Hedera transaction id**, `0.0.<fee payer>@<seconds>.<nanos>`,
   not a 32-byte hash. The scheme document calls the field `transactionId`; the
   reference implementation returns it as `transaction`. [GAP: which one this
   facilitator returns, and what `payer` names — the account that paid or the fee
   payer.]
4. [GAP: what an unconfirmed settlement looks like, and the instruction not to ask the
   buyer to sign again.]

## 6. What does not work today

| | Status | Why |
|---|---|---|
| **Everything on this page** | not available | Hedera is not integrated. `/supported` lists no Hedera network |
| **Hedera through its EVM layer** (`eip155:295` / `eip155:296`) | not available, not planned as the payment path | Not EVM payments: the Hedera integration is the native `exact` scheme above |
| [GAP: each limitation of the first release — assets, key types, aliases, custom fees, extensions — with its reason] | | |

## 7. Testing on Hedera testnet

| | |
|---|---|
| Mirror Node | `https://testnet.mirrornode.hedera.com` (read for this page on 2026-09-16) |
| Consensus / JSON-RPC endpoint | [GAP] |
| Explorer | [GAP] |
| Faucet | [GAP] |

[GAP: anything that looks like a facilitator bug and is not — found during the
integration's end-to-end tests.]

## 8. Running your own facilitator with Hedera

[GAP: the cargo feature, the environment variables (account id, key, network
endpoint), and how the network is switched on and off.]

[GAP: what the fee payer account needs before it can settle — HBAR for fees, and any
token association.]

---

## Sources

- x402 `exact` scheme for Hedera, at commit `6b930273`:
  [`specs/schemes/exact/scheme_exact_hedera.md`](https://github.com/x402-foundation/x402/blob/6b9302737f16eea7de90b3bf617c045cef23e032/specs/schemes/exact/scheme_exact_hedera.md)
  (read 2026-09-16). Merged upstream on 2026-02-06.
- Reference implementation: [`@x402/hedera`](https://github.com/x402-foundation/x402/tree/6b9302737f16eea7de90b3bf617c045cef23e032/typescript/packages/mechanisms/hedera).
- Mirror Node token records, read 2026-09-16:
  [USDC testnet](https://testnet.mirrornode.hedera.com/api/v1/tokens/0.0.429274),
  [USDC mainnet](https://mainnet-public.mirrornode.hedera.com/api/v1/tokens/0.0.456858).
- This repository's history: Hedera EVM chains 295/296 added in `66d34e6c`
  (2026-04-04) and removed in `278842e5` (2026-05-30).
- The earlier reports in `docs/reports/` (`hedera-integration-analysis.md`,
  `hedera-x402-feasibility-2026-04.md`, both April 2026) read x402 on Hedera through
  EIP-3009 and treat a Hedera-native scheme as something still to be invented — two
  months after that scheme had entered upstream (merged 2026-02-06). Do not use them
  to integrate.
