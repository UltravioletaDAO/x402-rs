# HashKey Chain Network Integration (v2.52.0)

**Date:** 2026-10-29
**Version:** 2.52.0
**Status:** in code; served once the 2.52.0 release is deployed (check `GET /supported`)

HashKey Chain (rebranded **HSK Chain**) is an OP Stack L2 on Ethereum whose gas
token is HSK. The facilitator settles **USDC by EIP-3009** (`exact` scheme) on
mainnet and testnet. Added for the HSK Chain Horizon hackathon (Agent Payments &
A2A Settlement track).

## TL;DR

| | Mainnet | Testnet |
|---|---|---|
| v1 name / CAIP-2 | `hashkey` / `eip155:177` | `hashkey-testnet` / `eip155:133` |
| Payment token | Bridged USDC (`USDC.e`) | USDC (the HSP sandbox token) |
| EIP-712 domain | `"Bridged USDC"` / `"2"` | `"USD Coin"` / `"2"` |
| Schemes | `exact` (EIP-3009). No `upto`, no escrow | same |
| Signatures | EOA. EIP-6492 is refused (no validator on chain) | same |
| Facilitator wallet | `0x103040545AC5031A11E8C03dd11324C7333a13C7` | `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8` |
| Wallet gas (2026-10-29) | **0 HSK — needs funding** | 0 test HSK |

**Until W1 (the mainnet wallet above) holds HSK, `hashkey` reads `down:
signer_gas_critical` on `/health/ready`**, and because it is a mainnet the
endpoint answers 503 overall. Precisely: `/health/ready` values one settle at
0.13 HSK (130,000 gas at the 1000 gwei cap below), calls the signer `down` under
10 settles (**1.3 HSK**) and `degraded` (200) up to 100 (**~13.1 HSK** for `ok`).
5 HSK reads `degraded`, 38 settles. Verify works meanwhile; settle on mainnet
needs the gas. `/health` (the ALB check) is unaffected.

