# Arc and Hedera usage documentation, reconciled with `main` (2026-09-17)

Two documents were competing for `docs/networks/arc.md`: an operations runbook on
`main` and an integrator's usage guide on a side branch written when Arc was switched
off and Hedera did not exist in the repository. Both are worth keeping. This change
gives each its own name, rewrites the usage guide against what the facilitator serves
today, and turns the Hedera template into a measured guide.

Everything below was re-measured on **2026-09-17** against
`https://facilitator.ultravioletadao.xyz`, the Arc RPCs, the Hedera Mirror Node and the
source at `origin/main` = `5fcb57c5`. Where a value in either source document was
wrong, the measurement won.

## The baseline, measured

| | Value |
|---|---|
| `origin/main` | `5fcb57c5`, `VERSION` 2.33.0 |
| Deployed `/version` | **2.33.0** — production deployed mid-task, see below |
| `/supported` | 156 kinds, 84 unique identifier strings |
| v1 network names | 41 — **22 mainnets**, 19 testnets |
| Arc | `arc` / `eip155:5042` **and** `arc-testnet` / `eip155:5042002`, `exact`, USDC 6 decimals — both live |
| Hedera | `hedera:testnet` **and** `hedera:mainnet`, x402 **v2**, `exact`, a different `extra.feePayer` each |
| `Network` enum | **46** variants; the union of the four `variants()` copies is **43** |

## How the names came out, and why

| File | What it is |
|---|---|
| `docs/networks/arc.md` | The **usage** guide. Kept the plain name because that is what an integrator searches for and what every existing link already points at |
| `docs/networks/arc-operations.md` | The **operations** runbook from `main`, moved here verbatim except for a new header paragraph. Because `arc.md` still exists (with the usage guide in it), git records this as an add rather than a rename; `git log --follow -M -C` still traces it back |
| `docs/networks/hedera.md` | New. The usage guide for Hedera, parallel in structure to `arc.md` |
| `docs/guides/hedera-native.md` | **Untouched.** Already on `main` and already real — configuration, budget, settlement store, recovery, rollout status |

The brief for this change asked for a Hedera usage guide written from scratch. `main` already had
`docs/guides/hedera-native.md`, a dense and measured operator guide that landed with
the Hedera work. Restating its contents would have recreated the exact problem this
change exists to fix, so `docs/networks/hedera.md` covers only the integrator's half —
identifiers, assets and units, the 402, what the buyer signs, what comes back, what is
refused — and defers configuration, budget and recovery to it by link. The same split
as Arc.

## Inbound links

`git grep -n "networks/arc"` across the tree found three, plus one for Hedera:

| Location | Action |
|---|---|
| `README.md:42` | **Updated.** Now points integrators at `docs/networks/arc.md` and operators at `docs/networks/arc-operations.md` |
| `terraform/environments/production/arc.tf:2` | **Not touched** — see the open item below |
| `docs/networks/arc-operations.md:6-7` | Not affected: those are links to the two SDK repositories, not to this file |
| Hedera | No inbound links to `docs/networks/hedera.md` existed; `README.md` now carries one |

`docs/networks/arc.md` still exists and its second paragraph sends the reader to
`arc-operations.md`, so the Terraform comment resolves in one hop rather than breaking.

## Claims re-measured, and what changed

### Arc — the branch's guide was written while Arc was dark

| Claim on the branch | Today |
|---|---|
| "in the code, not live", "`/supported` does not list it" | **Wrong now.** Both networks are served. Banner rewritten |
| "Arc mainnet does not exist here and has no date" | **Wrong now.** `arc` / `eip155:5042` is live. Row deleted from "what does not work" |
| "there is **no** `arc` alias" | **Wrong now.** `networkAliases: ["arc","eip155:5042"]` |
| Single testnet column throughout | Rewritten as mainnet + testnet everywhere |
| Production `/version` `2.29.6`, `/verify` answers `400 Invalid CAIP-2 format` | Stale. That table replaced with the live shape |
| Explorer `https://testnet.arcscan.app` | **Now a `301`** to `https://explorer.testnet.arc.io/`. Canonical host used |
| Base fee 20 gwei | `eth_gasPrice` answered **25 gwei**. Circle's documented 20 gwei minimum kept as a minimum |

