# EIP-6492 deployment sponsorship

What the facilitator does with a payment signed by a smart wallet that is not
deployed yet (an [EIP-6492](https://eips.ethereum.org/EIPS/eip-6492) envelope:
the wallet's factory, the factory calldata and the inner signature, followed by
the `0x6492...6492` magic suffix), on every EVM network it serves, HyperEVM
(chain 999) included.

## Today: counterfactual wallets are not accepted

The facilitator does not accept EIP-6492 signatures, and it deploys no wallet on
any network. The payer's wallet has to be deployed before it pays, or the payer
pays from an EOA.

Every EVM `exact` signature must be exactly 65 bytes. An EIP-6492 envelope never
is, so `POST /verify` answers `"isValid": false` with `"invalidReason":
"invalid_signature"` and `POST /settle` sends no transaction: nothing is
deployed and nobody pays any gas. The same rule applies to a deployed contract
wallet's ERC-1271 signature: it passes only when it is 65 bytes, and the wrapped
or passkey signatures most smart wallets produce are longer, so they are refused
the same way today. On Arc and HashKey Chain an EIP-6492 envelope is refused even
earlier: the universal signature validator is not deployed there.

## If sponsorship is ever enabled: the policy

Sponsoring deployments is not enabled. What follows is the policy a deployment
would answer to. It already runs, ahead of the 65-byte rule, in `log` mode, so it
records which factories payers send; it changes no answer. It is published, as
the values in force, by `GET /config` under `eip6492Sponsorship`.

- **Factories.** Only a deployment through a factory on the network's list would
  be sponsored. The default list is the same on every EVM network:

  | Factory | Address | Source |
  |---|---|---|
  | Coinbase Smart Wallet v1 | `0x0BA5ED0c6AA8c49038F819E587E2633c4A9F428a` | [coinbase/smart-wallet](https://github.com/coinbase/smart-wallet), README |
  | Coinbase Smart Wallet v1.1 | `0xBA5ED110eFDBa3D005bfC882d75358ACBbB85842` | [coinbase/smart-wallet](https://github.com/coinbase/smart-wallet), README |
  | Safe ProxyFactory 1.4.1 | `0x4e1DCf7AD4e460CfD30791CCC4F9c8a4f820ec67` | [safe-deployments](https://github.com/safe-global/safe-deployments) `v1.4.1/safe_proxy_factory.json` |
  | Safe ProxyFactory 1.3.0 | `0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2` | [safe-deployments](https://github.com/safe-global/safe-deployments) `v1.3.0/proxy_factory.json` |
  | Safe ProxyFactory 1.3.0 (eip155) | `0xC22834581EbC8527d974F8a1c97E1bEA4EF910BC` | [safe-deployments](https://github.com/safe-global/safe-deployments) `v1.3.0/proxy_factory.json` |

- **Gas.** The deploy-and-transfer transaction is held to its own gas ceiling
  (`exact-deploy` in `GET /config`, `evmGasCeilings`; repeated per network under
  `eip6492Sponsorship`). An estimate above it is refused before anything is
  signed or sent.
- **Mode.** `log` (in force) records an envelope whose factory is not listed and
  changes nothing else; `enforce` refuses it with `"invalidReason":
  "eip6492_factory_not_allowed"`, before anything is sent.

## Who would pay if it failed

Nobody. `/verify` simulates the deployment and the transfer together in one
`eth_call`, and `/settle` estimates the transaction before it is signed, so a
deployment or a transfer that would fail -- a payer without the token balance, a
factory call that does not produce the wallet, a signature the wallet rejects --
is refused before any transaction exists. The facilitator would pay the gas of a
valid payment only, and the payer would need the token balance in the wallet's
address before the payment: the deployment does not fund it.

## Operating it

| Variable | Default | Effect |
|---|---|---|
| `EIP6492_SPONSOR_MODE` | `log` | `log` or `enforce`; anything else is `log` plus a warning |
| `EIP6492_FACTORY_ALLOWLIST` | the table above | comma-separated addresses, replacing the list on every network |
| `EIP6492_FACTORY_ALLOWLIST_<NETWORK>` | unset | the same for one network (v1 name, upper-case, `-` as `_`); an empty value lists none |
| `EVM_MAX_GAS_EXACT_DEPLOY_<NETWORK>` | built in (`chain::gas_ceiling`) | the deployment's gas ceiling |

A value with an entry that is not an address is ignored whole, with a warning: a
typo never stops the process and never passes for a list. In `log` mode every
envelope with an unlisted factory is logged at WARN (`eip6492: the factory of
this envelope is not on the sponsorship list`), with its network and factory.
None of these variables enables sponsorship: the 65-byte rule decides that.

Code: `src/chain/eip6492_sponsor.rs` (policy), `src/chain/evm.rs`
(`assert_valid_payment`, where it runs).