**The two USDC contracts use different EIP-712 names.** A client that signs the
mainnet USDC with `"USD Coin"` (the usual Circle name, and testnet's) produces a
signature that recovers a different address and is refused. The names are
published in `GET /networks.json` (`tokens[].eip712`); `/supported` carries only
token, address and decimals. A seller announcing the mainnet price should set
`extra: {"name": "Bridged USDC", "version": "2"}` in its payment requirements.

## Chain data

| Field | Mainnet | Testnet |
|---|---|---|
| Chain ID | 177 (`0xb1`) | 133 (`0x85`) |
| RPC (keyless, official) | `https://mainnet.hsk.xyz` | `https://testnet.hsk.xyz` |
| Explorer | [hsk.blockscout.com](https://hsk.blockscout.com) | [testnet-explorer.hskchain.net](https://testnet-explorer.hskchain.net) |
| Gas token | HSK (18 decimals) | test HSK |
| Base fee | **500 gwei, held** (all 1025 readings of `eth_feeHistory`) | 1 gwei, held |
| `eth_maxPriorityFeePerGas` | 1 mwei | 1 mwei |
| Block gas limit | 30,000,000 | — |
| Faucet | — | [faucet.hsk.xyz/faucet](https://faucet.hsk.xyz/faucet) (browser + wallet) |
| Bridge | [bridge.hashkeychain.net](https://bridge.hashkeychain.net) | — |
| Docs | [docs.hskchain.net](https://docs.hskchain.net) | — |

CORS is open (`access-control-allow-origin: *`) on both RPCs, so the landing's
browser-side balance fallback can read them.

### Explorers: which URL and why

Measured on 2026-10-29:

- `https://testnet-explorer.hsk.xyz`, which the HashKey documentation still names
  (`Build-on-HashKey-Chain/network-info`), **has no DNS record**: `dns.google`
  answers NOERROR with no A and no AAAA record, and `curl` cannot resolve it.
- ethereum-lists registers `https://testnet-explorer.hskchain.net` for chain 133
  (`_data/chains/eip155-133.json`, EIP-3091). It answers 200, and its API knows
  the testnet USDC (`name` "USD Coin", 6 decimals).
- ethereum-lists registers `https://hsk.blockscout.com` for chain 177
  (`eip155-177.json`). `https://explorer.hsk.xyz` and
  `https://hashkey.blockscout.com` (the URL in the HashKey docs) both redirect to
  it with a 301, paths preserved.

`config/supported_tokens.json` (what `GET /networks.json` publishes and every
explorer link on `/` and `/networks` reads) uses the two that answer directly.

## Token contracts

| | Mainnet | Testnet |
|---|---|---|
| Address | `0x054ed45810DbBAb8B27668922D110669c9D88D0a` | `0x8FE3cB719Ee4410E236Cd6b72ab1fCDC06eF53c6` |
| Source | official HashKey token list | not in the official list (mainnet only); pinned by the HSP sandbox |
| `name()` / `symbol()` | "Bridged USDC" / "USDC.e" | "USD Coin" / "USDC" |
| `version()` | "2" | "2" |
| `decimals()` | 6 | 6 |
| Proxy implementation | `0x82f67bde…225b22de` (FiatToken) | `0x7fbab34b…5c4462e5` |
| `DOMAIN_SEPARATOR()` | `0x78101f50…a507259d` | `0x4b73b8fe…be71d09` |
| Recomputed from name, version, chain id, address | **matches** | **matches** |
| `TRANSFER_WITH_AUTHORIZATION_TYPEHASH()` | `0x7c7c6cdb…1c2a2267` = keccak of the EIP-3009 type | same |
| `transferWithAuthorization` in the implementation's dispatcher | both: `0xe3ee160e` (v,r,s) and `0xcf092995` (bytes, the one the facilitator calls for USDC) | both (implementation of the same size, 23,464 bytes) |
| `totalSupply()` | 1,781.54 USDC (thin) | 4,012,010 USDC |

The full 32-byte domain separators are pinned in
`src/chain/evm.rs::hashkey_tests` (`hashkey_usdc_domain_separators_are_the_ones_the_contracts_publish`):
change the address, the name, the version or the chain id and it goes red. They
are written there without the `0x` prefix, as `hex!` literals.

Not registered, on purpose:

- **USDT** (`0xf1b50ed67a9e2cc94ad3c477779e2d4cbfff9029`): OptimismMintableERC20
  with no EIP-3009 and no permit. `hashkey_accepts_its_own_usdc_and_nothing_else`
  pins that it is refused.
- The testnet's other USDC, "Bridged USDC" at
  `0x18Ec8e93627c893ae61ae0491c1C98769FD4Dfa2`: one USDC per network, and HSP
  pins `0x8FE3…`.

## Contracts the EVM path depends on

`eth_getCode` against the official RPCs, cross-checked with each network's
Blockscout (`is_contract`). The alternative RPCs listed elsewhere
(`hashkeychain-mainnet.alt.technology`, `hk-testnet.rpc.alt.technology`) no
longer resolve.

| Contract | Mainnet | Testnet | Consequence |
|---|---|---|---|
| Multicall3 `0xcA11bde0…76CA11` | 3808 bytes | 3808 bytes | present, though only the 6492 path (refused here) batches through it; an EOA payment is simulated with a direct `eth_call` |
| EIP-6492 validator `0xdAcD51A5…bD2a3B` | **0 bytes** | **0 bytes** | `has_eip6492_validator` answers `false`: a counterfactual (6492) signature is refused with a verdict, never sent to an empty address |
| `upto` Permit2 proxy `0x4020A4f3…240002` | **0 bytes** | **0 bytes** | not in `UPTO_DEPLOYED_NETWORKS`: `upto` is not announced |
| Permit2 `0x00000000…78BA3` | 9152 bytes | 9152 bytes | — |
| Arachnid CREATE2 `0x4e59b448…956C` | 69 bytes | 69 bytes | the 6492 validator and the upto proxy could be replayed later |

## Gas pricing

The mainnet base fee is a floor: 500 gwei on every reading. `eip1559_fee_floor`
gives `Network::HashKey` its own arm (`min_max_fee` 1000 gwei, `fallback_base_fee`
500 gwei, tip 1 mwei). With the generic arm, a fee history that answers without a
base fee priced a settle at 4.001 gwei, which the chain refuses, and a failed read
set no cap at all (alloy's filler priced it). 1000 gwei is twice the floor, the
same cap the normal path already quotes, and leaves room for the 1 mwei tip that
op-geth requires. It is a cap, not a price: a settle pays 500 gwei + 1 mwei per gas.

At ~100,000 gas per `transferWithAuthorization`, a settle costs ~0.05 HSK; a node
reserves gas limit × 1000 gwei (~0.1 HSK) per in-flight settle against the
signer, and `/health/ready` budgets 130,000 gas, 0.13 HSK, per settle.

## Facilitator wallets

| Wallet | Address | Status 2026-10-29 |
|---|---|---|
| EVM mainnet (W1) | `0x103040545AC5031A11E8C03dd11324C7333a13C7` | 0 HSK — at least 1.3 HSK takes `/health/ready` out of 503; ~13.1 HSK reads `ok` |
| EVM testnet (W2) | `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8` | 0 test HSK — faucet |

Addresses from `lambda/balances/handler.py`. Same EVM keys as every other EVM
network: no new secret.

## Production configuration

No secret is added. Both RPCs are keyless, so they go in the task definition's
`environment` block (`terraform/environments/production/main.tf`), which the CI
deploy applies (targeted at the task definition and the service):

| Variable | Value | Where |
|---|---|---|
| `RPC_URL_HASHKEY` | `https://mainnet.hsk.xyz` | `main.tf`, `environment` |
| `RPC_URL_HASHKEY_TESTNET` | `https://testnet.hsk.xyz` | `main.tf`, `environment` |

The balances Lambda (`lambda/balances/handler.py`, `hashkey-mainnet` /
`hashkey-testnet`) is shipped by the CI step that deploys it on a manual run.
The per-chain alarms in `alerts.tf` are a hand-picked set (Scroll, Robinhood,
Unichain, HyperEVM, BSC and SKALE are not in it either) and HashKey is not added.

## Verification (F1, 2026-10-29)

Read-only calls with foundry `cast` 1.4.4; the full script is reproduced below.

```text
== hashkey (mainnet) rpc=https://mainnet.hsk.xyz
cast chain-id -> 177 (expected 177)
block -> 29475689
name()="Bridged USDC"   version()="2"   decimals()=6   symbol()="USDC.e"
DOMAIN_SEPARATOR() on-chain = 0x78101f50…a507259d
DOMAIN_SEPARATOR recomputed(name=Bridged USDC, version=2, chainId=177, verifyingContract=0x054e…8D0a) = 0x78101f50…a507259d
MATCH
TRANSFER_WITH_AUTHORIZATION_TYPEHASH() = keccak256("TransferWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)")
authorizationState(0x0,0x0)=false

== hashkey-testnet rpc=https://testnet.hsk.xyz
cast chain-id -> 133 (expected 133)
block -> 35055934
name()="USD Coin"   version()="2"   decimals()=6   symbol()="USDC"
DOMAIN_SEPARATOR() on-chain = 0x4b73b8fe…be71d09
DOMAIN_SEPARATOR recomputed(name=USD Coin, version=2, chainId=133, verifyingContract=0x8FE3…F53c6) = 0x4b73b8fe…be71d09
MATCH
TRANSFER_WITH_AUTHORIZATION_TYPEHASH() = keccak256 of the same type string
authorizationState(0x0,0x0)=false
```

Re-run it (needs foundry):

```bash
DT=$(cast keccak "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
check() { # rpc token
  local chain name version
  chain=$(cast chain-id --rpc-url "$1")
  name=$(cast call "$2" "name()(string)" --rpc-url "$1" | tr -d '"')
  version=$(cast call "$2" "version()(string)" --rpc-url "$1" | tr -d '"')
  echo "chain=$chain name=$name version=$version decimals=$(cast call "$2" "decimals()(uint8)" --rpc-url "$1")"
  cast call "$2" "DOMAIN_SEPARATOR()(bytes32)" --rpc-url "$1"
  cast keccak "$(cast abi-encode "f(bytes32,bytes32,bytes32,uint256,address)" \
    "$DT" "$(cast keccak "$name")" "$(cast keccak "$version")" "$chain" "$2")"
}
check https://mainnet.hsk.xyz 0x054ed45810DbBAb8B27668922D110669c9D88D0a   # the last two lines must be equal
check https://testnet.hsk.xyz 0x8FE3cB719Ee4410E236Cd6b72ab1fCDC06eF53c6
```

## Testnet end to end (F4)

**Verify: done, against both chains. Settle: pending** (command below). Run on
2026-10-29 with the debug binary of this branch, started locally with only the
two HashKey RPCs and throwaway EVM signers (`ENABLE_WRITER_LEASE=false`; an empty
`config/blacklist.json`, as the Dockerfile creates). Every chain call was an
`eth_call` or a read; nothing was broadcast.

| Request | Result |
|---|---|
| `GET /supported` | `hashkey` / `eip155:177` and `hashkey-testnet` / `eip155:133`, `exact` only, `tokens: [usdc]`, both spellings linked by `networkAliases`; no `upto` |
| `GET /networks.json` | both rows, with `chainId`, `displayName`, icon, the explorers above and the EIP-712 domain of each USDC |
| `GET /hashkey.png` | 200 `image/png` |
| `POST /verify` testnet, domain `"USD Coin"`, value 0 | `{"isValid":true,"payer":…}` |
| `POST /verify` testnet, same body named `eip155:133` | `{"isValid":true,"payer":…}` |
| `POST /verify` testnet, 0.01 USDC from a payer with none | `{"isValid":false,"invalidReason":"insufficient_funds"}` |
| `POST /verify` testnet, signed as `"Bridged USDC"` (mainnet's name) | 400 `contract_call_failed`: the contract itself answered `FiatTokenV2: invalid signature` |
| `POST /verify` mainnet, domain `"Bridged USDC"`, value 0 | `{"isValid":true,"payer":…}` |
| `POST /verify` mainnet, 0.01 USDC from a payer with none | `{"isValid":false,"invalidReason":"insufficient_funds"}` |
| `POST /verify` mainnet, signed as `"USD Coin"` | 400 `contract_call_failed` |
| `GET /health/ready?network=hashkey` (signer with 0 HSK) | 503, `hashkey`: `down`, `signer_gas_critical` |

Value 0 lets a fresh payer with no balance reach the signature check: the
facilitator simulates `transferWithAuthorization` with `eth_call`, so a valid
answer means the contract accepted the domain, the signature and the nonce. The
same check is in the suite as an ignored test, read-only:

```bash
cargo test --locked -p x402-rs --lib --features solana,near,stellar,algorand,sui,xrpl,hedera \
  -- --ignored hashkey_tests::live_verify
```

2026-10-29, 19:3xZ: `1 passed`. `hashkey: verify Valid` and `hashkey-testnet:
verify Valid` for fresh payers; under the other network's name both answered
`execution reverted: FiatTokenV2: invalid signature`.

### Settle on testnet: pending

Two things are missing, neither in this repo: the testnet signer needs test HSK
(the faucet at `faucet.hsk.xyz/faucet` is a browser flow with a wallet), and the
buyer needs testnet USDC `0x8FE3…F53c6`, which the HSP sandbox faucet hands out
to registered teams. Once both exist, with `W2_KEY` the funded testnet signer
and `BUYER_KEY` a buyer holding at least 0.01 USDC (neither ever printed):

```bash
# 1. A local facilitator serving hashkey-testnet only.
[ -f config/blacklist.json ] || echo '[]' > config/blacklist.json
SIGNER_TYPE=private-key EVM_PRIVATE_KEY_TESTNET="$W2_KEY" EVM_PRIVATE_KEY_MAINNET="$W2_KEY" \
RPC_URL_HASHKEY_TESTNET=https://testnet.hsk.xyz ENABLE_WRITER_LEASE=false \
HOST=127.0.0.1 PORT=18402 \
cargo run --release --features solana,near,stellar,algorand,sui,xrpl,hedera &

# 2. The buyer signs a 0.01 USDC EIP-3009 authorization to PAY_TO (domain "USD Coin", "2", 133).
BUYER=$(cast wallet address --private-key "$BUYER_KEY")
PAY_TO=0x2222222222222222222222222222222222222222
NONCE=0x$(openssl rand -hex 32); BEFORE=$(( $(date +%s) + 600 ))
cat > auth.json <<JSON
{"types":{"EIP712Domain":[{"name":"name","type":"string"},{"name":"version","type":"string"},
 {"name":"chainId","type":"uint256"},{"name":"verifyingContract","type":"address"}],
 "TransferWithAuthorization":[{"name":"from","type":"address"},{"name":"to","type":"address"},
 {"name":"value","type":"uint256"},{"name":"validAfter","type":"uint256"},
 {"name":"validBefore","type":"uint256"},{"name":"nonce","type":"bytes32"}]},
 "primaryType":"TransferWithAuthorization",
 "domain":{"name":"USD Coin","version":"2","chainId":133,
  "verifyingContract":"0x8FE3cB719Ee4410E236Cd6b72ab1fCDC06eF53c6"},
 "message":{"from":"$BUYER","to":"$PAY_TO","value":"10000","validAfter":"0",
  "validBefore":"$BEFORE","nonce":"$NONCE"}}
JSON
SIG=$(cast wallet sign --data --from-file auth.json --private-key "$BUYER_KEY")

# 3. The x402 v1 body, then verify and settle.
cat > body.json <<JSON
{"x402Version":1,
 "paymentPayload":{"x402Version":1,"scheme":"exact","network":"hashkey-testnet",
  "payload":{"signature":"$SIG","authorization":{"from":"$BUYER","to":"$PAY_TO",
   "value":"10000","validAfter":"0","validBefore":"$BEFORE","nonce":"$NONCE"}}},
 "paymentRequirements":{"scheme":"exact","network":"hashkey-testnet","maxAmountRequired":"10000",
  "resource":"https://example.com/hashkey-e2e","description":"","mimeType":"application/json",
  "payTo":"$PAY_TO","maxTimeoutSeconds":300,
  "asset":"0x8FE3cB719Ee4410E236Cd6b72ab1fCDC06eF53c6","extra":{"name":"USD Coin","version":"2"}}}
JSON
curl -s -X POST http://127.0.0.1:18402/verify -H 'Content-Type: application/json' --data @body.json
curl -s -X POST http://127.0.0.1:18402/settle -H 'Content-Type: application/json' --data @body.json
# -> {"success":true,"transaction":"0x…","network":"hashkey-testnet",…}. Record it here as a
#    prefix (0x1234…abcd): the pre-commit hook refuses 0x + 64 hex, which a full link carries.
```

## Files modified

| File | Change |
|---|---|
| `src/network.rs` | `Network::HashKey` / `HashKeyTestnet`: enum, `Display`, `FromStr`, `NetworkFamily::Evm`, the four `variants()` copies, `is_testnet`, `to_caip2`, `from_caip2`; `USDC_HASHKEY`, `USDC_HASHKEY_TESTNET`; tests `hashkey_identity_tests` and `variants_copies_tests` |
| `src/chain/evm.rs` | chain ids 177 / 133, EIP-1559 on, `has_eip6492_validator` = false, HashKey mainnet fee floor; tests `hashkey_tests` (domain separators, signature vector, 6492 refusal, fee floor, a read-only live `verify`) |
| `src/chain/solana.rs` | `UnsupportedNetwork` arms |
| `src/from_env.rs` | `RPC_URL_HASHKEY`, `RPC_URL_HASHKEY_TESTNET` |
| `src/handlers.rs` | `GET /hashkey.png` |
| `src/openapi.rs` | network prose (mainnet and testnet) |
| `src/receipts/tests.rs` | `hashkey` / `hashkey-testnet` USDC on the receipt rail |
| `src/upto/types.rs` | test: no upto proxy on HashKey |
| `static/hashkey.png` | 96x96 logo rasterized from the official `hashkeychain-logo.svg` (HashkeyHSK/documentation), visible share 0.75 |
| `static/index.html` | mainnet and testnet cards, CSS (`#AE82F1`), browser balance RPCs |
| `static/networks.html` | browser balance RPCs |
| `static/.well-known/x402` | `hashkey` (USDC) and `hashkey-testnet` |
| `config/supported_tokens.json` | both networks, USDC contracts, summary (45 / 16 / 13) |
| `lambda/balances/handler.py` | `hashkey-mainnet`, `hashkey-testnet` |
| `terraform/environments/production/main.tf` | both RPC URLs in `environment` |
| `scripts/verify_landing_canonical.py` | `--expect-mainnets` 24 |
| `tests/frontend-capabilities.test.cjs` | 45 landing cards |
| `.env.example`, `README.md`, `CLAUDE.md`, `VERSION`, `CHANGELOG.md` | RPCs, tables, counts, 2.52.0 |

## Deployment checklist

- [x] Chain ids, USDC addresses, names, versions and domain separators verified on chain (F1)
- [x] In all four `variants()` copies (`variants_copies_tests`)
- [x] `RPC_URL_HASHKEY` / `RPC_URL_HASHKEY_TESTNET` reach the container via Terraform
- [x] Logo served at `/hashkey.png`; `/networks.json` row with explorer and icon
- [x] Landing cards, balances Lambda, `.well-known/x402`, OpenAPI
- [ ] Fund W1 with HSK (mainnet settles need it; `/health/ready` is 503 under 1.3 HSK, `degraded` under ~13.1)
- [ ] Fund W2 with test HSK (testnet settles need it)
- [ ] Deploy 2.52.0, then: `curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].network] | map(select(startswith("hashkey") or . == "eip155:177" or . == "eip155:133"))'`
- [ ] `python scripts/verify_landing_canonical.py` (24 mainnets)
