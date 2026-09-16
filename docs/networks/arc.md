# Arc (Circle) — getting paid in USDC on Arc through this facilitator

> **Status on 2026-09-16: in the code, not live.** Arc testnet is being added to the
> facilitator **switched off**: the production deployment does not serve it, and
> `GET /supported` does not list it. Nothing below works against
> `https://facilitator.ultravioletadao.xyz` until `/supported` lists it.
> [Check first](#1-is-it-live), every time: `/supported` is the only list that is true
> today, and this page is not.
>
> **Arc mainnet does not exist here and has no date.** Circle publishes testnet
> contract addresses only, so there is nothing to integrate against.

Everything on this page was read from the chain or from the facilitator's source,
and says when. Where something is documented by Circle but has not been measured
here, the page says so.

---

## At a glance

| | Arc testnet |
|---|---|
| Network, x402 v2 (CAIP-2) | `eip155:5042002` |
| Network, x402 v1 name | `arc-testnet` — there is **no** `arc` alias: that name belongs to a mainnet that does not exist yet |
| Chain id | `5042002` (`0x4cef52`) |
| Family | EVM |
| Scheme | `exact` only — an EIP-3009 `transferWithAuthorization` |
| Asset | USDC `0x3600000000000000000000000000000000000000`, **6 decimals** |
| EIP-712 domain | `name` = `USDC`, `version` = `2` |
| Gas | paid by the facilitator, **in USDC**. The buyer signs and pays nothing else |
| Facilitator fee | none |
| Wallets | an EOA (a plain key) signs; see [what does not work](#6-what-does-not-work-today) for contract wallets |

---

## 1. Is it live?

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -c '[.kinds[] | select(.network == "arc-testnet" or .network == "eip155:5042002") | {x402Version, scheme, network}]'
```

- `[]` — **not served.** Stop here. On 2026-09-16 it printed `[]`.
- Two entries, `exact` under `arc-testnet` (v1) and under `eip155:5042002` (v2), each
  with `extra.tokens` naming USDC at `0x3600…0000` with `decimals: 6` — served.
  That is the shape `robinhood-testnet` and the other EVM testnets that settle
  `exact` have in `/supported` today.

What a payment on Arc gets back while it is not served depends on the deployment:

| Deployment | `POST /verify` with `"network": "eip155:5042002"` |
|---|---|
| **Production today** (`/version` = `2.29.6`, measured 2026-09-16) | HTTP `400`, `Invalid CAIP-2 format: eip155:5042002` — the running binary does not know the chain at all |
| A release that contains Arc, with Arc switched off | refused as `invalid_network` — read from the source (the provider map has no Arc), not measured |
| Arc served | answered on the merits, like any other network |

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
downstream rescales it.

The same trap in the other direction, for anyone reading balances:

- `eth_getBalance` (18 decimals) and `balanceOf` (6) are **the same money**. Never add
  them together, and never format one with the other's decimals.
- A remainder smaller than one micro-USDC stays in the native balance and does not
  show through `balanceOf`.
- Measured on 2026-09-16 on one account: `eth_getBalance` =
  13,489,266,029,671,387,940 wei, `balanceOf` = 13,489,266 — exactly
  `floor(native / 10^12)`.
- Circle's own gas-and-fees page used `parseUnits("1", 6)` for a *native* transfer when
  it was read on 2026-09-15. Native amounts are 18 decimals; do not copy that line.

## 3. What the seller puts in the 402

Upstream's EVM package (`@x402/evm`, `defaultAssets.ts`, reviewed at commit
`6b930273` on 2026-09-15) has **no entry for Arc**, so a shorthand price such as
`"$0.01"` cannot find USDC there. Name the asset, the amount and the domain
explicitly.

x402 v2 requirements for 0.01 USDC:

```json
{
  "scheme": "exact",
  "network": "eip155:5042002",
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

- In a v2 body `accepted.network` **must** be the CAIP-2 form; `arc-testnet` is the
  spelling for v1 bodies. Measured on 2026-09-16 on a network that is served, with
  the body shape below: `"eip155:84532"` answered on the merits, `"base-sepolia"`
  answered HTTP `400 invalid_request_body`.
- `maxTimeoutSeconds: 300` is a choice, not an Arc requirement.
- `extra.name` / `extra.version` are for the **buyer's** wallet, which needs them to
  build the digest. The facilitator does not trust them: Arc USDC is in its static
  domain table, and that table wins over whatever `extra` says.

## 4. What the buyer signs

A standard EIP-3009 `TransferWithAuthorization`, over this domain:

| Domain field | Value |
|---|---|
| `name` | `USDC` |
| `version` | `2` |
| `chainId` | `5042002` |
| `verifyingContract` | `0x3600000000000000000000000000000000000000` |

That domain hashes to `0x36119152…11c8c6b0`; the full value is written once in the
facilitator's source, as `ARC_USDC_DOMAIN_SEPARATOR` in `src/chain/evm.rs`. It was
read from the contract's `DOMAIN_SEPARATOR()` **and** recomputed locally from the four
fields above; the two match (2026-09-16, block 62,400,328). If your wallet produces a
different separator, the signature will not verify — the usual cause is a wrong
chain id.

The contract is a proxy, so these values can change under the same address. The
facilitator pins them in a test; if you pin them too, re-read them when something
stops verifying.

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
    "network": "eip155:5042002",
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
   and no `Retry-After`: the transaction may well be mined. Look the hash up before
   doing anything else, and **do not ask the buyer to sign again** — a fresh
   authorization for a payment that did land is a second debit.

For anyone verifying a settlement from its receipt: Circle documents that a native
USDC movement can also log a `Transfer` from the system emitter
`0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE`, in **18** decimals, next to the token
contract's own `Transfer` in 6 — same topic, same `from` and `to`, same payment.
Identify the event by **the address that emitted it**, not by its topic, or 0.01 USDC
reads as ten billion. The facilitator's receipt reader filters by emitter, and a
fixture carrying both events holds it there.

## 6. What does not work today

Said plainly, so nobody builds on it:

| | Status | Why |
|---|---|---|
| **Arc mainnet** | not available, no date | Circle's contract list publishes testnet addresses only. No mainnet network exists in this facilitator, and none will be inferred from the node repository's genesis files |
| **EURC** | not accepted | The contract exists on Arc testnet (`0x89B50855Aa3bE2F677cD6303Cec089B5F319D72a`: `name` `EURC`, `version` `2`, 6 decimals, read 2026-09-16) but it is not registered here and has passed no end-to-end payment. Arc's asset allow-list holds USDC only, so an EURC payment is refused as `invalid_asset`. EURC is euros: when it comes, a dollar price will not convert 1:1 |
| **EIP-6492** (a counterfactual smart wallet, not deployed yet) | refused | The universal signature validator the facilitator calls, `0xdAcD51A54883eb67D95FAEb2BBfdC4a9a6BD2a3B`, has **no code on Arc** (0 bytes, 2026-09-16). `/verify` answers `isValid: false` with `invalid_signature`, and `/settle` sends nothing. That token is the same one a bad signature gets; the explanation is only in the server log |
| **Already-deployed EIP-1271 wallets** | not proven | The code path does not use the missing validator, but no positive payment from a contract wallet has been measured on Arc. Treat it as unsupported until one is |
| **Circle Gateway / Nanopayments authorizations** | refused | Gateway also advertises `exact` on `eip155:5042002`, but its buyers sign against a different domain (`GatewayWalletBatched`, version `1`) and it settles in batches. Those signatures are not USDC transfer authorizations and do not verify here; they belong to Circle Gateway. For this facilitator, sign the USDC domain above |
| **`upto`, `escrow` / `commerce`, ERC-8004** | not on Arc | None of those per-network lists names Arc. A network being served for `exact` does not enable anything else |
| **USYC** | not accepted | Listed by Circle on Arc testnet, not registered here, and not a plain stablecoin: its authorization, units and eligibility have not been analysed |

**EIP-6492 has a path; it has not been walked.** The deterministic CREATE2 factory
(`0x4e59b44847b379578588920cA78FbF26c0B4956C`) **is** deployed on Arc (69 bytes,
2026-09-16), which is what would let the validator be placed at its usual address.
That is a separate change, and it will be announced when a real EIP-6492 payment has
settled — not when the contract is merely deployed.

## 7. Testing on Arc testnet

| | |
|---|---|
| Public RPC | `https://rpc.testnet.arc.io` |
| Explorer | `https://testnet.arcscan.app` |
| Faucet (USDC for gas **and** for paying) | `https://faucet.circle.com` |
| Fees | EIP-1559. Circle documents a minimum `maxFeePerGas` of 20 gwei; base fee read 20 gwei on 2026-09-16 |

Two things that look like our bug and are not:

- **Arc blocks `0x70997970C51812dc3A010C7d01b50e0d17dc79C8` from genesis** — account #1
  of the public Foundry/Anvil test mnemonic. A USDC transfer to it reverts with
  `execution reverted: Blocked address`. Do not test with the default Anvil accounts.
- A generic Anvil fork does not reproduce Arc's native-USDC rules. Test against the
  testnet, or Circle's Arc Foundry tooling.

## 8. Running your own facilitator with Arc

Arc is served only where `RPC_URL_ARC_TESTNET` is set; without it the network is in
the code and served by nothing, which is exactly how it ships. `.env.example`
carries the line commented out on purpose:

```bash
RPC_URL_ARC_TESTNET=https://rpc.testnet.arc.io
```

Arc is a testnet, so the signer is your EVM testnet key (`EVM_PRIVATE_KEY_TESTNET`).
It pays Arc gas **in USDC** — there is no ETH on Arc — so it needs USDC from the
faucet before it can settle anything.

Nothing checks that the URL you configure is really Arc. The EIP-712 domain is
built with chain id 5042002 from the facilitator's table, while the transaction goes
to whatever chain the RPC answers for. Point it at another chain and every settle
fails there, and the failure looks like a problem with the payment rather than with
the configuration. Check with `eth_chainId` (expect `0x4cef52`) before you switch it
on.

---

## Sources

Measured for this page on 2026-09-16 against `https://rpc.testnet.arc.io`, pinned to
block 62,400,328: `eth_chainId`; USDC `decimals()`, `name()`, `version()` and
`DOMAIN_SEPARATOR()` (recomputed locally, matches); EURC `name()`, `version()` and
`decimals()`; `eth_getCode` on the EIP-6492 validator (0 bytes), the CREATE2 factory
(69 bytes) and EURC (1,798 bytes); the base fee. Explorer and faucet answered HTTP 200. The blocked genesis address and the
two-precision balance were measured on 2026-09-16 at block 62,335,077 while the
network was being added. The answers of `POST /verify` quoted above were taken from
production on 2026-09-16 with placeholder signatures; nothing was signed or settled.

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

Other networks are documented the same way; the next one is [Hedera](hedera.md),
which is a template until it is integrated.
