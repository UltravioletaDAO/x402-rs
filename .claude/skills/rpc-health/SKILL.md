---
name: rpc-health
description: Find, qualify and replace a blockchain RPC endpoint for the x402 facilitator. Use when an RPC is suspected down, degraded or rate-limited ("el RPC de celo esta caido", "necesitamos otro RPC", "cambiar el RPC de X"), when evaluating a candidate endpoint someone offered, or when a network's verify/settle calls fail while the facilitator itself is healthy. Includes Celo validator-registry discovery via celocli.
---

# RPC Health

Qualifying a replacement RPC and swapping it in. The hard part is not finding
an endpoint -- it is proving the one you found actually works, because the
common failure modes all return HTTP 200.

## When to use

- A network's settles fail but `/health` is fine and other networks work
- Someone hands over an RPC URL to adopt ("usemos este que es privado")
- Periodic re-check of a community/free endpoint we depend on

## Step 1 - Confirm the incumbent is actually broken

Do not take "esta caido" at face value, and do not take HTTP 200 as healthy
either. Run all four:

```bash
U=https://the-current-rpc.example
for m in eth_chainId eth_blockNumber eth_syncing web3_clientVersion; do
  echo -n "$m: "
  curl -s -m 12 -X POST "$U" -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$m\",\"params\":[]}"; echo
done
```

Read `references/failure-modes.md` before interpreting the output. `eth_chainId`
alone proves nothing.

## Step 2 - Build the candidate list

**Celo** has an on-chain registry of validator-operated RPCs. This is the
highest-value source and most people do not know it exists:

```bash
npx --yes @celo/celocli@latest network:rpc-urls --node https://rpc.celo-community.org
```

Returns every RPC URL registered in validator group metadata, with the operator
name and validator address. See `references/celo-registry.md` for the 2026-08-03
snapshot and which ones were usable.

For other chains: chainlist.org, the chain's docs, `src/network.rs` defaults,
and whatever the operator offered. Put one per line in a file.

## Step 3 - Qualify the candidates

```bash
S=.claude/skills/rpc-health/scripts

# 1. matrix: chain id, head freshness, full method coverage
python3 $S/probe_rpc.py --chain-id 42220 --url-file candidates.txt

# 2. soak: this is the one that decides. Single samples lie.
python3 $S/probe_rpc.py --chain-id 42220 --url-file candidates.txt --soak 30

# 3. burst: finds rate limits, which are an outage with no fallback
python3 $S/probe_rpc.py --chain-id 42220 --url-file candidates.txt --burst 60

# 4. nonce agreement, if this chain is on the write path
python3 $S/probe_rpc.py --chain-id 42220 --url-file candidates.txt --nonce-check
```

Pass `--token` and `--wallet` for non-Celo chains (defaults are Celo USDC and
the mainnet facilitator EOA). Copy those from `src/network.rs` and
`lambda/balances/handler.py` -- never type an address from memory.

The matrix stage feeds only its survivors into the later stages, so the soak
and burst output is already filtered.

## Step 4 - Choose

Rank by, in order:

1. **Zero soak errors.** A candidate with any error rate is out. There is no
   fallback in the facilitator (see below), so the primary must not flap.
2. **Zero rate limiting under burst.** 429s are an outage.
3. **Lag 0 at the tip.** A node a few blocks behind slows every receipt wait
   after a settle, bounded by `TX_RECEIPT_TIMEOUT_SECS`.
4. **Operator type.** A commercial infra provider beats a single validator's
   single node, which beats a community load balancer. LBs are worst: they
   inherit every broken backend in the pool.
5. p95 latency, last. All healthy candidates land within a few hundred ms.

## Step 5 - Swap it in

`RPC_URL_<NETWORK>` lives in **five** places. Missing one leaves the landing
page or the balances Lambda pointed at the dead endpoint:

| # | Location | Applies at |
|---|---|---|
| 1 | AWS Secrets Manager `facilitator-rpc-mainnet`, key = lowercase network | next ECS task start |
| 2 | `terraform/environments/production/lambda-balances.tf` | `terraform apply` |
| 3 | `lambda/balances/handler.py` (`PUBLIC_RPCS`) | Lambda redeploy |
| 4 | `static/index.html` (frontend balance fallback map) | facilitator rebuild |
| 5 | `.env.example` | docs only |

Testnet RPCs live in `facilitator-rpc-testnet`.

**Secrets Manager is a read-modify-write.** `update-secret --secret-string`
replaces the entire JSON document, so pulling the current value, editing one
key and writing it back is mandatory -- otherwise the other ~12 RPC URLs are
destroyed. `scripts/swap_secret_rpc.py` does this safely and verifies the key
set is unchanged. Rollback is the `AWSPREVIOUS` version stage; no secret is
ever written to disk.

Then verify after deploy:

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].network]|unique|length'
```

## No fallback exists

`src/chain/evm.rs` (`FromEnvByNetworkBuild::from_env`) reads exactly one env
var per network via `from_env::rpc_env_name_from_network`, which returns a
single `&'static str`. Same for solana/near/stellar/xrpl/algorand. There is no
failover, no retry against a second URL, no health gating.

The balances Lambda *does* have a fallback chain
(`lambda/balances/handler.py`: private -> env -> `PUBLIC_RPCS`).

So: the chosen primary is a single point of failure for that network, and the
qualification bar above is set accordingly. Adding real multi-RPC failover
would start at `rpc_env_name_from_network` returning a list.
