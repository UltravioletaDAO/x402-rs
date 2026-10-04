# SMA-7702 foreign delegation

Status: design, plus a local stub. The relay still answers
`relay_foreign_delegation`. This branch does not call a chain.

Ticket: SMA-7702 (KarmaKadabra, 2026-09-27). Branch:
`grok/sma-7702-erc1271-foreign-delegation`.

## Problem

86 KarmaKadabra ratings fail with `relay_foreign_delegation`. The rater EOA
is EIP-7702-delegated to Alchemy `SemiModularAccount7702` (the first PayBox
gasless money-op installs that delegation).

`DelegationState::Foreign` in `src/erc8004/relay.rs` rejects that on purpose.
Re-pointing the account at Execution Market's `FeedbackDelegate` would break
those money-ops, and the three teams agreed not to touch the delegation.
The rule is correct. There is no alternate path, so those raters cannot
finish a rating whose on-chain author is the rater.

Prepare and submit both turn `Foreign` into HTTP 400
`relay_foreign_delegation` (`src/handlers.rs`). `Supersedable` is only for
an older `FeedbackDelegate` of ours, told apart by `REPUTATION_REGISTRY()`.
An SMA has no such function and stays `Foreign`.

## What must stay true

- Do not ask the rater to sign a new EIP-7702 authorization that replaces
  the SMA.
- The Reputation Registry records `msg.sender` as the author. The deployed
  registry has no `giveFeedbackWithSignature` (see the module docs in
  `src/erc8004/relay.rs`). A call whose sender is the facilitator, or a
  sibling contract, is not a rating by the agent.
- While the account is delegated to the SMA, a transaction sent to the
  rater's address runs SMA code, not `FeedbackDelegate`.

## Published implementations

Alchemy documents one address per `SemiModularAccount7702` version, the
same on every EVM chain
(<https://www.alchemy.com/docs/wallets/smart-contracts/deployed-addresses>,
read 2026-10-04). This branch did not read the 86 raters, so both published
7702 implementations are allowlisted and nothing else is.

| Version | Contract | Address |
| --- | --- | --- |
| v1.0.0 | SemiModularAccount7702 | `0x69007702764179f14F51cdce752f4f775d74E139` |
| v1.1.0 | SemiModularAccount7702 | `0x77021100bD87b7008E5E1989d0eB38555d0d0000` |

v1.1.0 is Alchemy's default for new EIP-7702 accounts from 2026-09-21.
Existing v1.0.0 delegations stay put. `SemiModularAccountBytecode` and
`SemiModularAccountStorageOnly` are different contracts and are not 7702
delegates. Wallet APIs reject any other delegation address.

The 7702 designator on the rater is `0xef0100` plus one of those 20-byte
addresses. Recognition uses that, not a list of rater EOAs.

## Option A — ERC-1271 for a known SMA

When the designator target is one of the two addresses above, verify the
rater with the account's ERC-1271 path instead of requiring
`FeedbackDelegate` code on the account.

KarmaKadabra already signs EIP-3009 this way in production. The account
accepts `isValidSignature(bytes32,bytes)` (`0x1626ba7e`). The fallback
validation checks an EOA signature over `replaySafeHash(inner)`:

- domain type `EIP712Domain(uint256 chainId,address verifyingContract)`,
  `verifyingContract` = the rater account, no name, version, or salt;
- struct `ReplaySafeHash(bytes32 hash)`.

For a 7702 SMA that has not moved its fallback signer, that signer is
`address(this)`, the rater EOA.

The SDK packs the signature as `pack1271Signature` for entity 0 and an EOA
signer (`packages/smart-accounts/src/ma-v2/utils/signature.ts`):

```
0x00 || uint32(0) || 0xFF || 0x00 || ecdsa[65]
```

Entity 0 is the reserved fallback signer. The trailing `0x00` is
`SignaturePrefix.EOA`. A contract signer (`0x01`) or any other entity id is
a different check and is not handled locally.

### Local stub

`src/erc8004/sma7702.rs` implements that recognition and that envelope
check. It does not import a provider. Tests pin:

- the two implementation addresses, and the rejection of the bytecode and
  storage-only variants and of the Base `FeedbackDelegate`;
- the designator parser;
- the two typehashes from `SemiModularAccount.sol` and two full digests
  computed with an independent keccak;
- recovery of a wrapped EOA signature, and refusal of a bare signature, a
  wrong locator, trailing bytes, a stranger, and a high-`s` encoding.

`delegation_state` does not call the module. A known SMA is still
`Foreign`.

### What the stub is not

`sma_1271_eoa_authorises` recovers the default fallback signer. If the
account later sets a different fallback signer, the local check can succeed
while `isValidSignature` fails. The chain call is the verdict. This branch
does not make that call.

### Authorship is still open

Checking the signature does not make the rater `msg.sender`. A sibling
`FeedbackDelegate` that is callable from outside, checks `isValidSignature`
on the rater, and then calls `giveFeedback` would itself be the author.
That fails the ticket.

Paths that keep the author equal to the rater:

1. The SMA account executes `giveFeedback` itself (an SMA execute / PayBox
   money-op). That is Option B's shape, using this envelope as the
   authorization the account already understands.
2. Execution Market adds a registry entry point that records an explicit
   client address after verifying ERC-1271. The current registry has no
   such function. Shipping it is an EM change, not a facilitator-only one.

Temporarily installing `FeedbackDelegate` for one transaction is the thing
`Foreign` exists to prevent. EIP-7702 applies authorizations before
execution, so a type-4 transaction cannot run as `FeedbackDelegate` and
finish still delegated to the SMA unless the rater signs the SMA delegation
back, which is the overwrite the teams refused.

Do not wire Option A into prepare/submit until one of those two authorship
paths exists and an on-chain `isValidSignature` has been checked against a
real SMA. The local function is the digest and envelope half of that work.

## Option B — the SMA calls the registry

The account executes `giveFeedback` through a PayBox gasless money-op.
`msg.sender` is the rater, so authorship holds without a new registry
function and without touching the delegation.

KarmaKadabra would add a tool. The facilitator and Execution Market would
accept that feedback as the task's signed rating: rated-status, dedup, and
`superseded_by`. No code for this option is in this branch. The sweep
scripts named in the ticket (`scripts/kk/sweep_ratings_firmados.py`,
`emitir_publisher_rates_executor.py`) live in the KarmaKadabra repo, not
here.

## Done when (the ticket, not this branch)

`scripts/kk/sweep_ratings_firmados.py` (dry-run) and
`emitir_publisher_rates_executor.py --todos` no longer exclude SMA raters,
and a `--limite 1` emission seals with author = the agent account.

This branch is the design and the local ERC-1271 stub only. It does not
meet that bar.
