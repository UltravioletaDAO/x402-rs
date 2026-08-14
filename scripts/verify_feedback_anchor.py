#!/usr/bin/env python3
"""Close the chain document -> on-chain feedbackHash for an ERC-8004 feedback.

Read-only: no key, no gas, nothing signed. Answers one question -- is the
`feedbackHash` written on-chain the keccak256 of a document a third party can
actually fetch today?

Two things make this harder than it sounds, both measured on Base mainnet on
2026-08-13:

1. `readFeedback` does NOT return `feedbackHash`. All 29 selectors in the
   deployed implementation (0x16e0fa7f7c56b9a767e34b192b51f921be31da34, the impl
   behind proxy 0x8004BAa1...) were enumerated from its bytecode: nine are the
   reads and writes our ABI already declares, the rest are OpenZeppelin proxy /
   ownable machinery and three that revert on every read shape. The hash exists
   ONLY in the `NewFeedback` event. Auditing an anchor therefore depends on log
   availability, not on contract state.

2. Every public Base RPC caps `eth_getLogs` between 10 and 10.000 blocks
   (mainnet.base.org, drpc, blastapi, 1rpc), and publicnode wants a token. But
   historical `eth_call` works, so this walks the other way round: binary-search
   `getLastIndex(agentId, client)` over block height to find the exact block
   where a given feedbackIndex landed, then read that single block's logs.

Usage:

    python scripts/verify_feedback_anchor.py \
      '[{"agentId":18896,"client":"0x1030...","feedbackIndex":154}]'

Optional per-entry `lo` sets the lower bound of the search (default 47.000.000,
which is fast for recent feedback; use 25.000.000 for older entries).

Result on the first run (2026-08-13), both MATCH:

  * agent 18896, client 0x103040545AC5031A11E8C03dd11324C7333a13C7, index 154
    -> 0x582b03e6...c10c3, document 654 bytes, keccak identical
  * agent 58517, same client, index 109
    -> 0x96088089...a557, document 649 bytes, keccak identical

And one finding: feedback written by third-party raters on the same agent
carries a non-empty `feedbackHash` with an EMPTY `feedbackURI` -- a hash
committing to a document nobody can ever produce. That anchor is unresolvable by
construction, not by a CDN misconfiguration, and no infrastructure fix recovers
it.
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

RPC = "https://mainnet.base.org"
REP = "0x8004BAa17C55a88189AE136b182e5fdA19dE9b63"
NEW_FEEDBACK_TOPIC = (
    # keccak of
    # NewFeedback(uint256,address,uint64,int128,uint8,string,string,string,string,string,bytes32)
    "0x6a4a61743519c9d648a14e6493f47dbe3ff1aa29e7785c96c8326a205e58febc"
)

# foundry's cast, used only for keccak256 so this script needs no python crypto
# dependency. Override with CAST=/path/to/cast.
CAST = os.environ.get("CAST", os.path.expanduser("~/.foundry/bin/cast"))


class RevertedCall(Exception):
    """The node answered: the call reverted (e.g. no code at that block)."""


def rpc(method, params, retries=6):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    last = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(
                RPC, data=body.encode(), headers={
                    "content-type": "application/json",
                    "user-agent": "x402-chain-check/1.0 (read-only audit)",
                }
            )
            with urllib.request.urlopen(req, timeout=30) as r:
                out = json.load(r)
            if "error" in out:
                last = out["error"]
                msg = str(last)
                # A revert is an ANSWER, not a transport failure: before the
                # registry was deployed the call has no code to run. Retrying it
                # six times with backoff just makes the search take an hour.
                if "revert" in msg or "no contract code" in msg:
                    raise RevertedCall(msg)
                time.sleep(1.5 * (attempt + 1))
                continue
            return out["result"]
        except RevertedCall:
            raise
        except Exception as e:  # noqa: BLE001
            last = str(e)
            time.sleep(1.5 * (attempt + 1))
    raise RuntimeError(f"rpc {method} failed: {last}")


def pad_u256(n):
    return "%064x" % n


def pad_addr(a):
    return "%064x" % int(a, 16)


def last_index_at(agent_id, client, block):
    # getLastIndex(uint256,address) = 0xf2d81759
    data = "0xf2d81759" + pad_u256(agent_id) + pad_addr(client)
    tag = "latest" if block is None else hex(block)
    out = rpc("eth_call", [{"to": REP, "data": data}, tag])
    # Empty data on a VALID call: no code at that address yet at that height.
    # Exactly the shape EM hit after their delegate deploy, where it read as a
    # failed deploy -- here it is simply "the registry does not exist yet".
    if out in ("0x", "0x0", ""):
        raise RevertedCall("empty return: no code at that block")
    return int(out, 16)


def find_block_of_index(agent_id, client, target, lo, hi):
    """Smallest block where getLastIndex >= target."""
    while lo < hi:
        mid = (lo + hi) // 2
        try:
            v = last_index_at(agent_id, client, mid)
        except RevertedCall:
            # No registry at that height yet -> the index is certainly below.
            lo = mid + 1
            continue
        except RuntimeError:
            # pruned/unavailable state: walk forward
            lo = mid + 1
            continue
        if v >= target:
            hi = mid
        else:
            lo = mid + 1
        time.sleep(0.15)
    return lo


def decode_new_feedback(log):
    """Decode the non-indexed tail of NewFeedback.

    Non-indexed, in order: feedbackIndex uint64, value int128, valueDecimals uint8,
    tag1 string, tag2 string, endpoint string, feedbackURI string, feedbackHash bytes32
    """
    data = log["data"][2:]
    words = [data[i : i + 64] for i in range(0, len(data), 64)]

    def word(i):
        return int(words[i], 16)

    def read_string(offset_word_index):
        off = word(offset_word_index) // 32
        length = word(off)
        raw = "".join(words[off + 1 :])[: length * 2]
        return bytes.fromhex(raw).decode("utf-8", "replace")

    feedback_index = word(0)
    value = word(1)
    if value >= 2**127:
        value -= 2**128
    value_decimals = word(2)
    tag1 = read_string(3)
    tag2 = read_string(4)
    endpoint = read_string(5)
    feedback_uri = read_string(6)
    feedback_hash = words[7]
    return {
        "feedbackIndex": feedback_index,
        "value": value,
        "valueDecimals": value_decimals,
        "tag1": tag1,
        "tag2": tag2,
        "endpoint": endpoint,
        "feedbackURI": feedback_uri,
        "feedbackHash": feedback_hash,
        "txHash": log["transactionHash"],
        "block": int(log["blockNumber"], 16),
    }


def logs_for(agent_id, client, block):
    params = [
        {
            "address": REP,
            "topics": [
                NEW_FEEDBACK_TOPIC,
                "0x" + pad_u256(agent_id),
                "0x" + pad_addr(client),
            ],
            "fromBlock": hex(block),
            "toBlock": hex(block),
        }
    ]
    return rpc("eth_getLogs", params)


def keccak_of(raw: bytes) -> str:
    """keccak256 via foundry's cast (no python keccak dependency here)."""
    p = subprocess.run(
        [CAST, "keccak", "0x" + raw.hex()],
        capture_output=True,
        text=True,
        check=True,
    )
    return p.stdout.strip().lower().removeprefix("0x")


