# Algorand payment groups built by the known clients

Each file is the `paymentGroup` one published client builds for an `exact`
USDC payment on Algorand testnet, with the facilitator paying the fees:
index 0 the unsigned fee transaction, index 1 the payer's signed transfer.
`replay_verify_tests::the_groups_the_known_clients_build_verify_and_settle`
(`src/chain/algorand.rs`) holds that `/verify` and `/settle` accept them.

| File | Client | How it was built |
|---|---|---|
| `uvd-x402-sdk-typescript.json` | `uvd-x402-sdk` 2.116.0 (npm), `algosdk` 3.9.0 | `AlgorandProvider.signPayment` on its Pera path, the wallet signing with `txn.signTxn` |
| `uvd-x402-sdk-python.json` | `uvd-x402-sdk` 0.114.0 (PyPI), `py-algorand-sdk` 2.12.0 | `networks.algorand.build_atomic_group` with fixed `SuggestedParams` |
| `x402-avm.json` | `@x402/avm` 2.31.0 (npm), `algokit-utils` 10.0.0-alpha.46 | `ExactAvmScheme.createPaymentPayload` with `toClientAvmSigner` and `extra.feePayer` |

The clients ran their own code against an algod on 127.0.0.1 that served
only `/v2/transactions/params` (testnet genesis, last round 995), so the
groups are valid from round 995 and the tests' algod stub answers round 1000.
The facilitator, payer and `payTo` accounts were generated for the run and
their keys discarded: the files carry addresses and signed transactions,
never a key. The tests verify with a provider that answers for the file's
`facilitator` address, and wrap each group in a v1 request for
`algorand-testnet`: what they check is the group, not how a client names the
network.

What the clients put on the fee transaction: a zero-amount payment from the
facilitator to itself, fee 2_000, no lease. `@x402/avm` adds the note
`x402-fee-payer-` followed by `Date.now()`; the uvd-x402-sdk clients add none.
Other public x402 clients write `x402-fee-payer` alone, which the facilitator
accepts too.

The published Python `build_atomic_group` raises `TypeError` before it
returns: it base64-encodes the string `algosdk.encoding.msgpack_encode`
already returns in base64. Its file was built with that one call passing the
string through, which leaves the transactions it builds as they are; the
`client` field in the file says so.