| Claim re-measured | Verdict |
|---|---|
| USDC `0x3600…0000`, 6 decimals, `name` `USDC`, `version` `2` | **Holds on both networks** |
| Domain separators | **Both confirmed** from `DOMAIN_SEPARATOR()` and recomputed locally. Mainnet `0x940506…ccdf84`, testnet `0x361191…11c8c6b0`. They match `arc-operations.md` |
| Chain ids | `0x13b2` (5042) and `0x4cef52` (5042002) |
| EIP-6492 validator `0xdAcD51A5…` has no code | **Holds, and now on mainnet too** — 0 bytes on both |
| CREATE2 factory `0x4e59b448…` deployed | **Holds** — 69 bytes on both |
| EURC not accepted | **Holds, and got stronger.** On mainnet that address has **no code at all** (0 bytes); testnet still 1,798 bytes. Pinned by `arc_accepts_its_usdc_and_nothing_else` |
| No `upto` / escrow / commerce / ERC-8004 on Arc | **Holds.** `/supported` lists `exact` only; Arc is absent from `UPTO_DEPLOYED_NETWORKS` and `supported_networks()` |
| `settlement_unconfirmed` is HTTP 502, `retryable: false`, no `Retry-After` | **Holds** — `src/handlers.rs`, `SettlementUnconfirmedResponse` in `src/types.rs` |
| `/health/ready?network=arc` | `status: "ok"`, `rpc: "ok"`, signer `gasOk: true` |

The decimals section was kept **textual and hardened**, per the brief: it now says
outright that the mistake is available on mainnet, with real money, today.

### Hedera — the template's gaps, filled or declared

Values the template left as `[GAP]` and the source answered:

| Gap | Measured answer |
|---|---|
| Whether a v1 name exists | **No.** `supports_v1()` is false for Hedera (`src/network.rs`); `/supported` publishes `networkAliases: ["hedera:testnet"]` and nothing else. Settle answers `only x402 v2 is supported` |
| Which assets are accepted | HBAR `0.0.0` (8 decimals) and native USDC — `0.0.429274` testnet, `0.0.456858` mainnet, both 6. Extra HTS tokens are opt-in as `token-id:decimals` |
| The fee payer | `0.0.10576385` on testnet, published in `/supported`, funded with 2,164,663,981 tinybars |
| Whether `payTo` may be an alias | **No.** `account aliases unsupported` — a canonical numeric entity id is required |
| `maxTimeoutSeconds` | Bounds the transaction's valid duration, which must be in `15..=180` **and** `<= maxTimeoutSeconds`. The official client defaults to 120s, so a merchant below that rejects its own buyers |
| What the facilitator refuses | Nine codec refusals and eight settle refusals, each quoted, with the adversarial vectors in `tests/hedera-e2e/vectors/` |
| Which field carries the id | `transaction` holds the native id `0.0.<fee payer>@<seconds>.<nanos>`; `payer` is **the buyer**, not the fee payer |
| Key types | Ed25519, ECDSA secp256k1, KeyList and 2-of-3 threshold all have vectors; contract keys are refused |
| Tested client | `@x402/hedera` 2.26.0 with `@hiero-ledger/sdk` 2.85.0 |
| Mirror endpoints | Public testnet / mainnet nodes, overridable per network |
| EVM chain ids 295/296 | Added `66d34e6c` (2026-04-04), removed `278842e5` (2026-05-30) — both commits confirmed |

**No gap was left open — because the answer changed while this was being written.**
The first pass measured `/version` 2.32.2 serving `hedera:testnet` only, and the guide
was written to say mainnet was not payable. A re-measurement immediately before
committing found production on **2.33.0** with `hedera:mainnet` served, fee payer
`0.0.10868300`, HBAR `0.0.0` and USDC `0.0.456858`. The guide was rewritten to the
second measurement: banner, the at-a-glance rows, section 1 and section 6.

That is the whole argument for re-measuring at commit time rather than trusting a
reading taken at the start of the work. It also means the numbers on this page are a
snapshot: `/supported` is the only thing that settles availability.

**What did stay, as a measured caveat:** `/health/ready` reported both Hedera networks
`degraded` with `reason: "signer_gas_low"` — `rpc: "ok"`, `gasOk: true`, but **28**
settles remaining on mainnet and **21** on testnet. That is the deliberately
conservative canary budget, not an outage, and the guide says so in section 1 and
section 6 rather than presenting Hedera as open-throughput.

### README

