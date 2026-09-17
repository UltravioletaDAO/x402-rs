# Arc (Circle) — getting paid in USDC and EURC on Arc through this facilitator

> **Status on 2026-09-17: live on mainnet and on testnet.** `GET /supported` lists
> both `arc` / `eip155:5042` and `arc-testnet` / `eip155:5042002`, each with `exact`
> and USDC at 6 decimals. Measured against `https://facilitator.ultravioletadao.xyz`
> at `/version` **2.33.0**.
> [Check it yourself](#1-is-it-live) every time: `/supported` is the only list that is
> true today, and this page is not.

This is the integrator's page: what to put in a 402, what the buyer signs, what comes
back, and what does not work. Running a facilitator that serves Arc — activation
switches, funding, canaries, rollback and the acceptance evidence — is
[Arc facilitator operations](arc-operations.md).

Everything here was read from the chain or from this repository's source, and says
when. Where something is documented by Circle but has not been measured here, the page
says so.

---

## EURC: prices in euros

EURC is registered for direct EOA `exact` payments in x402 v1/v2. Circle publishes
different contracts for each network:

| Network | EURC contract | Payment decimals | EIP-712 name / version |
|---|---|---|---|
| Arc mainnet | `0xbEf5f6d51CB62b58e6A8f77868681825C6fe21c1` | 6 | `EURC` / `2` |
| Arc testnet | `0x89B50855Aa3bE2F677cD6303Cec089B5F319D72a` | 6 | `EURC` / `2` |

**0.01 EURC is 10000 atomic units and is a euro price.** No USD/EUR exchange rate
is applied. EURC has its own balance; the facilitator still pays gas in **USDC**.
Select the EURC address explicitly and keep USDC as the default dollar asset.
Do not pass a dollar quote into the EURC signing path.

Contract metadata and EIP-712 domain separators were checked through both live
RPCs on 2026-09-17. Offline signatures and network/token isolation are tested.
**Funded EURC verify/settle acceptance remains pending on both networks**, as
requested by the operator. Existing Arc payment receipts below are **USDC only**;
they do not prove EURC settlement. No EURC payment hashes are claimed.
[Assessment](../reports/2026-09-17-arc-eurc-assessment.json).
[Official Circle contract list](https://developers.circle.com/stablecoins/eurc-contract-addresses).


## At a glance

| | Arc mainnet | Arc testnet |
|---|---|---|
| Network, x402 v1 name | `arc` | `arc-testnet` |
| Network, x402 v2 (CAIP-2) | `eip155:5042` | `eip155:5042002` |
| Chain id | `5042` (`0x13b2`) | `5042002` (`0x4cef52`) |
| Family | EVM | EVM |
| Scheme | `exact` only — an EIP-3009 `transferWithAuthorization` | same |
| Asset | USDC `0x3600000000000000000000000000000000000000`, **6 decimals** | same address, **6 decimals** |
| EIP-712 domain | `name` = `USDC`, `version` = `2` | `name` = `USDC`, `version` = `2` |
| Domain separator | `0x940506…ccdf84` | `0x361191…11c8c6b0` |
| Gas | paid by the facilitator, **in USDC**. The buyer signs and pays nothing else | same |
| Facilitator fee | none | none |
| Wallets | an EOA (a plain key) signs; see [what does not work](#6-what-does-not-work-today) for contract wallets | same |

Both networks carry the same USDC address and the same EIP-712 `name`/`version`. **The
chain id is the only thing that separates the two domains**, so a signature made for
one network cannot verify on the other — which is the intended behaviour, not a bug.

---

## 1. Is it live?

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -c '[.kinds[] | select(.network | startswith("arc")) | {x402Version, scheme, network, networkAliases}]'
```

On 2026-09-17 that prints two entries, one per network:

```json
[{"x402Version":1,"scheme":"exact","network":"arc-testnet","networkAliases":["arc-testnet","eip155:5042002"]},
 {"x402Version":1,"scheme":"exact","network":"arc","networkAliases":["arc","eip155:5042"]}]
```

Each carries `extra.tokens` naming USDC at `0x3600…0000` with `decimals: 6`. An empty
`[]` means the deployment you are talking to does not serve Arc — stop there, because
nothing on this page works against it. A release that contains Arc with the network
switched off refuses it as an unknown network; a release that predates Arc answers
HTTP `400` `Invalid CAIP-2 format: eip155:5042` because the binary does not know the
chain at all.

Readiness for the mainnet signer is separate from `/supported`:

```bash
curl -s 'https://facilitator.ultravioletadao.xyz/health/ready?network=arc' | jq -c '.networks'
```

Measured 2026-09-17: `status: "ok"`, `rpc: "ok"`, the signer's `gasOk: true`.

## 2. The asset, and the trap in its decimals

**On Arc, USDC is the native gas token and an ERC-20 at the same time, over one
balance.** The native balance has **18** decimals. The ERC-20 interface at
`0x3600000000000000000000000000000000000000` shows the same money with **6**,
truncated: `balanceOf = floor(native_balance / 10^12)`.

**Every amount x402 carries on Arc is in the 6-decimal view.** `accepted.amount`
(v2), `maxAmountRequired` (v1) and `authorization.value` are all ERC-20 units:

| Price | Right (6 decimals) | Wrong (18 decimals) |
|---|---|---|
| 0.01 USDC | `"10000"` | `"10000000000000000"` |
| 1 USDC | `"1000000"` | `"1000000000000000000"` |

**An 18 where a 6 belongs asks for 10^12 times the price** — a trillion. At best the
payment fails for funds; at worst a buyer signs an authorization for a trillion
times what they meant to pay. EIP-3009 moves exactly the signed `value`; nothing
downstream rescales it. This is the single most expensive mistake available on Arc,
and it is available on mainnet now, with real money.

The same trap in the other direction, for anyone reading balances:

- `eth_getBalance` (18 decimals) and `balanceOf` (6) are **the same money**. Never add
  them together, and never format one with the other's decimals.
- A remainder smaller than one micro-USDC stays in the native balance and does not
  show through `balanceOf`.
- Measured on one account: `eth_getBalance` = 13,489,266,029,671,387,940 wei,
  `balanceOf` = 13,489,266 — exactly `floor(native / 10^12)`.
- Circle's own gas-and-fees page used `parseUnits("1", 6)` for a *native* transfer when
  it was read on 2026-09-15. Native amounts are 18 decimals; do not copy that line.

## 3. What the seller puts in the 402

Upstream's EVM package (`@x402/evm`, `defaultAssets.ts`, reviewed at commit
`6b930273` on 2026-09-15) has **no entry for Arc**, so a shorthand price such as
`"$0.01"` cannot find USDC there. Name the asset, the amount and the domain
explicitly.

x402 v2 requirements for 0.01 USDC on **mainnet**:

```json
{
  "scheme": "exact",
  "network": "eip155:5042",
  "asset": "0x3600000000000000000000000000000000000000",
  "amount": "10000",
  "payTo": "<the seller's EVM address>",
  "maxTimeoutSeconds": 300,
  "extra": {
    "name": "USDC",
    "version": "2"
  }
}
```

For testnet, the same body with `"network": "eip155:5042002"`.

- In a v2 body `accepted.network` **must** be the CAIP-2 form; `arc` and `arc-testnet`
  are the spellings for v1 bodies. Mixing them is an HTTP `400 invalid_request_body`,
  not a payment failure.
- `maxTimeoutSeconds: 300` is a choice, not an Arc requirement.
- `extra.name` / `extra.version` are for the **buyer's** wallet, which needs them to
  build the digest. The facilitator does not trust them: Arc USDC is in its static
  domain table, and that table wins over whatever `extra` says.

## 4. What the buyer signs

A standard EIP-3009 `TransferWithAuthorization`, over this domain:

| Domain field | Mainnet | Testnet |
|---|---|---|
| `name` | `USDC` | `USDC` |
| `version` | `2` | `2` |
| `chainId` | `5042` | `5042002` |
| `verifyingContract` | `0x3600000000000000000000000000000000000000` | same |

Those hash to the two separators abbreviated in the table at the top. The full
32-byte values are written once each, in `ARC_USDC_DOMAIN_SEPARATOR` in
`src/chain/evm.rs` and in [Arc facilitator operations](arc-operations.md) — compare
against those, or re-read `DOMAIN_SEPARATOR()` yourself. Both were read from the
contract's `DOMAIN_SEPARATOR()` **and** recomputed locally from the four fields above;
the two match. Re-measured 2026-09-17 at mainnet block 21,259,527 and testnet block
62,505,464, and a test fails if the contract stops publishing them.

If your wallet produces a different separator, the signature will not verify — the
usual cause is a wrong chain id, and with two networks on the same USDC address that
is now the easy mistake to make.

The contract is a proxy, so these values can change under the same address. If you pin
them too, re-read them when something stops verifying.

The `/verify` body in the v2 shape, with `payload` abbreviated. A runnable body with
well-formed placeholders, written for Base, is the v2 example in
[`/skill.md`](https://facilitator.ultravioletadao.xyz/skill.md) §3; the Arc body is
that one with the `accepted` below and `authorization.value` set to the same amount.

```json
{
  "x402Version": 2,
  "paymentPayload": {
    "x402Version": 2,
    "payload": {
      "signature": "0x…",
      "authorization": {
        "from": "<buyer>",
        "to": "<seller, the same address as payTo>",
        "value": "10000",
        "validAfter": "<unix seconds, as a string>",
        "validBefore": "<unix seconds, as a string>",
        "nonce": "0x… (32 bytes)"
      }
    }
  },
  "resource": {
    "url": "https://example.com/protected",
    "description": "One API call",
    "mimeType": "application/json"
  },
  "accepted": {
    "scheme": "exact",
    "network": "eip155:5042",
    "amount": "10000",
    "payTo": "<seller, the same address as authorization.to>",
    "maxTimeoutSeconds": 300,
    "asset": "0x3600000000000000000000000000000000000000",
    "extra": { "name": "USDC", "version": "2" }
  }
}
```

`authorization.value` and `accepted.amount` are the same 6-decimal number. The general
rules for that envelope — which fields are strings, what moved between v1 and v2 —
are in the same §3.

## 5. What the facilitator does, and what comes back

1. `POST /verify` checks the signature against the domain above, the amount, the
   recipient, the time window and the payer's balance. It does not send anything.
2. `POST /settle` submits `transferWithAuthorization` from the facilitator's own
   account, with `value = 0` in the transaction: the payment leaves
   `authorization.from`, and the facilitator pays the gas in USDC. No approval, no
   deposit, no bridge.
3. A settle answers with the transaction hash once there is a receipt. Arc is final
   on inclusion, but a submitted transaction is not yet a receipt. If the receipt does
   not arrive in time the answer is HTTP `502` with
   `{"error": "settlement_unconfirmed", "transaction": "0x…", "paymentId": "…", "retryable": false}`
   and deliberately no `Retry-After` (`src/handlers.rs`): the transaction may well be
   mined. Look the hash up before doing anything else, and **do not ask the buyer to
   sign again** — a fresh authorization for a payment that did land is a second debit.

For anyone verifying a settlement from its receipt: Circle documents that a native
USDC movement can also log a `Transfer` from the system emitter
`0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE`, in **18** decimals, next to the token
contract's own `Transfer` in 6 — same topic, same `from` and `to`, same payment.
Identify the event by **the address that emitted it**, not by its topic, or 0.01 USDC
reads as ten billion. The facilitator's receipt reader filters by emitter, and a
fixture carrying both events holds it there.

## 6. What does not work today

Said plainly, so nobody builds on it. Every row re-measured on 2026-09-17, on **both**
Arc networks unless stated.

| | Status | Why |
|---|---|---|
| **EURC** | registered, live payment acceptance pending | Separate mainnet/testnet contracts, six decimals, `EURC` / `2`. See the EURC section above; euro quotes require explicit token units. |
| **EIP-6492** (a counterfactual smart wallet, not deployed yet) | refused | The universal signature validator the facilitator calls, `0xdAcD51A54883eb67D95FAEb2BBfdC4a9a6BD2a3B`, has **no code on either Arc network** (0 bytes, mainnet and testnet). `/verify` answers `isValid: false` with `invalid_signature`, and `/settle` sends nothing. That token is the same one a bad signature gets; the explanation is only in the server log |
| **Already-deployed EIP-1271 wallets** | not proven | The code path does not use the missing validator, but no positive payment from a contract wallet has been measured on Arc. Treat it as unsupported until one is |
| **Circle Gateway / Nanopayments authorizations** | refused | Gateway also advertises `exact` on Arc, but its buyers sign against a different domain (`GatewayWalletBatched`, version `1`) and it settles in batches. Those signatures are not USDC transfer authorizations and do not verify here; they belong to Circle Gateway. For this facilitator, sign the USDC domain above |
| **`upto`, `escrow` / `commerce`, ERC-8004** | not on Arc | `/supported` lists `exact` and only `exact` for both Arc networks, and neither `UPTO_DEPLOYED_NETWORKS` (`src/upto/types.rs`) nor `supported_networks()` (`src/erc8004/mod.rs`) names Arc. A network being served for `exact` does not enable anything else |
| **USYC** | not accepted | Listed by Circle on Arc, not registered here, and not a plain stablecoin: its authorization, units and eligibility have not been analysed |

**EIP-6492 has a path; it has not been walked.** The deterministic CREATE2 factory
(`0x4e59b44847b379578588920cA78FbF26c0B4956C`) **is** deployed on both Arc networks
(69 bytes each, 2026-09-17), which is what would let the validator be placed at its
usual address. That is a separate change, and it will be announced when a real
EIP-6492 payment has settled — not when the contract is merely deployed.

## 7. Testing on Arc testnet

| | |
|---|---|
| Public RPC | `https://rpc.testnet.arc.io` (mainnet: `https://rpc.mainnet.arc.io`) |
| Explorer | `https://explorer.testnet.arc.io` (mainnet: `https://explorer.arc.io`) |
| Faucet (USDC for gas **and** for paying) | `https://faucet.circle.com` |
| Fees | EIP-1559. Circle documents a minimum `maxFeePerGas` of 20 gwei; `eth_gasPrice` answered 25 gwei on 2026-09-17 |

`https://testnet.arcscan.app` still answers, but with an HTTP `301` to
`https://explorer.testnet.arc.io/` — use the canonical host.

Two things that look like our bug and are not:

- **Arc blocks `0x70997970C51812dc3A010C7d01b50e0d17dc79C8` from genesis** — account #1
  of the public Foundry/Anvil test mnemonic. A USDC transfer to it reverts with
  `execution reverted: Blocked address`. Do not test with the default Anvil accounts.
- A generic Anvil fork does not reproduce Arc's native-USDC rules. Test against the
  testnet, or Circle's Arc Foundry tooling.

Testnet USDC comes from Circle's faucet and is free. **Mainnet Arc is real money and
the facilitator is live on it** — price a smoke test at one atomic unit (`"1"`), the
way the canary does.

## 8. Running your own facilitator with Arc

Each Arc network is served only where its own RPC variable is set, and each has its own
deployment switch:

```bash
RPC_URL_ARC=https://rpc.mainnet.arc.io
RPC_URL_ARC_TESTNET=https://rpc.testnet.arc.io
```

The signers are the ordinary EVM ones — `EVM_PRIVATE_KEY_MAINNET` for `arc`,
`EVM_PRIVATE_KEY_TESTNET` for `arc-testnet`. A signer pays Arc gas **in USDC**; there
is no ETH on Arc, so it needs USDC on that exact network before it can settle
anything. Balances on Ethereum, Base or Arc testnet do not fund Arc mainnet.

Nothing checks that the URL you configure is really the Arc network you named it. The
EIP-712 domain is built from the facilitator's table with chain id 5042 or 5042002,
while the transaction goes to whatever chain the RPC answers for. Point either one at
the wrong chain and every settle fails there, and the failure looks like a problem with
the payment rather than with the configuration. Check with `eth_chainId` — expect
`0x13b2` for mainnet and `0x4cef52` for testnet — before you switch it on.

Activation switches, funding thresholds, the canary script, the release checklist and
rollback are in [Arc facilitator operations](arc-operations.md).

---

## Sources

Measured for this page on **2026-09-17** against `https://rpc.mainnet.arc.io` (block
21,259,527) and `https://rpc.testnet.arc.io` (block 62,505,464): `eth_chainId` on both;
USDC `decimals()`, `name()`, `version()` and `DOMAIN_SEPARATOR()` on both, each
recomputed locally and matching; `eth_getCode` on the EIP-6492 validator (0 bytes on
both) and the CREATE2 factory (69 bytes on both). The original EURC check used
the testnet address on mainnet; the official mainnet address differs. The EURC
assessment linked above corrects this and verifies 1,798 bytes on each network; `eth_gasPrice` on testnet. `/supported`, `/version` and
`/health/ready?network=arc` were read from the production facilitator the same day.
The explorer hosts were probed for their HTTP status. The blocked genesis address and
the two-precision balance were measured on 2026-09-16 at testnet block 62,335,077.

Circle's documentation, not re-measured here: the 18 native decimals, the 20 gwei
minimum, finality on inclusion, and the list of contract addresses —
[connect to Arc](https://docs.arc.io/arc/references/connect-to-arc.md),
[contract addresses](https://docs.arc.io/arc/references/contract-addresses.md),
[stablecoin native model](https://docs.arc.io/arc/concepts/stablecoin-native-model.md),
[gas and fees](https://docs.arc.io/arc/references/gas-and-fees.md),
[USDC system events](https://docs.arc.io/arc/references/usdc-system-events.md),
[EIP-3009 relayer](https://docs.arc.io/integrate/relayers-and-paymasters/eip-3009-relayer.md),
[Nanopayments and x402](https://developers.circle.com/gateway/nanopayments/concepts/x402).

x402 reference: [`@x402/evm` at `6b930273`](https://github.com/x402-foundation/x402/tree/6b9302737f16eea7de90b3bf617c045cef23e032/typescript/packages/mechanisms/evm).

Operations, activation and the acceptance evidence for both networks:
[Arc facilitator operations](arc-operations.md). The other network documented this
way is [Hedera](hedera.md).

## EURC production and package verification (2026-09-17)

Facilitator **2.34.0** publishes USDC/EURC for both Arc networks and x402 v1/v2.
[Deployment](https://github.com/UltravioletaDAO/x402-rs/actions/runs/35182691614).
[Public discovery, OpenAPI, agent documents and OG verification](../reports/2026-09-17-arc-eurc-public-web.json).
The landing also uses the supplied Arc/Hedera logos and puts both native Hedera
accounts in the wallet section; the supplied image bytes match production.

Python **0.86.0** and TypeScript **2.94.0** were published, installed cleanly and
checked through eight offline EURC signatures (two networks × two versions ×
two SDKs). [Package integrity](../reports/2026-09-17-arc-eurc-published-integrity.json).
Python: 1,222 tests; TypeScript: 762 tests plus typecheck/lint/build; 430 existing
cross-language checks passed.

Read-only RPC simulations reach the EURC transfer entrypoint and reject unfunded
signers. The public facilitator likewise rejects eight unfunded authorizations
with the expected balance error using the published Python and TypeScript packages.
[Python verification](../reports/2026-09-17-arc-eurc-public-verification.json),
[TypeScript verification](../reports/2026-09-17-arc-eurc-typescript-public-verification.json).
**No EURC settlements were executed. Funded payment acceptance remains pending
on both networks by operator instruction.** The existing USDC receipts retain
their original scope.
