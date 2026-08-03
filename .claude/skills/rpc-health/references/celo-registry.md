# Celo RPC discovery and 2026-08-03 snapshot

## Discovery

Celo validator groups publish RPC URLs in on-chain group metadata. `celocli`
reads the registry:

```bash
npx --yes @celo/celocli@latest network:rpc-urls --node https://rpc.celo-community.org
```

Output is a table of `Validator Group Name | RPC URL | Validator Address`.
Takes ~1-2 min including the npx download. Any working Celo RPC can serve as
the `--node`; the registry itself is on-chain, so the node only has to be able
to read it.

This is authoritative for "who offers a Celo RPC" and is the right starting
point when the current endpoint dies. It does **not** say anything about
whether those nodes work -- of the 19 registered on 2026-08-03, 6 were dead or
misconfigured and 2 more were millions of blocks stale.

Chain identity: mainnet chain id **42220** (`0xa4ec`). Celo is an L2 since the
2025 migration; blocks are ~1s. Mainnet USDC is
`0xcebA9300f2b948710d2653dD7B07f33A8B32118C`.

## Snapshot: 2026-08-03

22 endpoints tested (19 registered + forno + celo-community + celocolombia).
30-round soak at 4s, then 60-request burst at 20 concurrent.

### Usable

| RPC | Operator | soak errors | burst | lag | p95 (soak) |
|---|---|---|---|---|---|
| `celo-rpc.quickapi.com` | ChainLayer | 0/30 | 60/60 | 0 | 431ms |
| `cr1.plusv.io` | PlusV | 0/30 | 60/60 | 0 | 420ms |
| `celo.newroad.network` | The Celo Group | 0/30 | 60/60 | 0 | 484ms |
| `r3-celo.grassecon.org` | GrassrootsEconomics | 0/30 | 60/60 | 0 | 367ms |
| `forno.celo.org` | cLabs (official) | 0/30 | 60/60 | 1-3 | 419ms |
| `celo-rpc-01.stakely.io` | Stakely | 0/30 | **33/60, 27x 429** | 0 | 417ms |
| `celo-rpc-03.atweb3.dev` | atweb3 | not soaked | - | 4 | - |
| `rpcm1.usopp.club` | usopp.club | 0/30 | not burst | 0 | 606ms |
| `celo-rpc.easy2stake.com` | Easy2Stake | not soaked | - | 4 | - |
| `r4/r5-celo.grassecon.org` | GrassrootsEconomics | not soaked | - | 0-2 | - |

**Chosen primary: `celo-rpc.quickapi.com`** (ChainLayer). Commercial infra
operator, tightest tail under load (p95 818ms / max 838ms in burst), no rate
limit, always at tip.

Runners-up if it degrades: `cr1.plusv.io`, then `celo.newroad.network`.
`r3-celo.grassecon.org` had the best raw numbers but `r1` and `r2` of the same
fleet have dead DNS, which suggests the fleet is not closely maintained.

### Unusable

| RPC | Problem |
|---|---|
| `rpc.celocolombia.org` | head stuck at block 0, re-syncing (was our production primary) |
| `celol2.lb.us-east-2.prod.stake.capital` | chain id 1, Ethereum genesis, unsynced |
| `rpc.celo-community.org` | 23/30 soak errors -- LB over the broken pool |
| `celo-rpc.keyko.rocks` | 10.8M blocks stale |
| `rpc.chainstaker.com/celo` | 10.8M blocks stale |
| `celo-rpc1/2.perfectstake.com` | NXDOMAIN |
| `r1/r2-celo.grassecon.org` | NXDOMAIN |
| `celo-rpc1.making.cash` | NXDOMAIN |
| `spectrum-01.simplystaking.xyz/celo-mainnet-rpc` | HTTP 404 |
| `alfajores-rpc.celo-community.org` | HTTP 530 (origin down) -- we use celo-sepolia, not alfajores |

See `failure-modes.md` for what each failure looks like on the wire.

### Testnet

Celo Sepolia (`RPC_URL_CELO_SEPOLIA`) is `https://rpc.ankr.com/celo_sepolia`
and was healthy (block `0x1efa0eb`). Not changed.

## Re-running this

```bash
npx --yes @celo/celocli@latest network:rpc-urls --node https://celo-rpc.quickapi.com \
  | awk '{print $NF=="" ? "" : $(NF-1)}' | grep '^https' > /tmp/celo-candidates.txt

python3 .claude/skills/rpc-health/scripts/probe_rpc.py \
  --chain-id 42220 --url-file /tmp/celo-candidates.txt --soak 30 --burst 60
```

Check the awk column extraction against the actual table before trusting it --
group names contain spaces.
