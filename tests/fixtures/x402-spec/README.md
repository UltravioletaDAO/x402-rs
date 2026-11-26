# x402 Foundation spec examples

The JSON examples of the x402 Foundation's `exact` scheme specs, copied from
[`x402-foundation/x402`](https://github.com/x402-foundation/x402) at commit
`4f3c7851` (2026-11-26). The tests that read them hold this facilitator to the
request and response shapes those specs define.

| File | Source |
|------|--------|
| `stellar_exact_payment_requirements.json` | `specs/schemes/exact/scheme_exact_stellar.md`, "`PaymentRequirements` for `exact`" |
| `stellar_exact_payment_payload.json` | same file, "Full `PaymentPayload` object" |
| `stellar_exact_settlement_response.json` | same file, "Phase 3: `SettlementResponse`" |
| `sui_exact_payment_payload.json` | `specs/schemes/exact/scheme_exact_sui.md`, "Full `PaymentPayload` object" |

Copied verbatim, but for one forced edit: the Sui `accepted.payTo` is a
32-byte address, `0x` + 64 hex, the shape `.githooks/pre-commit` refuses on
any added line. It is cut to `0x1eb7c57e...667fe0b1`; the tests write it out
again from the transaction of the same example, whose sender it is.

The examples illustrate shapes; they are not payments a facilitator could
settle, and the tests say so where they read them:

- **Stellar `payload.transaction`** is truncated in the spec itself
  (`AAAAAgAAAABriIN4...AAAAAA...`): it decodes to nothing. **Stellar `payTo`**
  (`GBHEGW3K...W3KO`, in the requirements and in `accepted`) fails the strkey
  checksum: it is no account, and no transfer can name it. The tests keep the
  example's every other field (network, asset, amount, timeout, `extra`) and
  put in its place a transaction they build and sign with a test key, to the
  spec's rules, paying a valid account.
- **Sui**: the transaction is real BCS `TransactionData`, but not a payment of
  the example's terms. Its sender is the address named `payTo`, pays its own
  gas, and sends 2.5 SUI (`2500000000` MIST, after a `MergeCoins` of six coins)
  to another address; `amount` says `10000000`. Its `signature` is 64 raw
  bytes, without the scheme flag and public key of a Sui signature. The asset,
  `0x2::sui::SUI`, is not one this facilitator settles. The tests read the
  example as the spec's request and expect the refusal it gets; the Sui
  payments they verify and settle are transactions built here.
