# Arc facilitator operations

This page is for whoever **runs** a facilitator on Arc: activation switches, funding,
the canary, the release checklist, rollback and the acceptance evidence. Whoever is
**integrating** — building a 402, signing an authorization, reading a settlement —
wants [Arc: getting paid in USDC and EURC on Arc](arc.md) instead.

Scope: direct `exact` USDC payments with EIP-3009 authorizations signed by an
EOA. Both x402 v1 names and v2 CAIP-2 identifiers resolve to distinct networks.
Python `uvd-x402-sdk` **0.84.0** and TypeScript `uvd-x402-sdk` **2.92.0**
include both Arc networks. See the [Python Arc guide](https://github.com/UltravioletaDAO/uvd-x402-sdk-python/blob/main/docs/networks/arc.md)
and [TypeScript Arc guide](https://github.com/UltravioletaDAO/uvd-x402-sdk-typescript/blob/main/docs/networks/arc.md).

Current releases **Python 0.85.0** and **TypeScript 2.93.0** retain these Arc
capabilities and add [native Hedera payments](hedera.md). The Arc receipts below
identify the exact earlier versions used for that acceptance run.

| Parameter | Mainnet | Testnet |
|---|---|---|
| v1 network | `arc` | `arc-testnet` |
| Chain ID / v2 network | 5042 / `eip155:5042` | 5042002 / `eip155:5042002` |
| RPC | `https://rpc.mainnet.arc.io` | `https://rpc.testnet.arc.io` |
| Explorer | `https://explorer.arc.io` | `https://explorer.testnet.arc.io` |
| RPC variable | `RPC_URL_ARC` | `RPC_URL_ARC_TESTNET` |
| Activation variable | `arc_mainnet_enabled` | `arc_testnet_enabled` |
| Signer | `EVM_PRIVATE_KEY_MAINNET` | `EVM_PRIVATE_KEY_TESTNET` |
| Facilitator wallet | `0x103040545AC5031A11E8C03dd11324C7333a13C7` | `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8` |

USDC is `0x3600000000000000000000000000000000000000` on both networks.
EIP-712 name/version are `USDC` / `2`. ERC-20 amounts use **6 decimals**;
native gas amounts use **18 decimals**. They are views of **one balance**.
Never sum `eth_getBalance` and `balanceOf` as separate funds.

Verified against the official RPCs on 2026-09-16, at mainnet block 21,183,711
and testnet block 62,428,890. The domain separators, also recomputed locally:

```text
mainnet 0x940506929bba468048a19b567f4f0d534714bc06604b5c3017e5d16785ccdf84
testnet 0x361191522483d32a83e70ae7183b4b9629442c13a78bc9921d6f707911c8c6b0
```

Sources: [RPC endpoints](https://docs.arc.io/arc/references/rpc-endpoints),
[contract addresses](https://docs.arc.io/arc/references/contract-addresses),
[connection parameters](https://docs.arc.io/arc/references/connect-to-arc),
[gas and fees](https://docs.arc.io/arc/references/gas-and-fees).
Mainnet parameters were published after the original testnet implementation;
the historical 2.30.0 changelog's mainnet availability note is superseded.

## Activation and funding

Both switches default to false. Production values live in
`terraform/environments/production/production.auto.tfvars`; the same switch
supplies the ECS and balances Lambda RPC. Enabling mainnet also installs RPC
and low-reserve alarms. The initial reserve threshold is 0.1 native USDC.
The landing page shows an Arc wallet only when `/supported` advertises it.

Fund the wallet **on the selected Arc network**, initially with 1 USDC for
mainnet (testnet funding comes from [Circle's faucet](https://faucet.circle.com)).
Amounts on Ethereum, Base or Arc testnet do not fund Arc mainnet. The canary
requires 0.1 USDC and refuses a gas quote above 50 gwei. Revisit the reserve
against actual traffic; it is not a capacity guarantee.

First run the read-only preflight, then an isolated candidate with the intended
RPC and signer, then enable that network through the normal CI release:

```bash
python scripts/arc_canary.py --network arc-testnet
python scripts/arc_canary.py --network arc
# Explicitly spends 0.000001 USDC plus gas, between our two EVM wallets:
python scripts/arc_canary.py --network arc-testnet --facilitator http://127.0.0.1:18402 --execute
# After rollout, test the PUBLIC route (same check for --network arc):
python scripts/arc_canary.py --network arc-testnet --execute
# The v2 envelope and CAIP-2 identifier, also without a proprietary SDK:
python scripts/arc_canary.py --network arc-testnet --x402-version 2 --execute
```

Requires `eth-account`, `eth-utils`, and `boto3` for execution. Execution reads
the matching `facilitator-evm-{mainnet,testnet}-private-key` AWS secret in
`us-east-2` into memory and asserts its address. Do not put keys in CLI arguments
or committed files. An isolated server needs the same blacklist provisioning
as Docker plus OFAC enabled; CI Docker builds default the local blacklist to
`[]` when absent. Keep local candidates out of the production writer election.

The canary validates chain identity, fresh block, USDC domain and decimals,
`/supported`, `/verify`, `/settle`, a successful receipt, the exact token
Transfer log and recipient delta, then replay without another debit. It logs
the authorization nonce before sending and the resulting transaction hash.
If settle is uncertain, reconcile that nonce/hash on the selected chain;
**do not create another authorization to retry an uncertain payment**.

Release checks: correct `/version`; expected `exact` network in `/supported`;
Arc's entry healthy in `/health/ready`; public canary receipt; balances Lambda reports
native USDC for the same wallet; mainnet alarms and delivery channel present.
Inspect Arc readiness separately from unrelated existing chain degradation.

Rollback: disable only the affected `arc_*_enabled` flag and release it through
the existing targeted CI deployment. Preserve nonce/hash evidence and reconcile
pending transactions first. Disabling a network does not cancel broadcasts.
Never use a full Terraform apply or `-refresh=false` to force activation.

## Validation evidence

- Full facilitator suite: 2,446 tests passed, plus ignored live RPC checks run
  explicitly for both networks. Clippy passed with existing repository warnings.
- Balance monitor tests cover independent switches, chain ID mismatch and one
  native balance. Terraform validation and landing canonical checks passed.
- Isolated testnet canary: [confirmed transfer](https://explorer.testnet.arc.io/tx/0x0f6aa81bdc52669fe4bde349d26b68e6270c563c26ef2639b469c22127fc2e39),
  one atomic USDC unit received, 0.002814575 USDC gas; replay HTTP 400, no second
  debit. Raw public evidence: `docs/reports/2026-09-16-arc-testnet-canary.jsonl`.
- Isolated testnet v2/CAIP-2 canary also passed: [confirmed transfer](https://explorer.testnet.arc.io/tx/0x139489c10866a42139f82dd44f91fb25cca23715d4023d0b10164d77723d67f5),
  gas 0.002189775 USDC; replay HTTP 400 without another debit.
- Mainnet signer funding confirmed by [receipt](https://explorer.arc.io/tx/0x4479f7ea0212c35b94389aca1f2cd790d309c22710098b4ed3528a31e6a4e31b).
- Isolated mainnet v1 canary passed: [receipt](https://explorer.arc.io/tx/0x2246ad72a2a00a5ea19f54d32effff32ee8cea7a58c5e0cf0740244216dc92f4),
  gas 0.002252606134343883 USDC; v2/CAIP-2 also passed: [receipt](https://explorer.arc.io/tx/0x5b66c97e80ca7773ba919a0052b79d4ec3db193419a1c355f33d0f5e4c62636d),
  gas 0.001804750735987521 USDC. Each delivered one atomic USDC unit and
  rejected replay without a second debit. Evidence is in `docs/reports/*candidate-canary.jsonl`.
- Production rollout and public acceptance completed **2026-09-16 17:24 UTC**:
  version **2.31.0**, image `2.31.0-5a7acfc`, ECS task revision 434, two healthy
  tasks. Both Arc networks advertise `exact` under v1 names and v2 CAIP-2 IDs;
  readiness is `ok`, balances are present and both mainnet alarms are `OK`.
- Four payments through the **public facilitator** passed (one atomic USDC each),
  including receipt, Transfer emitter, recipient balance and replay checks:

  | Network | v1 receipt | v2 receipt |
  |---|---|---|
  | Mainnet | [Confirmed](https://explorer.arc.io/tx/0xe661af1a632b7f4fc4d536fb6234c3b2563860068b8a68755aad82467e075488) | [Confirmed](https://explorer.arc.io/tx/0x15f7519fc68d676ca420456fd44c13233bca33a2a51fca2988b862208f482787) |
  | Testnet | [Confirmed](https://explorer.testnet.arc.io/tx/0x243f3ebb20f5afac0913f11890378eceb52af008c56c2b7c4b38dc2758359c8a) | [Confirmed](https://explorer.testnet.arc.io/tx/0xb57895281e25a15468d0b77a51850f3817a24db91261452fae521796a385be66) |

  [Production acceptance record](../reports/2026-09-16-arc-production-acceptance.json)
  includes fees, replay outcomes and the deployed configuration.
  [Release CI](https://github.com/UltravioletaDAO/x402-rs/actions/runs/35126105947)
  passed all jobs. Existing Ethereum/Polygon Amoy readiness issues are separate
  from this Arc acceptance and remain recorded in the evidence.

No Arc Gateway, EURC, USYC, `upto`, escrow, ERC-8004 writes, EIP-6492 or contract
wallet support is claimed. The universal validator address has no code on
either Arc network. Signatures from the other network fail before broadcast.

## SDK acceptance (2026-09-16)

The Python and TypeScript SDKs each completed four additional controlled payments
through the public facilitator: mainnet/testnet, x402 v1/v2. Every receipt succeeded,
credited exactly one atomic USDC unit, and replay produced no second credit.
These payments exercised the SDK signing, header encoding, verify and settle paths.
TypeScript used an injected EIP-1193 signer in Node, not a browser-wallet UI.

- [Python 0.84.0 receipt evidence](https://github.com/UltravioletaDAO/uvd-x402-sdk-python/blob/v0.84.0/docs/reports/2026-09-16-arc-sdk-acceptance.json)
- [TypeScript 2.92.0 receipt evidence](https://github.com/UltravioletaDAO/uvd-x402-sdk-typescript/blob/v2.92.0/docs/reports/2026-09-16-arc-sdk-acceptance.json)

Install with `pip install "uvd-x402-sdk[signer]>=0.84.0"` or
`npm install uvd-x402-sdk@^2.92.0`. Network names are `arc` and `arc-testnet`;
the v2 identifiers remain `eip155:5042` and `eip155:5042002`.

## EURC activation and acceptance

EURC is registered alongside USDC in 2.34.0 for both Arc networks. No new RPC or
key is required; gas remains USDC. See [contracts and euro pricing](arc.md#eurc-prices-in-euros).
Funded EURC acceptance is pending by operator decision. USDC receipts above are
unchanged and are not EURC proof. Python 0.86.0 and TypeScript 2.94.0 add EURC.
