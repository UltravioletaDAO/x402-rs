> **Update 2026-09-04 (after this document was written):** the 5 Solana rows marked `verified: true` by the code that predated v1.82.0 were set to `verified: false` (`notVerifiedReason: pre_gate_self_asserted_2026_08_18`). The certified corpus is now **100 % EVM**; re-count after the correction: **119 verified across 7 networks**. The numbers below are the ones measured before that correction and are kept as they were; §5 reproduces the current state.

# `durable-evidence` — Reproducible evidence for the upstream PR

**Snapshot:** 2026-09-04 03:13Z. **Facilitator in production:** `2.10.0` (`curl -s https://facilitator.ultravioletadao.xyz/version`).
**Source:** a full `scan` of the `facilitator_dx402_evidence` table (us-east-2, 827 items, no `LastEvaluatedKey`) plus the public API. Every number in this document comes from a command shown here; none comes from a third-party report.

The test address `0x1111111111111111111111111111111111111111` is excluded (6 rows carry it as payer or payee, 1 of them `verified`). Everything below is measured without them.

---

## 1. Corpus

```bash
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --output json > scan.json
python3 corpus.py   # section 5
```

| Metric | Value |
|---|---|
| Anchors in the table | **827** (821 without the test address) |
| `verified: true` | **121** — 116 on EVM chains, 5 on Solana (see §4: the Solana ones are **not** verified against the chain) |
| `signed: true` | **111** (110 of them also `verified`) |
| Networks with `verified` | **8**: avalanche 49, arbitrum 37, optimism 9, base 7, monad 7, ethereum 6, solana 5, polygon 1 |
| Distinct buyers (among `verified`) | **30** (26 counting EVM only) |
| Distinct sellers (among `verified`) | **28** (24 counting EVM only) |
| First / last `anchoredAt` (`verified`) | 2026-08-18T19:22:17Z / 2026-09-03T23:24:58Z |
| First / last `anchoredAt` (all) | 2026-08-17T21:20:05Z / 2026-09-03T23:24:58Z |
| `keyAlg` among `verified` | ECIES-secp256k1 116, ECIES-X25519 5 |
| `mode` / `retention` / backend among `verified` | `direct` / `90d` for all; ipfs 109, s3 12 |

Two notes on the table itself:

- `/dx402/stats` reports `anchored: 730` against 827 rows: the counter is a floor by design (the endpoint says so itself: *"records whose index write failed are not counted"*). The table is authoritative.
- 8 rows (2026-08-17/18) have no item-level `verified` attribute; they predate the current schema and count as not verified. In the remaining 819 the top-level BOOL matches the `verified` inside `record` (0 discrepancies).

## 2. Reproducible sample

Ten `paymentId`s with `verified: true`, spread across networks and with distinct sellers (polygon has only one verified anchor and shares its payee with the monad one). For each:

```bash
curl -s https://facilitator.ultravioletadao.xyz/dx402/evidence/<paymentId> | jq '{verified,signed,receiptSigner}'
```

The API returns `verified`, `signed`, `receiptSigner` and the signature in `receipt`; the network and the `txHash` live in `/dx402/receipt/<paymentId>` (`.receipt.network`, `.receipt.txHash`). All 10 answered `receiptSigner: 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF`.

