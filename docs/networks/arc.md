# Arc facilitator operations

Scope: direct `exact` USDC payments with EIP-3009 authorizations signed by an
EOA. Both x402 v1 names and v2 CAIP-2 identifiers resolve to distinct networks.
SDK publication is a separate, deferred task.

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
Arc's entry healthy in `/ready`; public canary receipt; balances Lambda reports
native USDC for the same wallet; mainnet alarms and delivery channel present.
Inspect Arc readiness separately from unrelated existing chain degradation.

Rollback: disable only the affected `arc_*_enabled` flag and release it through
the existing targeted CI deployment. Preserve nonce/hash evidence and reconcile
pending transactions first. Disabling a network does not cancel broadcasts.
Never use a full Terraform apply or `-refresh=false` to force activation.

## Evidence and remaining launch gate

- Full facilitator suite: 2,446 tests passed, plus ignored live RPC checks run
  explicitly for both networks. Clippy passed with existing repository warnings.
- Balance monitor tests cover independent switches, chain ID mismatch and one
  native balance. Terraform validation and landing canonical checks passed.
- Isolated testnet canary: [confirmed transfer](https://explorer.testnet.arc.io/tx/0x0f6aa81bdc52669fe4bde349d26b68e6270c563c26ef2639b469c22127fc2e39),
  one atomic USDC unit received, 0.002814575 USDC gas; replay HTTP 400, no second
  debit. Raw public evidence: `docs/reports/2026-09-16-arc-testnet-canary.jsonl`.
- Production configuration enables testnet. Public rollout acceptance must be
  recorded after deployment; the isolated receipt alone is not public acceptance.
- Mainnet code and read-only RPC/domain checks pass. The signer had zero USDC
  at verification; funding, real canary and activation remain required.

No Arc Gateway, EURC, USYC, `upto`, escrow, ERC-8004 writes, EIP-6492 or contract
wallet support is claimed. The universal validator address has no code on
either Arc network. Signatures from the other network fail before broadcast.