def fetch(url, timeout=20):
    req = urllib.request.Request(url, headers={"user-agent": "x402-chain-check/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, r.headers.get("content-type", ""), r.read()


def normalize(h):
    return h.strip().lower().removeprefix("0x")


def main():
    targets = json.loads(sys.argv[1])
    latest = int(rpc("eth_blockNumber", []), 16)
    print(f"[INFO] Base latest block = {latest}\n")

    results = []
    for t in targets:
        agent, client, index = t["agentId"], t["client"], t["feedbackIndex"]
        print(f"=== agent {agent} client {client} index {index} ===")
        blk = find_block_of_index(agent, client, index, t.get("lo", 47_000_000), latest)
        print(f"    block where index {index} landed: {blk}")
        logs = logs_for(agent, client, blk)
        if not logs:
            print("    [FAIL] no NewFeedback log in that block")
            continue
        decoded = [decode_new_feedback(l) for l in logs]
        match = [d for d in decoded if d["feedbackIndex"] == index]
        if not match:
            print(f"    [WARN] block has indexes {[d['feedbackIndex'] for d in decoded]}")
            match = decoded
        d = match[0]
        print(f"    tx           : {d['txHash']}")
        print(f"    tag1/tag2    : {d['tag1']!r} / {d['tag2']!r}")
        print(f"    value        : {d['value']} (decimals {d['valueDecimals']})")
        print(f"    feedbackURI  : {d['feedbackURI']}")
        print(f"    feedbackHash : 0x{d['feedbackHash']}")

        row = {**d, "agentId": agent, "client": client}
        uri = d["feedbackURI"]
        if uri.startswith("http"):
            try:
                status, ctype, body = fetch(uri)
                digest = keccak_of(body)
                row.update(
                    {
                        "httpStatus": status,
                        "contentType": ctype,
                        "bytes": len(body),
                        "keccak": digest,
                        "match": normalize(d["feedbackHash"]) == digest,
                    }
                )
                print(f"    document     : HTTP {status} {ctype} {len(body)} bytes")
                print(f"    keccak(doc)  : 0x{digest}")
                verdict = "MATCH" if row["match"] else "MISMATCH"
                print(f"    >>> {verdict}")
                if len(body) < 1200:
                    print(f"    body: {body[:1200]!r}")
            except Exception as e:  # noqa: BLE001
                row["fetchError"] = str(e)
                print(f"    document     : [FAIL] {e}")
        else:
            print("    document     : not an http(s) URI, cannot fetch")
        results.append(row)
        print()

    print(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