| # | Network | `paymentId` | `verified` | `signed` | `txHash` |
|---|---|---|---|---|---|
| 1 | avalanche | `0x7e7ca6a10e0d2a835fd0d8045d0a95a6ee5f5dd29b39696e69c3c21c7a3387ec` | true | true | [`0x71799e37…22ced4`](https://snowtrace.io/tx/0x71799e37a07e5b8b7edf96f6c2c3b62455c4dad859c1885ef995fdd95622ced4) |
| 2 | arbitrum | `0x7ecc7e392df64e78a7aa07d7934ee75e6f08ca29300d86ba1382c576aff11111` | true | true | [`0xf9ba043d…e159e6`](https://arbiscan.io/tx/0xf9ba043d55a2871e4aeb54a4d4854bc92f8200f3e519a4feb8264a8f52e159e6) |
| 3 | optimism | `0x7611a8d13a5948cf3ebb3b1daf69125c7fbf6a71d9a1e610bf7aa5e9da816d10` | true | true | [`0x2a03c6a6…97bc30`](https://optimistic.etherscan.io/tx/0x2a03c6a64cef6a29358b548bc79470aa70062b549387fa6e4fbda4c61997bc30) |
| 4 | base | `0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f` | true | true | [`0x9aa235ef…a0c4cf`](https://basescan.org/tx/0x9aa235ef1ffeacf1f8a2731d208a8a959c452f90be1c84262385d71022a0c4cf) |
| 5 | monad | `0x1b5bc20fa48814cab84395c875a499f18d87038cead861f1e85e30bbfae9fda0` | true | true | `0x36afe04e…9d07ec8` — explorer: verify via RPC (below) |
| 6 | ethereum | `0xe302ad23f94a2fd28973e64785b38bfae2d5b9e019c87d4be91ccbe98e29b0b4` | true | true | [`0xbd7b8fd1…d186d5`](https://etherscan.io/tx/0xbd7b8fd12f97754a032ccf95d79175403db2366a0c2f0ecb993e966584d186d5) |
| 7 | solana | `0xe386f89a4d382746034400e385f19ac6d66ea1db2f6d8be4c569e5d7eeb0626b` | true | **false** | `KKFIRMADEMOc559568b7f7d460c` — **not a Solana signature**; see §4 |
| 8 | polygon | `0xbf805733abd51e074789ff6aacd3d5e8fdff7877808b430b2f78c8f5e6d2deec` | true | true | [`0xcc026f6c…4e768c`](https://polygonscan.com/tx/0xcc026f6c4c4fe0818c610d7c0cd76a2a7e9b1fb6bd077ec28d835e5c704e768c) |
| 9 | avalanche | `0xb80ec7227a9f1afa8aa1502d8817c6a0c52dc6d1b229c39e00f696fbc17a4aef` | true | true | [`0x2dc7f4ae…7e5f228`](https://snowtrace.io/tx/0x2dc7f4aea047ac074cc9d6fcf843066dda3a53a509e91bfb07c143cbe7e5f228) |
| 10 | arbitrum | `0xab6f4f98b4ad8c6eb33e83a7099bcdaa51ecd809cad69e11a275887b114cede1` | true | true | [`0x05d21828…626f1c7`](https://arbiscan.io/tx/0x05d218287eaeb53815e3f6c70989692ccf0b28c241f21c9eec4945bf1626f1c7) |

Two of the transactions were also confirmed against the chain in addition to the explorer (`monadexplorer.com` redirects to `monadvision.com`, which answers 403 to `curl`):

```bash
$ cast receipt 0x9aa235ef1ffeacf1f8a2731d208a8a959c452f90be1c84262385d71022a0c4cf --rpc-url https://mainnet.base.org | grep -E '^(status|blockNumber|from)'
blockNumber          50839205
from                 0x103040545AC5031A11E8C03dd11324C7333a13C7   # the facilitator's EOA: our settle
status               1 (success)

$ cast receipt 0x36afe04eba6bd1e17215c21e0b2307b96c04934d088418032100f755b9d07ec8 --rpc-url https://rpc.monad.xyz | grep -E '^(status|blockNumber)'
blockNumber          101676008
status               1 (success)
```

## 3. Offline receipt verification

`/dx402/receipt/{paymentId}` returns everything needed: the 9-field struct, the signature, the `signer` the facilitator declares and the domain (`name`, `version`, `chainId` of the settlement chain). Nothing was missing; no field had to be invented.

```bash
$ curl -s https://facilitator.ultravioletadao.xyz/dx402/receipt/0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f
{"receipt":{"paymentId":"0x96d9…761f","contentHash":"0xbca4…f93c","pointer":"ipfs+https://facilitator.ultravioletadao.xyz/dx402/blob/0x96d9…761f#bafkreifc22…74qi","payer":"0x09C32b8FC0a94A1EeD424499A42180e29667bEeE","payee":"0x64dbE996E626260F21F5c4FaD3C9bA209978c368","txHash":"0x9aa2…c4cf","network":"base","mode":"direct","anchoredAt":1788467786,"retentionUntil":1796243786},"signature":"0xfffe84ac…6661c","signer":"0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF","domain":{"name":"DX402 Evidence","version":"1","chainId":8453}}
```

The script (`eth_account` 0.13.7). Field order is the one in `src/dx402/receipt.rs:34-44` — it is part of the `typeHash`, so reordering it invalidates every receipt ever issued. `mode` is encoded as `EvidenceMode::as_u8` (`src/dx402/types.rs:47`: `direct`=0, `escrowed`=1). Non-EVM payer and payee go in as the zero address (`receipt.rs:80`).

```python
# verify_receipt.py  --  python3 verify_receipt.py <paymentId>
import json, sys, urllib.request
from eth_account import Account
from eth_account.messages import encode_typed_data

j = json.load(urllib.request.urlopen(
    f"https://facilitator.ultravioletadao.xyz/dx402/receipt/{sys.argv[1]}"))
r, dom = j["receipt"], j["domain"]
types = {"Dx402EvidenceReceipt": [
    {"name": "paymentId",      "type": "bytes32"},
    {"name": "contentHash",    "type": "bytes32"},
    {"name": "pointer",        "type": "string"},
    {"name": "payer",          "type": "address"},
    {"name": "payee",          "type": "address"},
    {"name": "txHash",         "type": "bytes32"},
    {"name": "mode",           "type": "uint8"},
    {"name": "anchoredAt",     "type": "uint64"},
    {"name": "retentionUntil", "type": "uint64"},
]}
MODE = {"direct": 0, "escrowed": 1}
msg = {
    "paymentId":   bytes.fromhex(r["paymentId"][2:]),
    "contentHash": bytes.fromhex(r["contentHash"][2:]),
    "pointer":     r["pointer"],
    "payer":       r["payer"],
    "payee":       r["payee"],
    "txHash":      bytes.fromhex(r["txHash"][2:]),
    "mode":        MODE[r["mode"]],
    "anchoredAt":  r["anchoredAt"],
    "retentionUntil": r["retentionUntil"],
}
signable  = encode_typed_data(domain_data=dom, message_types=types, message_data=msg)
recovered = Account.recover_message(signable, signature=j["signature"])
print("network      :", r["network"], "chainId", dom["chainId"])
print("signer (API) :", j["signer"])
print("recovered    :", recovered)
print("MATCH" if recovered.lower() == j["signer"].lower() else "MISMATCH")
```

Output, on two different chains (two different `chainId`s in the domain):

```
$ python3 verify_receipt.py 0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f
network      : base chainId 8453
signer (API) : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
recovered    : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
MATCH

$ python3 verify_receipt.py 0x7e7ca6a10e0d2a835fd0d8045d0a95a6ee5f5dd29b39696e69c3c21c7a3387ec
network      : avalanche chainId 43114
signer (API) : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
recovered    : 0x7bC4b9cc90a057A95A0F5a8F93C3e3996EE4e0DF
MATCH
```

A third party does not need the facilitator for this: with the receipt JSON and the published address `0x7bC4…4e0DF`, recovery is local.

## 4. What this corpus does NOT prove

- **One operator, one marketplace.** The 116 verified EVM anchors come from the KarmaCadabra fleet operating on Execution Market. The 26 distinct buyers and 24 distinct sellers are wallets of that fleet, not 50 independent parties. There is **no third-party seller** yet that has anchored evidence.
- **The 5 Solana `verified: true` rows are not verified against the chain, and three of their `txHash` values are not Solana signatures** (`KKFIRMADEMO…`, `KKBIDI…`, `KKCUSTODIA…`; the other two are 64-hex strings without `0x`). They are the 5 rows from 2026-08-18, predating the v1.82.0 authority ladder. The current code cannot produce them: `evaluate_gate` returns `UnverifiableChain` for every non-EVM family (`src/dx402/service.rs:441-447`) and `verified = gate_verdict.is_none()` (`:542`), so today a Solana anchor comes out `verified: false` with `notVerifiedReason: dx402_unverifiable_chain`. **Those 5 rows had to be corrected or excluded before quoting the corpus**; this document counts them separately for that reason. With them out, the honest numbers are **116 verified across 7 EVM networks** (see the update at the top: they have since been corrected, and the re-count is 119).
- **Non-EVM in general**: NEAR/Stellar/Algorand/Solana anchors can be `signed` (ed25519 signature by the payee, v1.82.0) but never `verified`; the gate does not read those chains. In this corpus there is no non-EVM `signed` anchor.
- **Phase 2 is not on.** Everything verified was verified in phase 1 (`DX402_REQUIRE_PROOF=false`): the gate runs and reports, it does not reject. Nobody has yet measured that legitimate traffic passes with the gate enforcing.
- **"Sustained for 7 days" is not met.** The range spans 2026-08-18 to 2026-09-03, but 122 of the verified anchors are concentrated on 2026-09-03/04 and the fleet was paused on 2026-09-04 02:09Z.
- What it does prove: that the complete sequence — real settle → provisional anchor → payee signature within the window → verification against the on-chain receipt → EIP-712 receipt recoverable offline — ran 116 times across 7 EVM networks with a stable signer.

## 5. How to reproduce it

```bash
# 0. Facilitator state
curl -s https://facilitator.ultravioletadao.xyz/version
curl -s https://facilitator.ultravioletadao.xyz/supported | jq .extensions      # ["bazaar","durable-evidence"]
curl -s https://facilitator.ultravioletadao.xyz/dx402/stats | jq '{anchored,receiptSigner,note}'

# 1. Corpus (requires AWS credentials with read access to the table)
aws dynamodb scan --table-name facilitator_dx402_evidence --region us-east-2 --output json > scan.json
python3 - <<'EOF2'
import json, collections, datetime
TEST = "0x1111111111111111111111111111111111111111"
rows = []
for it in json.load(open("scan.json"))["Items"]:
    r = json.loads(it["record"]["S"]); rc = r["receipt"]
    if TEST in (rc["payer"], rc["payee"]): continue
    rows.append(dict(verified=it.get("verified", {}).get("BOOL", False), signed=it.get("signed", {}).get("BOOL", False),
                     network=rc["network"], payer=rc["payer"].lower(), payee=rc["payee"].lower(), tx=rc["txHash"], at=rc["anchoredAt"]))
ver = [x for x in rows if x["verified"]]; evm = [x for x in ver if x["network"] != "solana"]
f = lambda t: datetime.datetime.fromtimestamp(t, datetime.UTC).isoformat()
print("anchors", len(rows), "verified", len(ver), "verified_evm", len(evm), "signed", sum(x["signed"] for x in rows))
print("per-network verified", collections.Counter(x["network"] for x in ver).most_common())
print("payers", len({x["payer"] for x in ver}), "payees", len({x["payee"] for x in ver}),
      "| evm-only payers", len({x["payer"] for x in evm}), "payees", len({x["payee"] for x in evm}))
print("first/last verified", f(min(x["at"] for x in ver)), f(max(x["at"] for x in ver)))
print("verified with non-EVM txHash", [x["tx"] for x in ver if not (x["tx"].startswith("0x") and len(x["tx"]) == 66)])
EOF2

# 2. Sample: evidence + receipt per paymentId (no credentials needed)
for id in 0x7e7ca6a10e0d2a835fd0d8045d0a95a6ee5f5dd29b39696e69c3c21c7a3387ec \
          0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f; do
  curl -s https://facilitator.ultravioletadao.xyz/dx402/evidence/$id | jq -c '{verified,signed,receiptSigner,backend,keyAlg}'
  curl -s https://facilitator.ultravioletadao.xyz/dx402/receipt/$id  | jq -c '{network:.receipt.network,txHash:.receipt.txHash,chainId:.domain.chainId}'
done

# 3. Offline receipt (pip install eth-account) -- script in section 3
python3 verify_receipt.py 0x96d9a8fbe441d2fd8abd3628d14644582d8f3b926f2a223edb0a5b75e8f9761f

# 4. The transaction exists and succeeded (foundry `cast`)
cast receipt 0x9aa235ef1ffeacf1f8a2731d208a8a959c452f90be1c84262385d71022a0c4cf --rpc-url https://mainnet.base.org | grep -E '^(status|blockNumber)'
cast receipt 0x36afe04eba6bd1e17215c21e0b2307b96c04934d088418032100f755b9d07ec8 --rpc-url https://rpc.monad.xyz    | grep -E '^(status|blockNumber)'
```
