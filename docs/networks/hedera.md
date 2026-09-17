# Hedera — getting paid in HBAR or HTS tokens through this facilitator

> **Status on 2026-09-17: both networks are live.** `GET /supported` lists
> `hedera:mainnet` and `hedera:testnet`, each under x402 **v2** with scheme `exact`.
> Measured against `https://facilitator.ultravioletadao.xyz` at `/version` **2.33.0**;
> mainnet arrived with that release, hours after testnet.
> [Check it yourself](#1-is-it-live) every time: `/supported` is the only list that is
> true today, and this page is not.

This is the integrator's page: what to put in a 402, what the buyer signs, what comes
back, and what does not work. Running a facilitator that serves Hedera — the cargo
feature, the environment variables, the daily budget, the settlement store, recovery
after an uncertain outcome and the rollout evidence — is
[Native Hedera payments](../guides/hedera-native.md).

**Hedera is not EVM, and nothing from [the Arc page](arc.md) carries over by analogy.**
A Hedera x402 payment is not an EIP-3009 authorization: there is no EIP-712 domain, no
`validBefore`/`nonce` pair and no chain id. The buyer signs a native Hedera
`TransferTransaction`, and the facilitator co-signs it as the fee payer. Hedera also
runs an EVM-compatible layer with its own chain ids (`eip155:295` mainnet,
`eip155:296` testnet); **those are not the payment network.** This facilitator carried
them for ERC-8004 from 2026-04-04 (`66d34e6c`) and removed them on 2026-05-30
(`278842e5`); nothing advertises them today.

**Accounts do not look like addresses.** A Hedera account is an entity id of the form
`shard.realm.num` — `0.0.1234` — not a `0x…` address and not a public key. The same
shape names tokens (`0.0.429274`) and HBAR itself (`0.0.0`, which is not a "zero
address"). **Aliases are refused**: `payTo`, the payer and the fee payer must each be a
canonical numeric entity id (`src/chain/hedera/id.rs`, `src/chain/hedera/codec.rs`).
That is deliberate — an HBAR transfer to an alias can create the account, and the fee
payer would end up funding that creation.

---

## At a glance

| | Hedera testnet | Hedera mainnet |
|---|---|---|
| Served on the production facilitator | **yes**, 2026-09-17 | **yes**, 2026-09-17 |
| Network, x402 v2 (CAIP-2) | `hedera:testnet` | `hedera:mainnet` |
| Network, x402 v1 name | **none.** Hedera is v2-only: `Network::supports_v1()` is false for it (`src/network.rs`), and `/supported` publishes `networkAliases: ["hedera:testnet"]` with no v1 spelling | none |
| Chain id | none — not an EVM network | none |
| Family | Hedera (native `CryptoTransfer`) | same |
| Scheme | `exact` only — a partially signed `TransferTransaction` | `exact` |
| Assets | HBAR `0.0.0` (8 decimals) and native USDC `0.0.429274` (6) | HBAR `0.0.0` (8) and native USDC `0.0.456858` (6) |
| Fee payer (`extra.feePayer`) | `0.0.10576385` | `0.0.10868300` — **a different account**; both published in `/supported` |
| Gas | paid by the fee payer, in HBAR. The buyer signs and pays nothing else | same |
| Facilitator fee | none — the sponsor pays consensus fees only | same |
| Keys | Ed25519, ECDSA secp256k1, KeyList and threshold keys all have passing vectors; **contract keys are refused** | same |

The fee payer is **network-specific**. Read it from `/supported` for the network you
are paying on; never carry one over from the other.

---

## 1. Is it live?

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -c '[.kinds[] | select(.network | startswith("hedera")) | {x402Version, scheme, network, feePayer: .extra.feePayer}]'
```

On 2026-09-17, at `/version` 2.33.0, that prints two entries:

```json
[{"x402Version":2,"scheme":"exact","network":"hedera:testnet","feePayer":"0.0.10576385"},
 {"x402Version":2,"scheme":"exact","network":"hedera:mainnet","feePayer":"0.0.10868300"}]
```

Each carries `extra.tokens`: `0.0.0` at 8 decimals, and USDC at 6 — `0.0.429274` on
testnet, `0.0.456858` on mainnet.

**Mainnet arrived the same day as testnet, and the fee payers are different accounts.**
Read `extra.feePayer` per network from this response; carrying the testnet one into a
mainnet 402 is refused as `fee payer mismatch or unsupported extension`.

A network being listed is necessary but not sufficient — check that it can actually
settle:

```bash
curl -s https://facilitator.ultravioletadao.xyz/health/ready \
  | jq -c '[.networks[] | select(.network | startswith("hedera"))]'
```

Measured 2026-09-17: both Hedera networks answered `status: "degraded"` with
`reason: "signer_gas_low"` — `rpc: "ok"` and `gasOk: true`, but only 28 settles
remaining on mainnet and 21 on testnet. That is the conservative canary budget
described in [section 6](#6-what-does-not-work-today), not an outage. Treat the Hedera
rail as low-throughput until that number grows.

## 2. The asset, and the trap in its units

Amounts are integer strings in the asset's own smallest unit:

| Asset | Testnet id | Mainnet id | Decimals | Smallest unit |
|---|---|---|---|---|
| HBAR | `0.0.0` | `0.0.0` | 8 | tinybar — 1 HBAR = `"100000000"` |
| Native USDC (HTS) | `0.0.429274` | `0.0.456858` | 6 | 1 USDC = `"1000000"` |

Both USDC ids were re-read from the public Mirror Node on 2026-09-17: each answers
`symbol` `USDC`, `decimals` `6`, `type` `FUNGIBLE_COMMON`, `deleted: false`, and the
testnet record carries no fixed or fractional custom fees. Token metadata can change;
the facilitator re-reads it on every payment rather than trusting a cached copy.

The traps:

- **HBAR and USDC have different decimals under the same account.** `"1000000"` is
  1 USDC but only 0.01 HBAR. An amount written for the wrong asset is off by a factor
  of 100 in one direction or the other.
- **HBAR is not a dollar.** A price in USD needs a conversion to HBAR; nothing here
  does it implicitly, and HBAR volume is never USD volume.
- **One asset per payment.** A transfer moving more than one token is refused
  (`multiple assets unsupported`).
- **The payee must already hold the association.** An HTS token has to be associated
  by both buyer and recipient, with KYC granted and freeze/pause clear, before the
  transfer can succeed.
- **Additional HTS tokens are opt-in and must declare their decimals.** An operator
  adds them as `token-id:decimals` pairs, which the facilitator then checks against
  fresh Mirror metadata on every payment. An asset not on that list is refused
  (`unsupported HTS asset`). NFTs are refused outright.

## 3. What the seller puts in the 402

```json
{
  "scheme": "exact",
  "network": "hedera:testnet",
  "asset": "0.0.429274",
  "amount": "1000",
  "payTo": "0.0.YOUR_MERCHANT",
  "maxTimeoutSeconds": 180,
  "extra": { "feePayer": "0.0.10576385" }
}
```

That body asks for **0.001 USDC**. Replace `payTo` with your own numeric account id,
and take `feePayer` from `/supported` rather than copying it from here.

- **`extra.feePayer` is required**, unlike on EVM networks: the buyer builds the
  transaction around it, setting the transaction id's account to that value. A payload
  whose fee payer does not match the one the facilitator runs is refused
  (`fee payer mismatch or unsupported extension`).
- **`maxTimeoutSeconds` bounds the transaction's valid duration.** The facilitator
  accepts a duration in `15..=180` seconds that is also `<= maxTimeoutSeconds`
  (`src/chain/hedera/codec.rs`). The official client's default duration is 120
  seconds, so a merchant asking for less than that will reject its own buyers — 180 is
  the safe ceiling and the value used in the test fixtures.
- `payTo` must be a numeric entity id. An EVM address or a public-key alias is refused.

## 4. What the buyer signs

The buyer:

1. builds a native `TransferTransaction` moving `amount` of `asset` from their account
   to `payTo`;
2. sets the transaction id's account to `extra.feePayer`;
3. signs it — a **partially signed** transaction, still missing the fee payer's
   signature;
4. sends it Base64-encoded as `payload.transaction`:

```json
{ "transaction": "<base64 of the signed TransferTransaction bytes>" }
```

The tested client is `@x402/hedera` **2.26.0** with `@hiero-ledger/sdk` 2.85.0
(`tests/hedera-e2e/package.json`). HBAR is not a default USD asset in that client:
add an explicit, bounded `spendControls.allowedAssets` entry for HBAR or a custom FT
rather than disabling spend controls.

Every frozen node variant in the submitted bytes is inspected, not just the first.
What the facilitator refuses inside the transaction, each with a vector in
`tests/hedera-e2e/vectors/`:

| Refusal | Meaning |
|---|---|
| `only native CryptoTransfer is supported` / `not a transfer` | anything but a direct transfer — scheduled, batched or contract calls |
| `unsupported Hedera protobuf field or operation` | an unknown field or a rider operation (an NFT transfer, an allowance hook) smuggled alongside the payment |
| `multiple assets unsupported` | more than one token moved |
| `payment amount mismatch` | the transfer does not move exactly `amount` |
| `account aliases unsupported` | an alias where a numeric entity id belongs |
| `contract or unsupported account key` | the payer's on-chain key is a contract key |
| `unsupported signature` / `invalid protobuf varint` | a malformed signature or envelope |

Adversarial vectors also cover a second node variant repointed to another recipient, a
stale signature, a wrong signer, an empty public-key prefix, an `is_approval` debit, a
duplicate node and a fee payer debited as a sender. Each is rejected **before** the
facilitator sponsors anything.

## 5. What the facilitator does, and what comes back

1. `POST /verify` decodes the transaction, applies every refusal above, and checks
   that the payer's on-chain key actually signed it — all before any sponsorship. It
   also reads admission state, and **fails closed** if that state cannot be read.
2. `POST /settle` reserves the transaction id, the intent fingerprint and the daily
   budget atomically, persists the exact co-signed bytes, then adds the fee payer's
   signature and submits. Only x402 v2 is accepted (`only x402 v2 is supported`).
3. On success the response carries:
   - `payer` — **the buyer's** account id, not the fee payer's;
   - `transaction` — the native **Hedera transaction id**,
     `0.0.<fee payer>@<seconds>.<nanos>`, not a 32-byte hash;
   - `network` — the CAIP-2 identifier.
4. **Retries return the original settlement.** The same payload settles once. Make
   fulfilment idempotent on that transaction id, or a repeated HTTP request delivers
   the same purchase twice.
5. After an uncertain answer, reconcile **the same payload and transaction id**. Do
   not create a new payment to replace an unknown outcome — resubmission reuses the
   same bytes and never regenerates a transaction id. An expired transaction with no
   authoritative outcome stays uncertain, and the absence of a record is not proof of
   failure. The recovery machinery is described in
   [Native Hedera payments](../guides/hedera-native.md).

A native transaction id renders on HashScan with dashes rather than `@` and `.`:
`0.0.10576385@1789609553.483480778` is
`https://hashscan.io/testnet/transaction/0.0.10576385-1789609553-483480778`.

## 6. What does not work today

| | Status | Why |
|---|---|---|
| **Sustained throughput** | budgeted, not open | Both networks read `degraded` / `signer_gas_low` on 2026-09-17, with 28 settles remaining on mainnet and 21 on testnet. See the budget note below |
| **x402 v1 on Hedera** | refused | Hedera is v2-only by construction (`supports_v1()` is false), and the settle path answers `only x402 v2 is supported` |
| **Hedera through its EVM layer** (`eip155:295` / `eip155:296`) | not available, not the payment path | Removed from this facilitator on 2026-05-30. The Hedera rail is the native `exact` scheme above |
| **Account aliases** (EVM address or public key) as `payTo`, payer or fee payer | refused | A canonical numeric entity id is required; an alias transfer can create an account at the sponsor's expense |
| **NFTs, allowances, hooks, scheduled and batch transactions, custom token fees** | refused before sponsorship | Each has an adversarial vector. A payee must receive exactly `amount` |
| **HTS tokens outside the allowlist** | refused | `unsupported HTS asset`. Extra fungible tokens need an explicit `token-id:decimals` entry from the operator and matching Mirror metadata |
| **`upto`, `escrow` / `commerce`, ERC-8004, DX402** | not on Hedera | `/supported` lists `exact` and only `exact`; neither `UPTO_DEPLOYED_NETWORKS` (`src/upto/types.rs`) nor `supported_networks()` (`src/erc8004/mod.rs`) names a Hedera network |

**Throughput is budgeted, not unlimited.** Admission charges the *maximum signed*
transaction fee once per admitted payment against a UTC-day budget shared by all
replicas, and it is not refunded when the actual fee is lower. The configured budgets
are canary-sized. Treat a `insufficient sponsor HBAR` refusal as a budget or funding
condition, not as a defect in your payload.

## 7. Testing on Hedera testnet

| | |
|---|---|
| Mirror Node | `https://testnet.mirrornode.hedera.com/` (mainnet: `https://mainnet-public.mirrornode.hedera.com/`) |
| Explorer | `https://hashscan.io/testnet` |
| Faucet / account creation | [portal.hedera.com](https://portal.hedera.com) |
| Fee payer, testnet | `0.0.10576385` — 2,164,663,981 tinybars (≈21.6 HBAR) on 2026-09-17 |
| Fee payer, mainnet | `0.0.10868300` — 2,861,955,686 tinybars (≈28.6 HBAR) on 2026-09-17 |

The end-to-end harness lives in `tests/hedera-e2e/` and drives the official client
against a running facilitator; `live-canary.mjs` is the full
`x402Client` + `x402HTTPClient` handshake, and `associate-testnet.mjs` performs the
HTS association a new account needs before it can hold USDC. See
[its README](../../tests/hedera-e2e/README.md).

Two things that look like our bug and are not:

- **A recipient that has not associated the token.** The transfer cannot succeed on
  chain regardless of the payload. Associate first.
- **A second HTTP request returning the first transaction id.** That is the
  idempotency rule working, not a duplicate payment.

**`hedera:mainnet` is real money and is served now.** Smoke-test on testnet, and price
a mainnet check at the smallest unit the asset allows.

## 8. Running your own facilitator with Hedera

Build with the cargo feature `hedera` (`Cargo.toml`); the production image and CI
already include it. Each network is enabled independently, both switches default to
off, and mainnet additionally requires an explicitly set daily budget. The signing key
and account id are injected per network from dedicated secrets, and the Mirror URL
defaults to the public node for that network
(`HEDERA_MIRROR_URL_TESTNET` / `HEDERA_MIRROR_URL_MAINNET` override it).

A dedicated DynamoDB settlement table is **required**, not optional: verification
fails closed when admission state cannot be read.

The full variable list, the budget semantics, the recovery worker, the readiness probe
and the network egress requirement are in
[Native Hedera payments](../guides/hedera-native.md). They are documented once, there,
so the two pages cannot drift.

---

## Sources

Measured for this page on **2026-09-17**: `/supported`, `/version` and the Hedera entry
above from the production facilitator; the Mirror Node token records for
[USDC testnet `0.0.429274`](https://testnet.mirrornode.hedera.com/api/v1/tokens/0.0.429274)
and [USDC mainnet `0.0.456858`](https://mainnet-public.mirrornode.hedera.com/api/v1/tokens/0.0.456858);
both fee payer account records (`0.0.10576385` testnet, `0.0.10868300` mainnet) and `/health/ready`. Identifiers, refusal reasons, the v2-only rule,
the alias policy, the duration bounds and the settle response shape were read from this
repository at `src/network.rs`, `src/chain/hedera/{mod,codec,config,id}.rs` and
`tests/hedera-e2e/`.

- x402 `exact` scheme for Hedera, at commit `6b930273`:
  [`specs/schemes/exact/scheme_exact_hedera.md`](https://github.com/x402-foundation/x402/blob/6b9302737f16eea7de90b3bf617c045cef23e032/specs/schemes/exact/scheme_exact_hedera.md).
  Merged upstream 2026-02-06.
- Reference implementation: [`@x402/hedera`](https://github.com/x402-foundation/x402/tree/6b9302737f16eea7de90b3bf617c045cef23e032/typescript/packages/mechanisms/hedera).
- This repository's history: Hedera EVM chains 295/296 added in `66d34e6c`
  (2026-04-04) and removed in `278842e5` (2026-05-30).
- Operator guide, recovery and rollout status: [Native Hedera payments](../guides/hedera-native.md);
  [the original integration plan](../plans/hedera-native-x402-integration-plan.md).
- The April 2026 reports in `docs/reports/` (`hedera-integration-analysis.md`,
  `hedera-x402-feasibility-2026-04.md`) read x402 on Hedera through EIP-3009 and treat
  a Hedera-native scheme as something still to be invented — two months after that
  scheme had entered upstream. They are history, not integration guidance.

The other network documented this way is [Arc](arc.md).
