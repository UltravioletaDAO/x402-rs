# x402-rs

**Gasless multi-chain payment facilitator with ERC-8004 reputation**

```
 _  _  _  _  ___  ____      ____  ____
( \/ )/ )( \(__ \(  _ \ ___(  _ \/ ___)
 )  ( ) __ ( / _/ )   /(___))   /\___ \
(_/\_)\_)(_/(____)(__\_)   (__\_)(____/
```

[![Live](https://img.shields.io/badge/live-facilitator.ultravioletadao.xyz-00d4aa)](https://facilitator.ultravioletadao.xyz)
[![Version](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Ffacilitator.ultravioletadao.xyz%2Fversion&query=%24.version&label=version&color=blue)](https://facilitator.ultravioletadao.xyz/version)
[![Python SDK](https://img.shields.io/pypi/v/uvd-x402-sdk?label=python%20sdk)](https://pypi.org/project/uvd-x402-sdk/)
[![TypeScript SDK](https://img.shields.io/npm/v/uvd-x402-sdk?label=typescript%20sdk)](https://www.npmjs.com/package/uvd-x402-sdk)
[![Swagger](https://img.shields.io/badge/docs-Swagger_UI-85ea2d)](https://facilitator.ultravioletadao.xyz/docs/)
[![Rust](https://img.shields.io/badge/rust-2021-orange)](https://www.rust-lang.org/)

---

## What is this?

A payment settlement service implementing the [HTTP 402](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/402) protocol. Users sign payment authorizations off-chain, the facilitator submits them on-chain and pays gas fees.

Includes [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) on-chain reputation for AI agents across 23 networks (13 mainnets + 10 testnets).

**No custody. No trust. Just payments.**

---

## Supported Networks

> **Live availability**: Run `python scripts/verify_landing_canonical.py` and inspect `/supported`. Counting identifier strings counts aliases as networks — most networks appear twice, under a v1 name and a CAIP-2 id, and one network can carry several schemes. Drop the CAIP-2 ids before counting:
>
> ```bash
> # mainnets that have a v1 name (22 on 2026-09-17; Hedera has none, so it is not in this count)
> curl -s https://facilitator.ultravioletadao.xyz/supported | jq -r '[.kinds[].network] | unique | .[]' | grep -v ':' | grep -vcE 'testnet|sepolia|devnet|fuji|amoy'
> ```
>
> The two tables below listed exactly what `/supported` served on 2026-09-17: 23 mainnets and 20 testnets, Arc and Hedera included.

### Arc (independently enabled)

| Network | Chain ID / CAIP-2 | Payment token | Explorer |
|---------|------------------|---------------|----------|
| Arc mainnet | 5042 / `eip155:5042` | USDC/EURC, direct `exact`, EOA | [explorer.arc.io](https://explorer.arc.io) |
| Arc testnet | 5042002 / `eip155:5042002` | USDC/EURC, direct `exact`, EOA | [explorer.testnet.arc.io](https://explorer.testnet.arc.io) |

Arc has separate RPC and deployment switches. A running instance serves a network
only when it appears in [`/supported`](https://facilitator.ultravioletadao.xyz/supported).
Integrators start at [Arc: getting paid in USDC and EURC](docs/networks/arc.md); operators want
[Arc operations, canaries and activation](docs/networks/arc-operations.md).
The Python and TypeScript SDKs (`uvd-x402-sdk`; current versions on the badges above)
include both Arc networks. USDC is also the gas token: native and ERC-20 amounts are
two precisions of the same balance.
Integrators start at [Hedera: getting paid in native USDC](docs/networks/hedera.md);
operators want [native Hedera operations and public receipts](docs/guides/hedera-native.md).
Both network tables include Arc and native Hedera; runtime availability remains `/supported`.

### Arc and Hedera payment identifiers

| Network | Payment identifier | Asset | Facilitator fee payer |
| --- | --- | --- | --- |
| Arc mainnet | `arc` / `eip155:5042` | USDC/EURC (6 decimals) | `0x103040545AC5031A11E8C03dd11324C7333a13C7` |
| Arc testnet | `arc-testnet` / `eip155:5042002` | USDC/EURC (6 decimals) | `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8` |
| Hedera mainnet | `hedera:mainnet` | USDC `0.0.456858` (6) | `0.0.10868300` |
| Hedera testnet | `hedera:testnet` | USDC `0.0.429274` (6) | `0.0.10576385` |

Discover availability and the current network-specific `extra.feePayer` from
`/supported`. These are facilitator accounts, not merchant destinations. Set
`payTo` to the seller's own account. Arc supports direct EOA `exact` payments in
x402 v1/v2; its ERC-20 USDC address is `0x3600000000000000000000000000000000000000`,
with EIP-712 domain `USDC` / `2`. Its 18-decimal gas view is the same balance;
payment amounts use 6 decimals.

Hedera supports native `CryptoTransfer`, `exact`, **x402 v2 only**. It uses numeric
accounts and native token IDs, not EVM chain IDs 295/296. Buyer and recipient must
be associated with USDC. The sponsor pays HBAR fees without contributing payment
principal. HBAR is used only for network fees; payments accept native USDC only. Neither addition enables
escrow, `upto` or Gateway on that network; ERC-8004 is served on both Arc networks,
not on Hedera. Native Hedera also rejects
durable-evidence and other unsupported extensions.

### Mainnets

| Network | Chain ID | Token | Explorer |
|---------|----------|-------|----------|
| **Arc** | 5042 | USDC, EURC | [explorer.arc.io](https://explorer.arc.io) |
| **Hedera** | `hedera:mainnet` (native) | USDC | [hashscan.io/mainnet](https://hashscan.io/mainnet) |
| **Ethereum** | 1 | USDC | [etherscan.io](https://etherscan.io) |
| **Base** | 8453 | USDC | [basescan.org](https://basescan.org) |
| **Arbitrum** | 42161 | USDC | [arbiscan.io](https://arbiscan.io) |
| **Optimism** | 10 | USDC | [optimistic.etherscan.io](https://optimistic.etherscan.io) |
| **Polygon** | 137 | USDC | [polygonscan.com](https://polygonscan.com) |
| **Avalanche** | 43114 | USDC | [snowtrace.io](https://snowtrace.io) |
| **Celo** | 42220 | USDC, USDT | [celoscan.io](https://celoscan.io) |
| **HyperEVM** | 999 | USDC | [hyperliquid.xyz](https://hyperliquid.xyz) |
| **Unichain** | 130 | USDC | [uniscan.xyz](https://uniscan.xyz) |
| **Monad** | 143 | USDC, AUSD, USDT | [monad.xyz](https://monad.xyz) |
| **BSC** | 56 | AUSD | [bscscan.com](https://bscscan.com) |
| **SKALE Base** | 1187947933 | USDC.e | [skale-base-explorer](https://skale-base-explorer.skalenodes.com) |
| **Scroll** | 534352 | USDC | [scrollscan.com](https://scrollscan.com) |
| **Robinhood Chain** | 4663 | USDG | [robinhoodchain.blockscout.com](https://robinhoodchain.blockscout.com) |
| **Sui** | - | USDC | [suiscan.xyz](https://suiscan.xyz) |
| **Solana** | - | USDC, AUSD, PYUSD | [solscan.io](https://solscan.io) |
| **Fogo** | - | USDC | [fogoscan.com](https://fogoscan.com) |
| **NEAR** | - | USDC | [nearblocks.io](https://nearblocks.io) |
| **Stellar** | - | USDC | [stellarchain.io](https://stellarchain.io) |
| **Algorand** | - | USDC | [allo.info](https://allo.info) |
| **XRPL** | - | XRP, USDC, RLUSD | [livenet.xrpl.org](https://livenet.xrpl.org) |

### Testnets

| Network | Chain ID | Faucet |
|---------|----------|--------|
| Arc Testnet | 5042002 | [faucet.circle.com](https://faucet.circle.com) |
| Hedera Testnet | `hedera:testnet` (native) | [faucet.circle.com](https://faucet.circle.com) (USDC) |
| Ethereum Sepolia | 11155111 | [faucet.circle.com](https://faucet.circle.com) |
| Base Sepolia | 84532 | [faucet.circle.com](https://faucet.circle.com) |
| Arbitrum Sepolia | 421614 | [faucet.circle.com](https://faucet.circle.com) |
| Optimism Sepolia | 11155420 | [faucet.circle.com](https://faucet.circle.com) |
| Polygon Amoy | 80002 | [faucet.polygon.technology](https://faucet.polygon.technology) |
| Avalanche Fuji | 43113 | [faucet.avax.network](https://faucet.avax.network) |
| Celo Sepolia | 11142220 (`celo-sepolia`) | [faucet.celo.org](https://faucet.celo.org) |
| HyperEVM Testnet | 998 (`hyperevm-testnet`) | - |
| Unichain Sepolia | 1301 | - |
| SKALE Base Sepolia | 324705682 | [base-sepolia-faucet.skale.space](http://base-sepolia-faucet.skale.space) |
| Solana Devnet | - | [solfaucet.com](https://solfaucet.com) |
| Fogo Testnet | - | [fogoscan.com](https://fogoscan.com/?cluster=testnet) |
| NEAR Testnet | - | [near-faucet.io](https://near-faucet.io) |
| Stellar Testnet | - | [friendbot](https://friendbot.stellar.org) |
| Algorand Testnet | - | [dispenser.testnet.aws.algodev.network](https://dispenser.testnet.aws.algodev.network) |
| Sui Testnet | - | [suifaucet.com](https://suifaucet.com) |
| XRPL Testnet | `xrpl-testnet` / `xrpl:1` | - |
| Robinhood Chain Testnet | 46630 | [faucet.testnet.chain.robinhood.com](https://faucet.testnet.chain.robinhood.com) |

### Supported Stablecoins

> **Note**: Use `GET /supported` for payment capabilities on the running facilitator. `python scripts/stablecoin_matrix.py` inventories registered deployments, including assets that a payment provider cannot process.

| Token | Networks |
|-------|----------|
| **USDC** | All payment networks except Robinhood Chain and BSC (BSC USDC lacks ERC-3009) |
| **AUSD** | Ethereum, Polygon, Arbitrum, Avalanche, Monad, BSC, Solana |
| **EURC** | Ethereum, Base, Avalanche, Arc mainnet/testnet |
| **USDT** | Arbitrum, Celo, Optimism, Monad |
| **PYUSD** | Ethereum, Solana (Token-2022) |
| **USDG** | Robinhood Chain (Paxos Global Dollar, EIP-712 domain "Global Dollar" v1) |
| **RLUSD** | XRPL |
| **XRP** | XRPL (native) |

**Exact payment stablecoins:**

| Network | USDC | AUSD | EURC | USDT | PYUSD | USDG | RLUSD |
|---------|:----:|:----:|:----:|:----:|:-----:|:----:|:-----:|
| Ethereum | Y | Y | Y | - | Y | - | - |
| Base | Y | - | Y | - | - | - | - |
| Arbitrum | Y | Y | - | Y | - | - | - |
| Optimism | Y | - | - | Y | - | - | - |
| Polygon | Y | Y | - | - | - | - | - |
| Avalanche | Y | Y | Y | - | - | - | - |
| Celo | Y | - | - | Y | - | - | - |
| BSC | - | Y | - | - | - | - | - |
| Monad | Y | Y | - | Y | - | - | - |
| HyperEVM | Y | - | - | - | - | - | - |
| Unichain | Y | - | - | - | - | - | - |
| Scroll | Y | - | - | - | - | - | - |
| Robinhood Chain | - | - | - | - | - | Y | - |
| Arc (mainnet + testnet) | Y | - | Y | - | - | - | - |
| Hedera (mainnet + testnet) | Y | - | - | - | - | - | - |
| SKALE Base | Y | - | - | - | - | - | - |
| Solana | Y | Y | - | - | Y | - | - |
| Sui | Y | - | - | - | - | - | - |
| Fogo | Y | - | - | - | - | - | - |
| NEAR | Y | - | - | - | - | - | - |
| Stellar | Y | - | - | - | - | - | - |
| Algorand | Y | - | - | - | - | - | - |
| XRPL | Y | - | - | - | - | - | Y |

> **Arc and Hedera note**: both are served on mainnet *and* testnet as of
> 2026-09-17 03:08Z. Each network still has its own independent activation
> switch, so a self-hosted instance may serve neither — `/supported` is what
> decides. Hedera accepts native USDC; HBAR pays sponsor network fees only.
>
> **XRPL note**: In addition to USDC (issued token), XRPL also supports **RLUSD** (issued token) and **native XRP**. These are not EIP-3009 tokens, so they are not tracked by `scripts/stablecoin_matrix.py` (which only enumerates EIP-3009 stablecoins). See `docs/plans/xrpl-native-x402-integration-plan.md`.

---

## Quick Start

```bash
# Clone
git clone https://github.com/UltravioletaDAO/x402-rs.git
cd x402-rs

# Configure
cp .env.example .env
# Add your private keys (use testnet keys for development)

# Run
cargo run --release --features solana,near,stellar,algorand

# Test
curl http://localhost:8080/health
curl http://localhost:8080/supported | jq '.kinds | length'
# Output is live; aliases and multiple schemes are not distinct networks.
```

### Docker

```bash
docker-compose up -d
curl http://localhost:8080/
```

---

## API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Landing page |
| `/health` | GET | Health check |
| `/version` | GET | Current version |
| `/supported` | GET | List all networks |
| `/verify` | POST | Verify payment authorization |
| `/settle` | POST | Submit payment on-chain (supports escrow with `refund` extension) |
| `/blacklist` | GET | OFAC sanctioned addresses |
| `/feedback` | GET/POST | ERC-8004 reputation (query/submit) |
| `/identity/:network/:agentId` | GET | Agent identity lookup |
| `/reputation/:network/:agentId` | GET | Agent reputation summary |
| `/discovery/resources` | GET | Browse the curated Bazaar (filter by `q`, `network`, `tier`, `health`, …) |
| `/discovery/stats` | GET | Bazaar totals by source, network, tier and health |
| `/discovery/register` | POST | Register a paid endpoint |
| `/bazaar` | GET | Visual Bazaar explorer (HTML) |
| `/events` | GET | Live traffic stream (SSE), one message per verify/settle |
| `/events/live` | GET | Watch the stream in a browser (HTML) |
| `/stats` | GET | Aggregated metrics (HTML) |
| `/api/stats` | GET | Totals per network and asset (JSON) |
| `/transactions` | GET | Recent recorded operations (JSON) |
| `/docs` | GET | Interactive Swagger UI |

### Live traffic and metrics

Watch payments as they settle:

```bash
curl -N https://facilitator.ultravioletadao.xyz/events
```

Or open **[/events/live](https://facilitator.ultravioletadao.xyz/events/live)** in a
browser. Each message names the network, the endpoint being bought, the seller,
the amount and — on a settle — the transaction hash.

Totals live at **[/stats](https://facilitator.ultravioletadao.xyz/stats)**
(`/api/stats` for JSON).

> **Neither of these is a ledger — the chain is.** The stream is *lossy by
> design*: it will never slow down or fail a payment to keep an observer in
> sync, so an event you were not connected for does not exist anywhere, and
> **absence is not evidence that nothing happened**. The stored records are
> written best-effort *after* settlement, so an outage loses rows without
> affecting payments. Verify anything that matters against the transaction hash.
>
> By default, operations that **error** are published nowhere, so a 100% success
> rate means "no failures were recorded" rather than "no failures occurred".
> Operators can turn that on with `X402_EVENTS_PUBLISH_FAILURES=true`.

### Example: Check supported networks

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network'
```

### Example: Settle a payment

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Content-Type: application/json" \
  -d '{"payload": "...", "network": "base"}'
```

---

## The Curated Bazaar

The facilitator runs a **meta-bazaar**: it aggregates discoverable x402 resources
from a dozen external facilitators (Coinbase CDP, PayAI, Thirdweb, QuestFlow,
AnySpend, Heurist, Polymer, Meridian, …) into one catalog at
`GET /discovery/resources`, with a visual explorer at
[`/bazaar`](https://facilitator.ultravioletadao.xyz/bazaar).

Aggregating a firehose means inheriting its junk, so the catalog is **curated**
rather than merely mirrored:

**1. Ingestion filter.** Nothing enters the registry unless it is actually
payable. Rejected at import: non-`http(s)` schemes (the ecosystem is full of
`monopoly://`-style entries), private/metadata/encoded-IP hosts, URLs carrying
userinfo, resources whose `accepts` declares no network or a zero amount, and
oversized fields. Spec-legal `routeTemplate` URLs (`/users/:id`) are kept but
flagged unprobeable. A retention GC applies the same rules to already-stored
entries — the first pass removed ~5,000 junk listings.

**2. Liveness probing ("pre-ping").** A background prober checks every listed
URL with an SSRF-hardened connector and never attaches payment. For an x402
resource **HTTP 402 is the healthy signal**; `401/403/405` is auth-gated (healthy
by design), `404/410`/dead/5xx counts toward quarantine. MCP endpoints are probed
with a JSON-RPC `initialize` handshake instead. A resource is quarantined after 3
consecutive failures (backoff 1h → 6h → 24h → 72h) and recovers automatically
after 2 consecutive successes. Quarantined resources are hidden from the default
listing but retained — pass `?health=any` to see everything.

**3. Curated tiers.** Listings are ordered `first_party` > `vip` > `verified`
(probe-confirmed 402) > `listed`, then by liveness, then recency. Tiers come from
`config/bazaar_curation.json` and are matched host-exact with a path boundary on
the parsed URL, so a lookalike host can never inherit a curated tier.

**4. On-chain verification (ERC-8004).** Curated entries can carry a
`curation.verification` object resolved from the ERC-8004 registries, so a
consumer can check an agent's identity and reputation on-chain instead of
trusting this API. Writing probe-derived uptime attestations on-chain is
implemented behind `ENABLE_BAZAAR_ATTESTATIONS` (default off).

**5. Security.** The prober resolves DNS and refuses the request if *any*
resolved address is private/metadata (a mixed answer is treated as an attack),
pins the socket to the checked address, follows redirects manually re-checking
each hop, and restricts ports. If a live 402 ever advertises a `payTo` the
listing never declared, the resource is quarantined immediately and a
`paytoswap` alarm is logged — that is a hijack signal, not a health signal.

```bash
# Search the whole catalog
curl -s '.../discovery/resources?q=weather&limit=5' | jq '.items[].url'

# Only endpoints confirmed to answer 402 right now
curl -s '.../discovery/resources?health=alive&limit=5' | jq '.items[].url'

# Catalog composition
curl -s '.../discovery/stats' | jq '{total, visible, byTier, byHealth}'
```

Design docs: [`docs/plans/bazaar/`](docs/plans/bazaar/).

---

## Chain-Specific Features

### EVM Chains (EIP-3009)
Standard `transferWithAuthorization` for gasless USDC transfers.

### Solana (SPL Token + Token2022)
Supports both SPL Token (USDC) and Token2022 (AUSD, PYUSD) programs. Both
Token-2022 mints carry a `transferFeeConfig` extension that is 0 today; the
facilitator reads the mint on every verify and settle and refuses the payment if
a fee is in force or scheduled, since the payee would then receive less than the
signed amount.

### NEAR (NEP-366)
Meta-transactions with delegate actions for gasless payments.

### Stellar (Soroban)
Smart contract-based authorization on Stellar's Soroban VM.

### Algorand (Atomic Groups)
Fee pooling via atomic transaction groups. Facilitator signs transaction 0 (fee tx), user signs transaction 1 (payment tx). Based on [GoPlausible x402-avm spec](https://github.com/GoPlausible/x402-avm).

---

## DX402 — Durable Evidence (`durable-evidence` extension)

x402 settles payment on-chain **permanently**, then delivers the purchased
resource **exactly once**, in the body of a `200 OK`, and keeps nothing. If the
buyer did not capture it at that instant, it is gone — and neither party can
later prove *what* was delivered, only *that* payment happened.

DX402 seals a copy of the response to the payer's own public key and anchors it.

**The idea:** a payment authorization is a digital signature, and a signature
yields the signer's **public key**, not merely their address. So the seller can
encrypt to the buyer using key material the payment already produced.

> **Paying is publishing your encryption key.**

No registration, no key exchange, no extra round trip.

| Property | What it means |
|---|---|
| **Durable** | the delivered body survives the session |
| **Private** | encrypted to the payer — not the facilitator, not the storage backend, not us |
| **Coupled** | derived from the payment itself |

Payer-key availability for the network families with durable-evidence support (native Hedera rejects this extension):

| Family | Curve | Source |
|---|---|---|
| EVM | secp256k1 | ECDSA recovery over the EIP-712 digest |
| Solana / Fogo | ed25519 | the address **is** the key |
| NEAR | ed25519 | access key (`ed25519:…`) |
| Stellar | ed25519 | the `G…` address is the encoded key |
| Algorand | ed25519 | address is key + checksum |
| Sui | either | the signature carries the key |
| XRPL | either | `SigningPubKey` of the signed transaction |

### Seller integration

```rust
use x402_axum::durable::{DurableConfig, DurableEvidenceHook, HttpPutSink};

let hook = DurableEvidenceHook::new(
    DurableConfig::from_env(),   // 32 MiB per body, 160 MiB across concurrent captures
    Arc::new(HttpPutSink::new("https://evidence.example.com")),
    "https://facilitator.ultravioletadao.xyz",
);
let hook = hook.with_anchor_signer(seller_key);   // without this every anchor stays provisional
let layer = x402
    .with_price_tag(usdc.amount(0.010)?)                              // plain
    .with_durable_offer(usdc.amount(0.012)?, DurableEvidenceConfig::default()) // + evidence, buyer's choice
    .with_durable_evidence(hook);
```

The buyer opts in from the client side with `X402Payments::with_wallet(w).prefer_durable_evidence()`;
a client that never asks keeps paying for the plain offer, whatever order the seller listed them in.

Every anchor is judged against the chain (`verified` / `signed` on the record), and a payment
released from an x402r escrow carries `escrowRelease` so the facilitator can resolve the real
buyer through the escrow's own `getHash`. See docs/DX402.md — "The anchor gate", "Letting the
buyer choose", "Payments released from an x402r escrow".

**DX402 can never fail a payment.** An oversized body, a full memory budget, an
unreachable sink, a smart-contract wallet with no recoverable key, or a buyer who
paid for the plain offer (`not_selected`) all downgrade to a skip notice in the `X-Durable-Evidence` header; the response is
delivered exactly as before.

`DX402_MAX_BODY_BYTES` (default 32 MiB) is the largest body that gets evidence,
and `DX402_MAX_INFLIGHT_BYTES` (default 160 MiB) bounds the memory all concurrent
captures may hold. They are one setting in two halves: sealing buffers the
plaintext and the ciphertext together, so a generous body limit with unbounded
concurrency is an OOM, and an OOM drops responses that were already paid for.
The budget is not memory taken, only memory refused — a 4 KB response reserves a
few KB whatever the ceiling.

Neither is a storage ceiling, and in pointer mode neither touches the
facilitator's storage: the object goes to the seller's own sink. For objects
genuinely too large to hold in memory the answer is streaming, which is not
implemented yet (`docs/plans/dx402/04-STREAMING-EVIDENCE-HANDOFF.md`).

### Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/dx402/anchor` | a resource server reports an anchor (metadata only) |
| `GET` | `/dx402/evidence/{paymentId}` | pointer, content hash, mode, receipt |
| `GET` | `/dx402/receipt/{paymentId}` | signed receipt, verifiable offline |
| `GET` | `/dx402/blob/{paymentId}` | the ciphertext, when the facilitator hosts it (private bucket) |
| `GET` | `/dx402/stats` | anchor count **and the backends this deployment actually offers** |
| `POST` | `/dx402/repair/{paymentId}` | rewrite a pointer that names nothing — **admin only** (`DX402_ADMIN_TOKEN`, 404 without it) |
| `POST` | `/dx402/recover` | `escrowed` mode — **501**, out of scope (spec v0.2 §17) |

Present only when `ENABLE_DX402=true`. Full documentation: **[docs/DX402.md](docs/DX402.md)**;
normative spec: **[docs/plans/dx402/08-SPEC-v0.2.md](docs/plans/dx402/08-SPEC-v0.2.md)** (v0.1 is kept for history).

---

## ERC-8004 Trustless Agents (On-Chain Reputation)

The facilitator integrates [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) for AI agent identity and reputation across 23 networks.

### What is ERC-8004?

Three on-chain registries enabling trust in the agentic economy:

- **Identity Registry** - ERC-721 based agent handles with verifiable metadata
- **Reputation Registry** - Standardized feedback posting with proof-of-payment
- **Validation Registry** - Third-party attestation of agent capabilities

### Supported ERC-8004 Networks (23)

Addresses come from the canonical [erc-8004 reference deployment](https://github.com/erc-8004/erc-8004-contracts)
and are identical on every EVM chain (CREATE2) -- there is no chain-specific fork:

| Registry | Mainnet | Testnet |
|----------|---------|---------|
| Identity | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` | `0x8004A818BFB912233c491871b3d84c89A494BD9e` |
| Reputation | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| Validation | `0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58` (not on SKALE Base) | `0x8004Cb1BF31DAf7788923b405b754f57acEB4272` |

| Network | Type | Identity Registry | Reputation Registry |
|---------|------|-------------------|---------------------|
| Ethereum | Mainnet | `0x8004A169...9a432` | `0x8004BAa1...dE9b63` |
| Base | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Polygon | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Arbitrum | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Optimism | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Celo | Mainnet | Same (CREATE2) | Same (CREATE2) |
| BSC | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Monad | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Avalanche | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Scroll | Mainnet | Same (CREATE2) | Same (CREATE2) |
| SKALE Base | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Arc | Mainnet | Same (CREATE2) | Same (CREATE2) |
| Solana | Mainnet | Anchor program | Anchor program |
| Ethereum Sepolia | Testnet | `0x8004A818...4BD9e` | `0x8004B663...8713` |
| Base Sepolia | Testnet | Same | Same |
| Polygon Amoy | Testnet | Same | Same |
| Arbitrum Sepolia | Testnet | Same | Same |
| Optimism Sepolia | Testnet | Same | Same |
| Celo Sepolia | Testnet | Same | Same |
| Avalanche Fuji | Testnet | Same | Same |
| SKALE Base Sepolia | Testnet | Same | Same |
| Arc Testnet | Testnet | Same | Same |
| Solana Devnet | Testnet | Anchor program | Anchor program |

All EVM mainnet contracts use CREATE2 deterministic deployment (same addresses on every chain). Solana uses a dedicated Anchor program.

### ERC-8004 API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/feedback` | GET | List ERC-8004 supported networks |
| `/feedback` | POST | Submit on-chain reputation feedback |
| `/identity/:network/:agentId` | GET | Query agent identity |
| `/reputation/:network/:agentId` | GET | Query reputation summary |

### Example: Submit Feedback

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/feedback \
  -H "Content-Type: application/json" \
  -d '{
    "network": "base-mainnet",
    "agentId": "0x...",
    "rating": 5,
    "tags": ["uptime", "quality"],
    "proofOfPayment": "0x..."
  }'
```

---

## x402r Escrow Extension (Trustless Refunds)

The facilitator supports the [x402r extension](https://github.com/coinbase/x402/issues/864) for trustless refunds via escrow contracts.

### Architecture Overview

```
                           STANDARD PAYMENT FLOW
  ┌──────────┐    ┌───────────────┐    ┌────────────┐    ┌──────────┐
  │  Buyer   │───>│  Facilitator  │───>│   USDC     │───>│ Merchant │
  │ (signs)  │    │  (pays gas)   │    │ (on-chain) │    │(receives)│
  └──────────┘    └───────────────┘    └────────────┘    └──────────┘
       │                 │
       │  EIP-3009       │  transferWithAuthorization()
       │  signature      │

                           ESCROW PAYMENT FLOW (x402r)
  ┌──────────┐    ┌───────────────┐    ┌──────────────┐    ┌──────────┐
  │  Buyer   │───>│  Facilitator  │───>│ DepositRelay │───>│  Escrow  │
  │ (signs)  │    │  (pays gas)   │    │   (proxy)    │    │ (holds)  │
  └──────────┘    └───────────────┘    └──────────────┘    └──────────┘
       │                 │                    │                  │
       │  EIP-3009       │  executeDeposit()  │  deposit()       │
       │  to PROXY       │                    │                  │
       │                 │                    │                  ▼
       │                 │                    │            ┌──────────┐
       │                 │                    │            │ Merchant │
       │                 │                    └───────────>│(after    │
       │                 │                      release()  │ window)  │
       │                 │                                 └──────────┘
       │                 │
       └─────────────────┴───── Buyer can request refund within window
```

### Supported Networks

| Network | Chain ID | Factory | Escrow | Status |
|---------|----------|---------|--------|--------|
| Base | 8453 | `0x41Cc...A814` | `0xC409...f6bC` | Production |
| Base Sepolia | 84532 | `0xf981...BaC2` | `0xF7F2...0E58` | Testnet |

### Documentation

- **Technical Deep-Dive:** [`docs/X402R_ESCROW.md`](docs/X402R_ESCROW.md)
- **Testing Guide:** [`docs/X402R_ESCROW_TESTING.md`](docs/X402R_ESCROW_TESTING.md)
- **x402r Proposal:** https://github.com/coinbase/x402/issues/864
- **x402r Contracts:** https://github.com/BackTrackCo/x402r-contracts

---

## Configuration

```bash
# Wallet keys (leave empty for AWS Secrets Manager)
EVM_PRIVATE_KEY_MAINNET=
EVM_PRIVATE_KEY_TESTNET=
SOLANA_PRIVATE_KEY_MAINNET=
SOLANA_PRIVATE_KEY_TESTNET=
NEAR_PRIVATE_KEY_MAINNET=
NEAR_ACCOUNT_ID_MAINNET=
STELLAR_SECRET_KEY_MAINNET=
STELLAR_SECRET_KEY_TESTNET=
ALGORAND_MNEMONIC_MAINNET=
ALGORAND_MNEMONIC_TESTNET=

# RPC URLs (premium recommended for production)
RPC_URL_BASE=https://mainnet.base.org
RPC_URL_NEAR_MAINNET=https://rpc.mainnet.near.org
RPC_URL_ALGORAND_MAINNET=https://mainnet-api.algonode.cloud
# ... see .env.example for all networks
```

---

## Architecture

```
┌─────────────┐     ┌─────────────────┐     ┌──────────────┐
│   Client    │────▶│   Facilitator   │────▶│  Blockchain  │
│  (signs)    │     │  (pays gas)     │     │  (settles)   │
└─────────────┘     └─────────────────┘     └──────────────┘
```

**Payment Flow:**
1. User signs EIP-3009 (EVM), NEP-366 (NEAR), or atomic group (Algorand)
2. User sends signed payload to facilitator
3. Facilitator verifies signature and submits on-chain
4. Facilitator pays gas, user pays nothing

---

## Deployment

### AWS ECS (Production)

```bash
# Build & push
docker build -t facilitator:v1.46.0 .
docker push <AWS_ACCOUNT_ID>.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.46.0

# Deploy
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment
```

**Infrastructure:** Terraform configs in `terraform/environments/production/`

**Cost:** ~$45/month (Fargate 1vCPU/2GB + ALB)

---

## Development

```bash
# Format
cargo fmt

# Lint
cargo clippy --features solana,near,stellar,algorand

# Test
cd tests/integration && python test_facilitator.py
```

### Claude Code Skills

This project includes Claude Code skills for automated development workflows:

| Skill | Command | Description |
|-------|---------|-------------|
| **add-network** | `/add-network scroll` | Add new blockchain network with automated research, EIP-3009 verification, and deployment |
| **add-erc8004-network** | `/add-erc8004-network polygon` | Add ERC-8004 reputation support to a network |
| **stablecoin-addition** | `/stablecoin-addition` | Add new EIP-3009 compatible stablecoins (USDT, EURC, AUSD, etc.) |
| **ship** | `/ship` | Full automated deployment: commit → build → ECR push → ECS deploy → verify |
| **deploy-prod** | `/deploy-prod` | Build and deploy Docker image to production ECS |
| **test-prod** | `/test-prod` | Test production facilitator endpoints |

**Example: Add a new network**
```
> add facilitator scroll

Claude will:
1. Research chain IDs, RPCs, USDC contracts
2. Verify EIP-3009 support
3. Check wallet balances and logo
4. Request any missing prerequisites
5. Implement all code changes
6. Deploy if all prerequisites met
```

### Adding New Networks

See [`guides/ADDING_NEW_CHAINS.md`](guides/ADDING_NEW_CHAINS.md) for the complete manual checklist.

**Quick automated path:** Use `/add-network {network-name}` skill.

### Version & Docs Maintenance Checklist

When bumping the version, adding endpoints, or adding networks, update **all** of these:

| File | What to update |
|------|---------------|
| `VERSION` | the release version — never `Cargo.toml`, which holds a frozen `0.0.0` placeholder |
| `src/openapi.rs` | endpoint docs, network lists (the version is patched at runtime from `VERSION`) |
| `README.md` | Version badge, network tables, API endpoint table, ERC-8004 network count |
| `docs/networks/<network>.md` | The integrator's usage guide: identifiers, asset and decimals, what the buyer signs, what does not work. Same sections as [`docs/networks/arc.md`](docs/networks/arc.md). Operator runbooks go beside it as `<network>-operations.md` or under `docs/guides/` |
| `static/index.html` | Network cards, stats, ERC-8004 showcase badges, i18n strings (EN/ES) |
| `docs/CHANGELOG.md` | New version entry |
| `src/erc8004/mod.rs` | `supported_networks()` when adding ERC-8004 networks |

**Swagger UI**: https://facilitator.ultravioletadao.xyz/docs/ (auto-generated from `src/openapi.rs`)

---

## Acknowledgments

Special thanks to:
- **[GoPlausible](https://github.com/GoPlausible)** - For the [x402-avm specification](https://github.com/GoPlausible/x402-avm) and documentation that made Algorand integration possible
- **[x402-rs](https://github.com/x402-rs/x402-rs)** - The upstream project this facilitator is forked from

---

## Links

- **Live:** https://facilitator.ultravioletadao.xyz
- **Upstream:** https://github.com/x402-rs/x402-rs
- **x402 Protocol:** https://www.x402.org

---

## License

Apache 2.0

---

**Built by [Ultravioleta DAO](https://ultravioletadao.xyz)**

## Native Hedera

Native Hedera `exact` payments use x402 v2 and native USDC only. HBAR is reserved for sponsor network fees. Networks are enabled independently and reported by `/supported`; mainnet requires its own funded signer and release canaries. Hedera is **v2-only** and has no v1 network name, accounts are numeric entity ids (`0.0.1234`, never a `0x` address or an alias), and the EVM chain ids 295/296 are not this payment rail.

Integrators start at [Hedera: getting paid in native USDC](docs/networks/hedera.md); operators want [configuration, merchant requirements, recovery and validation status](docs/guides/hedera-native.md). The `extra.feePayer` differs per network — read it from `/supported`, never carry one across.

## Portable facilitator receipts

Arc mainnet/testnet exact USDC/EURC and native Hedera USDC v2 support additive
facilitator receipts: network, asset, atomic amount, payTo, request hash,
settlement ID, status and refusal reason. Persist purchase context before sending
the authorization and reuse it after uncertainty. Payment confirmation does not
prove merchant delivery. See [the receipt guide](docs/facilitator-receipts.md).
Live EURC acceptance was proven on Arc mainnet on 2026-09-22 (tx `0xd9de3864e11698cf730664147ac383acb763279056ac091bab57cfd3bf536128`); Arc testnet is still pending.
