# RPC failure modes that return HTTP 200

Every one of these was observed on a real endpoint during the 2026-08-03 Celo
sweep. None of them is caught by a liveness check, and most are not caught by
`eth_chainId` either. This is why `probe_rpc.py` checks what it checks.

## 1. Right chain, head stuck at block 0

`rpc.celocolombia.org` -- our production Celo RPC at the time.

```
eth_chainId     -> 0xa4ec          correct, Celo mainnet
eth_blockNumber -> 0x0             three samples, all zero
eth_syncing     -> currentBlock 0x0, highestBlock 0x434abb3, startingBlock 0x3e86600
eth_getCode     -> -32801 "no historical RPC is available for this historical
                   (pre-L2) execution request"
```

The node was re-syncing from scratch and never finished. It answered every
request with a 200 and the correct chain id, so any naive health check passed
-- while every state read failed, meaning every settle on Celo failed. It had
been like this for days.

The `-32801` is a Celo-L2-specific tell: with head at 0, `latest` resolves to a
pre-L2 block, so state execution is refused.

**Detection:** `eth_blockNumber == 0`, or `eth_syncing.currentBlock == 0`.

## 2. Wrong chain entirely, unconfigured genesis

`celol2.lb.us-east-2.prod.stake.capital` -- StakeCapital's *officially
registered* Celo validator RPC.

```
eth_chainId       -> 0x1                    Ethereum, not Celo (0xa4ec)
eth_blockNumber   -> 0x0
block 0 hash      -> 0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3
web3_clientVersion-> Geth/v0.1.0-untagged-b3cca990-20250813
```

That block hash is the **Ethereum mainnet genesis**. A freshly initialized Geth
with the default genesis, never pointed at Celo, never synced. Alternate paths
(`/celo`, `/rpc`, `/42220`) all 404, so it was not an auth-path problem.

The URL was correct -- it matched what validator `0xbe505Db3...BeA4` has
registered on-chain. The *node* was broken. Do not assume a bad response means
a bad URL.

**Detection:** `eth_chainId` mismatch. Adopting this as Celo's primary would
have been worse than the outage it was meant to fix.

## 3. Fast and healthy-looking, millions of blocks stale

`celo-rpc.keyko.rocks` (p50 255ms) and `rpc.chainstaker.com/celo` (p50 454ms).

Correct chain id, non-zero head, good latency -- and **10,807,398 blocks
behind** the tip (~63M vs ~73.8M). The nastiest of the set, because latency
benchmarks rank them near the top.

**Detection:** compare each candidate's head against the max head observed
across all candidates. `probe_rpc.py` fails anything more than `MAX_LAG` behind.

## 4. Load balancer fronting the broken pool

`rpc.celo-community.org` -- the endpoint published on celo-community.org.

A single curl succeeded. A 30-round soak gave **23/30 errors (77%)**, p95 2021ms,
106 blocks of lag, with a mix of HTTP 500s and timeouts.

It round-robins over the same validator-operated nodes in the on-chain registry
-- including the two broken ones above. So it statistically inherits their
failure rate. A community LB is the *worst* option, not the safest one, because
it launders individual node failures into intermittent ones.

**Detection:** only the soak catches this. One probe is not evidence.

## 5. Rate limiting under concurrency

`celo-rpc-01.stakely.io` -- 0 errors across a 30-round soak, then **27 of 60**
concurrent `eth_call`s returned HTTP 429 at ~33 req/s.

Invisible to sequential testing. With no fallback in the facilitator, a 429 is
an outage.

**Detection:** the burst probe.

## 6. Cached edge serving a lagging head

`forno.celo.org` -- the official cLabs endpoint. Zero errors, no rate limit,
but consistently 1-3 blocks behind tip across every round (it sits behind
Cloudflare). Nonce agreed with every other candidate, so it is safe, just
slower on the write path: each settle's receipt wait starts a couple of seconds
late.

Usable. Just not the best choice when tip-fresh alternatives exist.

## 7. The measurement artifact: User-Agent blocking

The first sweep marked 10 of 22 endpoints as `HTTP 403 Forbidden` -- including
`forno.celo.org`, which had answered curl seconds earlier. The cause was
Python's default `Python-urllib/3.x` User-Agent.

Sending a browser-shaped UA recovered every one of them. `probe_rpc.py` always
sends one. If a sweep shows a suspicious cluster of 403s, suspect the client
before the servers.