**`main` moved underneath this change.** A rebase before pushing landed on
`83ac6d07` (#70, "publish Arc and Hedera across network surfaces"), which had already
restructured the same region: it folded Arc and Hedera into both network tables,
dropped the `(21)` / `(18)` counts from the headers, added an "Arc and Hedera payment
identifiers" table, and replaced the stale `# => 121` comment.

The conflict was resolved **onto #70's version**, not by reasserting the earlier work.
Only the deltas that #70 had not already made were re-applied, each re-verified against
`/supported` first:

| Fix | Evidence |
|---|---|
| Celo `cUSD` → `USDC, USDT` | `/supported` `celo` carries `usdc` and `usdt`, no cUSD |
| Monad chain id `10143` → `143` | `networkAliases: ["monad","eip155:143"]` |
| Monad token `MON` → `USDC, AUSD, USDT` | `/supported` `monad` tokens |
| Testnet row `Monad Testnet` → `XRPL Testnet` | No Monad testnet is served; `xrpl-testnet` / `xrpl:1` is |
| Arc link split | #70 still pointed at `docs/networks/arc.md` for the runbook; now usage + operations |
| Hedera link | #70 linked only the operator guide; the usage guide is now named beside it |
| `docs/networks/<network>.md` in the update checklist | New, and it names the `-operations.md` convention |
| Counting note | Rewritten for #70's structure |

Dropped as already done by #70: the `# => 121` fix. Dropped as obsolete: the side
branch's "In the code, not served yet" section, whose only entry was Arc testnet as
switched off, and its "21 mainnets and 18 testnets" counts.

**The tables now match `/supported` exactly**, which is worth stating because it was
not true before: 23 mainnet rows against 23 served mainnets (22 with a v1 name, plus
`hedera:mainnet`, which has none), and 20 testnet rows against 20 served testnets. The
one row that broke the match was `Monad Testnet`, which is not served at all.

## Prose that is wrong today and was left alone

Reported rather than fixed, per the anti-scope-creep rule. Neither touches Arc or
Hedera:

- `README.md` testnet table: **"Celo Alfajores | 44787"**. The v1 name `/supported`
  actually serves is `celo-sepolia`, and this repository's `CLAUDE.md` records that
  `RPC_URL_CELO_ALFAJORES` does not exist. The chain id is right; the name is stale.
- `README.md` stablecoin matrix: the row **"Arc (when enabled)"**. Still technically
  true — Arc does have its own switches — but it reads as though Arc were not live.

Also measured, for whoever next touches the network tables: the `Network` enum has 46
variants while the union of the four `variants()` copies has 43. `Sei`, `SeiTestnet`
and `XdcMainnet` appear in the enum and in none of the four copies — unchanged in kind
from what was recorded before, though the absolute numbers moved when Hedera and Arc
landed.

## What was not touched

`VERSION`, `CHANGELOG`, `static/`, `src/`, `terraform/`, `.github/`, `Cargo.*`. This
change is documentation only.

## Two gates, run locally

- **`.github/workflows/no-account-id.yml`**, the gate that runs on every pull request,
  re-implemented against the five changed files: no ARN carrying an account, no ECR
  host, no bare 12-digit number, no Secrets Manager name with its random suffix. Clean.
- **`.githooks/pre-commit`**, the anti-key hook, run against the real staged diff. It
  flagged `hex64` twice, and both were addressed rather than bypassed:
  - The runbook's own separators and receipt hashes — already on `main`, flagged only
    because a move registers as an add. Fixed by landing the move as its **own commit**,
    a pure `R100` rename with zero added lines, which the hook passes.
  - A new row in `arc.md` carrying both separators in full. Fixed the way the hook's own
    remediation advises: the table now abbreviates them, and section 4 points at the two
    places that already hold the exact 32 bytes — `ARC_USDC_DOMAIN_SEPARATOR` in
    `src/chain/evm.rs` and `arc-operations.md`.

  The hook now exits 0 on the full change. Nothing was committed with `--no-verify`.

No Rust, Python or Terraform file is touched, so no compiler, clippy or `terraform
validate` run applies. What was run instead: the two gates above, plus the measurement
commands quoted throughout this page.

## Open item

`terraform/environments/production/arc.tf:2` still says the canary is in
`docs/networks/arc.md`; it is now in `docs/networks/arc-operations.md`. Left alone
because the brief for this change puts Terraform out of scope, and because a `.tf` edit would pull
the Terraform CI jobs into a prose-only pull request. The link resolves in one hop
through `arc.md`. A one-line comment fix is available on request.
