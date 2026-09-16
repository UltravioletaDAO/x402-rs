---
name: add-network
description: Add new blockchain networks to the x402-rs facilitator. This skill should be used when adding support for a new EVM or Solana network (e.g., "add facilitator scroll", "add network monad"). It performs automated research, gathers USDC contract info, verifies EIP-3009 support, checks wallet balances, and guides through implementation. If all prerequisites are met (logo exists, wallets funded), it can deploy automatically.
---

# Add Network Skill

This skill provides a complete automated workflow for adding new blockchain networks to the x402-rs payment facilitator.

## When to Use This Skill

Invoke this skill when:
- Adding a new EVM chain (Scroll, Monad, Linea, zkSync, etc.)
- Adding a new L2/L3 network
- User says "add facilitator {network}" or "add network {network}"

## Quick Reference: What the Skill Does

```
User: "add facilitator scroll"
         │
         ▼
┌─────────────────────────────────┐
│ 1. RESEARCH PHASE               │
│    - Chain ID, RPCs             │
│    - USDC contracts             │
│    - EIP-3009 verification      │
│    - Explorer URLs              │
│    - Native token name          │
│    - EIP-1559 support           │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 2. PREREQUISITES CHECK          │
│    - Logo exists? (/static/)    │
│    - Mainnet wallet funded?     │
│    - Testnet wallet funded?     │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 3. ASK USER FOR MISSING ITEMS   │
│    - Request PNG if missing     │
│    - Request wallet funding     │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 4. IMPLEMENTATION               │
│    17 files, always. See the    │
│    inventory near the end of    │
│    this file - it is measured,  │
│    not a summary.               │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 5. SHIP (if auto-deploy)        │
│    - bump VERSION               │
│    - stage per file, commit     │
│    - push to main -> CI deploys │
│    - Verify /supported          │
└─────────────────────────────────┘
```

---

## Phase 1: Research

### 1.1 Gather Chain Information

**Required data to collect:**

| Field | Description | Example |
|-------|-------------|---------|
| Network Name | Official name | "Scroll" |
| Mainnet Chain ID | EVM chain ID | 534352 |
| Testnet Chain ID | Testnet chain ID | 534351 |
| Testnet Name | Full testnet name | "Scroll Sepolia" |
| Native Token | Gas token | "ETH" |
| EIP-1559 | Transaction type support | true/false |
| Mainnet RPC | Public RPC URL | https://rpc.scroll.io |
| Testnet RPC | Public RPC URL | https://sepolia-rpc.scroll.io |
| Mainnet Explorer | Block explorer | https://scrollscan.com |
| Testnet Explorer | Block explorer | https://sepolia.scrollscan.com |
| Brand Color | Hex color for CSS | #FFEEDA |

**Research sources:**
1. Official network documentation
2. ChainList.org for chain IDs and RPCs
3. Block explorers for contract verification

### 1.2 Find USDC Contract Addresses

**Search priority:**
1. Circle's official deployments: https://developers.circle.com/stablecoins/docs/usdc-on-main-networks
2. Bridge documentation (canonical USDC vs bridged)
3. Block explorer token search
4. DeFiLlama stablecoin tracker

**For each network, record:**

```
USDC Mainnet:
  - Address: 0x...
  - Decimals: 6
  - Type: Native/Bridged
  - EIP-712 Name: "USD Coin" or "USDC"
  - EIP-712 Version: "2"

USDC Testnet:
  - Address: 0x...
  - Same fields...
```

### 1.3 Verify EIP-3009 Support

**CRITICAL: x402 protocol REQUIRES EIP-3009 `transferWithAuthorization`.**

Run verification for each USDC contract:

```bash
# Test if transferWithAuthorization exists
cast call <USDC_ADDRESS> \
  "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)" \
  0x0000000000000000000000000000000000000001 \
  0x0000000000000000000000000000000000000002 \
  1000000 0 9999999999 \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  0x0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 \
  --rpc-url <RPC_URL>
```

**Interpretation:**
- `"invalid signature"` = EIP-3009 EXISTS (good!)
- `"execution reverted"` (generic) = NOT SUPPORTED (stop here)

### 1.4 Get EIP-712 Domain Metadata

Query the USDC contract to get exact EIP-712 domain:

```bash
# Get name (may differ from token symbol!)
cast call <USDC_ADDRESS> "name()" --rpc-url <RPC> | cast --to-ascii

# Get version
cast call <USDC_ADDRESS> "version()" --rpc-url <RPC> | cast --to-ascii

# Get decimals
cast call <USDC_ADDRESS> "decimals()" --rpc-url <RPC>
```

**IMPORTANT:** EIP-712 name often differs between chains:
- Ethereum/Avalanche: `"USD Coin"`
- Base/Celo/HyperEVM: `"USDC"`
- Some chains: `"Bridged USD Coin"`

Always verify from contract, never assume!

---

## Phase 2: Prerequisites Check

### 2.1 Check Logo Exists

```bash
ls -la static/{network}.png
```

If logo doesn't exist, ask user to provide:
- PNG format
- Transparent background
- ~32x32px or larger
- Place in `static/` directory

### 2.2 Check Wallet Balances

**Facilitator wallet addresses:**
- Mainnet: `0x103040545AC5031A11E8C03dd11324C7333a13C7`
- Testnet: `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`

Check both wallets have native tokens for gas:

```bash
# Mainnet balance
curl -s -X POST <MAINNET_RPC> \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x103040545AC5031A11E8C03dd11324C7333a13C7","latest"],"id":1}' \
  | jq -r '.result' | xargs -I{} python3 -c "print(int('{}', 16) / 1e18)"

# Testnet balance
curl -s -X POST <TESTNET_RPC> \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8","latest"],"id":1}' \
  | jq -r '.result' | xargs -I{} python3 -c "print(int('{}', 16) / 1e18)"
```

**Minimum recommended balances:**
- Mainnet: ~0.01 ETH equivalent (for ~100 transactions)
- Testnet: Any amount > 0 (faucet tokens)

If wallets are empty, ask user to fund:
- Provide faucet links for testnet
- User must send mainnet tokens manually

### 2.3 Summary Before Implementation

Present a summary to user before proceeding:

```
Network: Scroll
Chain IDs: 534352 (mainnet), 534351 (testnet)
USDC Contracts:
  - Mainnet: 0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4
  - Testnet: 0x...
EIP-3009: Verified
EIP-712 Domain: name="USD Coin", version="2"

Prerequisites:
  - Logo: static/scroll.png EXISTS
  - Mainnet wallet: 0.05 ETH FUNDED
  - Testnet wallet: 0.1 ETH FUNDED

Ready to implement!
```

---

## Phase 3: Implementation

### 3.1 Update src/network.rs

**Add Network enum variants:**

```rust
/// Scroll mainnet (chain ID 534352).
#[serde(rename = "scroll")]
Scroll,
/// Scroll Sepolia testnet (chain ID 534351).
#[serde(rename = "scroll-sepolia")]
ScrollSepolia,
```

**Add to all FOUR copies of `variants()`.**

They are four copies of the SAME function, not four different functions.
`mainnet_variants()`, `testnet_variants()` and `evm_variants()` DO NOT EXIST
anywhere in this repository:

```bash
grep -rn -e mainnet_variants -e testnet_variants -e evm_variants src/ crates/ examples/
# 0 hits (measured 2026-09-16 on dc109511)
```

What exists is one `Network::variants()` written out four times, each behind a
different `algorand` / `sui` feature combination:

| `#[cfg(...)]` above it | line | entries |
|---|---|---|
| `all(feature = "algorand", feature = "sui")` | `src/network.rs:346` | 39 |
| `all(feature = "algorand", not(feature = "sui"))` | `src/network.rs:394` | 37 |
| `all(not(feature = "algorand"), feature = "sui")` | `src/network.rs:440` | 37 |
| `all(not(feature = "algorand"), not(feature = "sui"))` | `src/network.rs:486` | 35 |

An EVM network belongs in **all four**. Only the first is compiled by the
production feature set, so the build stays green while the other three are wrong,
and the next person who builds without `--features sui` silently loses your chain.

**Nothing checks this.** `variants()` returns an array, not a `match`, so the
compiler cannot warn about a missing entry -- unlike `NetworkFamily`
(`src/network.rs:288`) and `to_caip2()` (`:576`), which are exhaustive and refuse
to compile. See [Closure criterion](#closure-criterion-supported-never-it-compiles)
at the end of this file: three networks that sit in the enum and in zero of the
four copies have been shipping, unserved, for months.

**Add Display impl:**

```rust
Self::Scroll => "Scroll",
Self::ScrollSepolia => "Scroll Sepolia",
```

**Add FromStr impl** (`src/network.rs:216`). It ends in
`_ => Err(NetworkParseError(...))` (`src/network.rs:269`), so a missing arm
compiles fine and only fails at runtime, on a client that named your chain:

```rust
"scroll" => Ok(Self::Scroll),
"scroll-sepolia" => Ok(Self::ScrollSepolia),
```

**Add `to_caip2()`** (`src/network.rs:576`). This one IS exhaustive -- no `_` arm
-- so the compiler names the network you skipped:

```rust
Self::Scroll => "eip155:534352".to_string(),
Self::ScrollSepolia => "eip155:534351".to_string(),
```

**Add to `from_caip2()`** (`src/network.rs:643`), a *separate* hand-written match
that ends in `_ => None` (`src/network.rs:704`). Skip it and `eip155:534352`
resolves to nothing while `"scroll"` works -- the exact split a v2 client hits and
a v1 client never does:

```rust
"eip155:534352" => Some(Network::Scroll),
"eip155:534351" => Some(Network::ScrollSepolia),
```

Those three sites -- the four `variants()` copies, `FromStr` and `from_caip2` --
are the ONLY per-network sites in `src/network.rs` the compiler does not enforce.
`Display`, `NetworkFamily` and `to_caip2` are exhaustive matches and break the
build if you skip them. The compiler-silent three are where a network goes
missing.

**Add NetworkFamily mapping:**

```rust
Self::Scroll | Self::ScrollSepolia => NetworkFamily::Evm,
```

**Add USDC deployment constants:**

```rust
// ============================================================================
// USDC on Scroll
// ============================================================================

static USDC_SCROLL: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4").into(),
            network: Network::Scroll,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

static USDC_SCROLL_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("...TESTNET_ADDRESS...").into(),
            network: Network::ScrollSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});
```

**Add to usdc_deployments():**

```rust
Self::Scroll => Some(USDC_SCROLL.clone()),
Self::ScrollSepolia => Some(USDC_SCROLL_SEPOLIA.clone()),
```

### 3.2 Update src/from_env.rs

**Add RPC constants:**

```rust
pub const ENV_RPC_SCROLL: &str = "RPC_URL_SCROLL";
pub const ENV_RPC_SCROLL_SEPOLIA: &str = "RPC_URL_SCROLL_SEPOLIA";
```

**Add to rpc_env_name_from_network():**

```rust
Network::Scroll => ENV_RPC_SCROLL,
Network::ScrollSepolia => ENV_RPC_SCROLL_SEPOLIA,
```

### 3.3 Update src/chain/evm.rs

**Add chain ID mappings in TryFrom<Network>:**

```rust
Network::Scroll => Ok(EvmChain::new(value, 534352)),
Network::ScrollSepolia => Ok(EvmChain::new(value, 534351)),
```

**Add EIP-1559 support (usually true for modern chains):**

```rust
Network::Scroll => true,
Network::ScrollSepolia => true,
```

**IMPORTANT:** Some chains like SKALE don't support EIP-1559. Set to `false` for those.

**Decide the EIP-1559 fee floor** -- `eip1559_fee_floor()`, `src/chain/evm.rs:329`.

It is a `match` with a catch-all `_` arm, so a new chain silently inherits the
default (`min_priority` 1 mwei, `min_max_fee` 0, `fallback_base_fee` 2 gwei) and
the compiler says nothing. That default is deliberate and right for most L2s, but
it is a decision you are making by omission, so make it on purpose:

- Explicit arms today: `Ethereum | EthereumSepolia` (1 gwei tip, 5 gwei cap) and
  `Polygon | PolygonAmoy` (30 gwei tip, 1000 gwei cap).
- Add an arm only if the chain's nodes refuse the default. Measure
  `eth_maxPriorityFeePerGas` and the recent base fee before writing a number.
- Do NOT copy the Ethereum arm "to be safe". A 1 gwei floor copied onto the L2
  branch on 2026-09-10 made a Base settle cost 0.0001038 ETH instead of
  ~0.0000006 ETH, drained the mainnet signer in four days and refused every Base
  settle until 2.29.4. The comment above the `_` arm carries the full account.
- Zero is not safe either: geth/op-geth mine no tip below 1 mwei, so a zero floor
  yields a transaction that is never mined while it holds a nonce.

### 3.3b Optional per-network lists (`escrow`, `upto`)

Neither is automatic and neither is required. A new network gets `exact` for free;
these two are opt-in lists and joining them is a separate decision:

| Scheme | List | Extra work |
|---|---|---|
| `escrow` / `commerce` | `ESCROW_NETWORKS`, `src/payment_operator/addresses.rs:185` | Needs a deployed PaymentOperator + escrow + token collector for that chain. The test at `addresses.rs:452` asserts `ESCROW_NETWORKS.len() == 11` -- bump it or the suite goes red. Also bumps the `x402r.networksTitle` count on the landing (EN **and** ES). |
| `upto` | `UPTO_DEPLOYED_NETWORKS`, `src/upto/types.rs:60` | Only after the Permit2 proxy CREATE2 deployment has actually been replayed there. Verify with `eth_getCode` against **two** independent RPCs first: a wrong entry reports settlement success while moving zero tokens, which is exactly what shipped once. |

If the network joins neither, change neither file.

### 3.4 Update src/chain/solana.rs

**Add UnsupportedNetwork exclusions:**

```rust
Network::Scroll => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
Network::ScrollSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
```

### 3.5 Update src/handlers.rs

**Add logo handler:**

```rust
pub async fn get_scroll_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/scroll.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}
```

**Add route:**

```rust
.route("/scroll.png", get(get_scroll_logo))
```

### 3.6 Update static/index.html

**Add CSS styling:**

```css
.network-badge.scroll {
    border-color: #FFEEDA;
    box-shadow: 0 0 20px rgba(255, 238, 218, 0.2);
}

.network-badge.scroll:hover {
    border-color: #FFEEDA;
    box-shadow: 0 8px 24px rgba(255, 238, 218, 0.3);
}
```

**Add mainnet card:**

```html
<div class="network-badge scroll"
    style="padding: 1.25rem 2rem; font-size: 1.1rem; flex-direction: column; gap: 0.75rem; min-height: 220px; width: 100%; cursor: pointer;"
    onclick="window.open('https://scrollscan.com/address/0x103040545AC5031A11E8C03dd11324C7333a13C7', '_blank')">
    <div style="display: flex; align-items: center; gap: 0.75rem;">
        <img src="/scroll.png" alt="Scroll" style="width: 32px; height: 32px; object-fit: contain;">
        <span style="font-weight: 700;">Scroll</span>
    </div>
    <div class="balance-amount" data-balance="scroll-mainnet"
        style="font-size: 1.1rem; font-weight: 700; font-family: 'JetBrains Mono', monospace; color: rgba(255,255,255,0.9);">
        Loading...
    </div>
    <div style="font-size: 0.7rem; color: var(--text-muted);">ETH Balance</div>
    <div data-tokens="scroll-mainnet"></div>
</div>
```

**Add testnet card** (similar structure).

**Add BALANCE_CONFIG entries:**

```javascript
'scroll-mainnet': {
    rpc: 'https://rpc.scroll.io',
    address: MAINNET_ADDRESS
},
'scroll-testnet': {
    rpc: 'https://sepolia-rpc.scroll.io',
    address: TESTNET_ADDRESS
}
```

**Add TOKEN_SUPPORT entries:**

```javascript
'scroll-mainnet': ['usdc'],
'scroll-testnet': ['usdc']
```

### 3.7 Update .env.example

```bash
# Scroll RPCs
RPC_URL_SCROLL=https://rpc.scroll.io
RPC_URL_SCROLL_SEPOLIA=https://sepolia-rpc.scroll.io
```

### 3.8 Update README.md

- Update network counts (mainnets and testnets)
- Add to supported networks table
- Run `python scripts/stablecoin_matrix.py --md` and update stablecoin table

### 3.9 Update config/supported_tokens.json

After adding the network to `src/network.rs`, also update `config/supported_tokens.json`:

- Add the new network entry with `chainId`, `tokens` array, `explorer` URL, and `facilitatorWallet`
- Place mainnet entries under `evm_mainnets` and testnet entries under `evm_testnets`
- Use the correct facilitator wallet address for the chain type:
  - **EVM mainnets:** `0x103040545AC5031A11E8C03dd11324C7333a13C7`
  - **EVM testnets:** `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`
  - **Solana/Fogo:** `F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq`
  - **SUI:** `0xe7bbf2b13f7d72714760aa16e024fa1b35a978793f9893d0568a4fbf356a764a`
  - **NEAR:** `uvd-facilitator.near`
  - **Stellar:** `GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB`
  - **Algorand:** `KIMS5H6QLCUDL65L5UBTOXDPWLMTS7N3AAC3I6B2NCONEI5QIVK7LH2C2I`
- Update the summary counts at the bottom of the JSON file
- **NEVER type wallet addresses from memory** - always copy from `lambda/balances/handler.py`

**Example entry:**

```json
"scroll": {
  "chainId": 534352,
  "tokens": ["usdc"],
  "explorer": "https://scrollscan.com",
  "facilitatorWallet": "0x103040545AC5031A11E8C03dd11324C7333a13C7"
}
```

### 3.10 Update src/openapi.rs (Swagger / OpenAPI)

The interactive API docs at `/docs` (Swagger UI) and `/api-docs/openapi.json` are
generated from `src/openapi.rs`. A new network is INVISIBLE in the published API
spec until it is added here — this is a required step, not optional.

- Add the new network's serde name (e.g. `"scroll"`) to the `Network` enum /
  examples in `src/openapi.rs`. Keep it consistent with `src/network.rs`.
- Do NOT edit the version in `src/openapi.rs`. It is patched at runtime from the
  `VERSION` file via `FACILITATOR_VERSION` (`src/version.rs`). It does **not**
  come from `Cargo.toml`: `Cargo.toml:3` is a frozen `0.0.0` placeholder and must
  stay untouched, because the Docker dependency layer is keyed on that file.
- `src/openapi.rs` also hardcodes network lists and counts in prose, plus the
  escrow and `upto` lists. Line numbers drift, so find them rather than trusting a
  list: `grep -n 'Scroll\|scroll\|mainnets\|networks (' src/openapi.rs`. On
  `dc109511` that is `:35` (the EVM mainnet roll-call), `:59` and `:175` (the
  ERC-8004 counts), `:822` (upto), `:826` (escrow) and `:1691` / `:1750` (the
  per-endpoint EVM lists). A new network is invisible in `/docs` until each one is
  edited.
- Verify after deploy:
  `curl -s https://facilitator.ultravioletadao.xyz/api-docs/openapi.json | jq '.paths,.components.schemas.Network'`

### 3.11 Canonical consistency — landing MUST match /supported

The landing page network counts are NOT free text: they are a single source of
truth. `/supported` is canonical for payment networks; `src/payment_operator/addresses.rs`
for escrow; `src/erc8004/mod.rs` for ERC-8004. NEVER hardcode a number that
disagrees with these.

- The landing computes the live payment-network count from `/supported` in the
  browser (`[data-live-count="payment-mainnets"]`). There is no longer a typed
  `N mainnets` fallback string on that page -- the guard reports
  `typed 'N mainnets' : not typed on this page` (measured 2026-09-16). Do not go
  looking for `data-i18n="sdk.networks"`; it is gone. Leave it gone.
- The balance wall on `/` is still hand-written: 39 `class="network-badge ..."`
  cards in `static/index.html`, one per served network, confirmed live
  (`curl -s https://facilitator.ultravioletadao.xyz/ | grep -c 'class="network-badge'`
  -> 39). Adding a network means adding two cards there (mainnet + testnet).
- `/networks` is NOT hand-written: `static/networks.html` builds its whole table
  from `GET /supported`. Do not add a row there. What it DOES need is one entry
  per name in `ICONO_DE_RED`, `static/x402.js:14` -- four keys per network (v1
  mainnet, v1 testnet, CAIP-2 mainnet, CAIP-2 testnet). Without them the chip
  falls back to a monogram, which is the deliberate behaviour for a network with
  no PNG, so nothing warns you.
- If the network gains escrow or ERC-8004, update those grids AND their
  "Escrow Deployed on N Networks" / "Deployed on N Networks" headings + the
  ERC-8004 stat card (`id="ovr-erc8004-networks"`), EN + ES. These ARE typed and
  the guard treats a mismatch as an error, not a note.
- After building, run the canonical check and resolve any drift it reports:
  ```bash
  python scripts/verify_landing_canonical.py            # reads live /supported
  python scripts/verify_landing_canonical.py --offline  # what CI runs
  ```
  CI runs the `--offline` form in the `Build & test` job, so a drift here goes red
  before the deploy job starts.
- **Bump `--expect-mainnets`.** Its default lives in the script itself
  (`scripts/verify_landing_canonical.py:306`, currently `21`). A new mainnet makes
  `/supported` disagree with it. The docstring header (`:11`) carries the same
  number and is prose -- update both.

### 3.11b `src/caip2.rs` -- usually NOT edited

Measured, against the claim that an alta touches it: a new **EVM** chain needs no
change in `src/caip2.rs`. The file is generic over `eip155:<chain-id>`
(`Caip2NetworkId::eip155`, `src/caip2.rs:175`) and carries no per-network table;
its diff in the last full alta (`7dbe194e`) was pure `rustfmt`. The per-network
CAIP-2 work lives in `to_caip2()` / `from_caip2()` in `src/network.rs` (§3.1).

`src/caip2.rs` only needs editing for a new **family** -- a namespace that is not
already one of `eip155`, `solana`, `near`, `stellar`, `xrpl`, `algorand`, `sui`
(`Namespace`, `src/caip2.rs:52`). That is a different, much larger job than adding
a chain.

### 3.12 Update lambda/balances/handler.py

Add the network to `get_network_configs()` (RPC list + facilitator wallet) so the
landing balance cards render. Copy the wallet address from this file — NEVER from memory.

### 3.13 Give the CONTAINER the RPC URL (Terraform)

**This is the step that decides whether the network appears in `/supported` at
all**, and it is the one most often skipped, because everything else compiles and
passes without it.

`src/from_env.rs` only declares the variable NAME. `.env.example` only documents
it for local runs. Production reads the ECS task definition, which is generated by
Terraform. Declared in `from_env.rs` alone, the container never receives the URL,
`NetworkProvider::from_env` returns `None`, and the network is quietly absent from
`/supported` with a clean build behind it.

- **Public / free RPC** -> `terraform/environments/production/main.tf`, in the
  task definition's `environment` block. This is where every recent alta put it:

  ```hcl
  {
    name  = "RPC_URL_SCROLL"
    value = "https://rpc.scroll.io"
  },
  {
    name  = "RPC_URL_SCROLL_SEPOLIA"
    value = "https://sepolia-rpc.scroll.io"
  },
  ```

- **RPC URL carrying an API key** -> NEVER in `environment`. Add the key to the
  `facilitator-rpc-mainnet` / `facilitator-rpc-testnet` secret and reference it
  from the `secrets` block, wired in
  `terraform/environments/production/secrets.tf`. Task definitions are plaintext
  and their history is retained, so a key put there is exposed even after
  rotation.

Both testnet and mainnet need an entry. After the deploy, the fastest proof is
`/supported` itself, not the logs.

### 3.14 `VERSION` and `docs/CHANGELOG.md`

- `VERSION` (repo root) -- bump it, see Phase 5.1. Nothing ships without it: CI
  fails the run outright if the file is empty, and the image tag and
  `FACILITATOR_VERSION` both come from it. `Cargo.toml` stays untouched.
- `docs/CHANGELOG.md` -- add the release entry. It is the only written record:
  releases stopped being git-tagged at `v2.0.2`, so a chain added without a
  CHANGELOG line has no date attached to it anywhere.

---

## Phase 4: Build and Verify Locally

```bash
# Build
cargo build --release

# Check for errors
cargo clippy --all-targets

# The gate CI actually runs. A red run here blocks the production deploy, and
# --test-threads=1 is not optional: parallel runs hang on CI runners.
cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1

# Run locally
cargo run --release

# Verify network appears -- THIS is the check that matters, not the build
curl http://localhost:8080/supported | jq '[.kinds[].network] | map(select(contains("scroll")))'
```

**A green build proves nothing about `/supported`.** If that last command returns
`[]`, the network is not served, however clean the compile was. See
[Closure criterion](#closure-criterion-supported-never-it-compiles).

---

## Phase 5: Deploy

**There is no manual deploy. Pushing to `main` IS the deploy.**

`.github/workflows/ci.yaml` tests, builds the image, pushes it to ECR and
`terraform apply -auto-approve`s it onto ECS, then waits for the rollout and
checks `/health`. A merge is a release. The `docker build` / `aws ecs
update-service` sequence this section used to describe is not how anything has
shipped for a long time, and `aws ecs update-service --force-new-deployment` on
its own re-runs the CURRENT task definition -- it does not move the image.

### 5.1 Bump `VERSION`

The release version lives in the `VERSION` file at the repo root, **not** in
`Cargo.toml` (`Cargo.toml:3` is a frozen `0.0.0` placeholder; touching it makes
every deploy recompile the whole dependency tree). Bump from what is DEPLOYED,
not from whatever is local:

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # e.g. {"version":"2.29.6"}
echo "2.30.0" > VERSION                                   # a network add is a minor bump
```

### 5.2 Stage per file, never `git add -A`

`git add -A` is forbidden in this repository. `.unused/` and untracked scratch
files live beside the tree, and the 2026-05-19 security audit recorded a wallet
rotation script that writes a freshly generated key into the repo root, where one
`git add -A` would publish it (`docs/reports/2026-05-19-security-audit.md`). Name
every path:

```bash
git add src/network.rs src/from_env.rs src/chain/evm.rs src/chain/solana.rs
git add src/handlers.rs src/openapi.rs
git add static/index.html static/x402.js static/{network}.png
git add config/supported_tokens.json lambda/balances/handler.py
git add terraform/environments/production/main.tf
git add scripts/verify_landing_canonical.py .env.example README.md
git add VERSION docs/CHANGELOG.md

git status --short          # read it; anything unexpected staged is a stop
git diff --cached --stat    # and this
git commit -m "feat(network): add {Network} mainnet and testnet ({mainnet-id}/{testnet-id})"
```

Run `git config core.hooksPath .githooks` once per clone: the pre-commit hook
refuses a staged diff that adds `0x` + 64 hex.

### 5.3 Push and let CI ship it

```bash
git push origin main
```

What CI then does, in order (`.github/workflows/ci.yaml`):

1. `test` -- clippy + the full feature-set test suite. Red here blocks everything.
2. `preflight` -- emits `deploy=true` when the AWS repo secrets are present.
3. drift gate -- read-only `terraform plan`, never applies.
4. `deploy` -- image tag is `$(cat VERSION)-$(git rev-parse --short HEAD)`, built
   with `--build-arg FACILITATOR_VERSION=$(cat VERSION)`, pushed to ECR, then a
   **targeted** `terraform apply` on the ECS task definition + service +
   autoscaling with `-var image_tag=...`. Never a full apply.
5. waits for the rollout, then polls `/health`.

Two consequences worth knowing:

- The workflow has a `paths` filter. `.claude/**`, `docs/**` and `guides/**` are
  NOT in it, so a documentation-only commit never triggers CI and never deploys.
  Every file in the alta inventory except those three trees IS in it.
- A failed deploy leaves `main` ahead of production. Compare
  `curl -s https://facilitator.ultravioletadao.xyz/version` with `git log -1`
  before assuming your commit is live.

Releases are **not** git-tagged any more -- `git tag` stops at `v2.0.2` while
`VERSION` is well past it. Do not add a tag to "finish" a release.

### 5.4 Verify against production

```bash
curl -s https://facilitator.ultravioletadao.xyz/version
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[].network] | map(select(startswith("scroll")))'
curl -sI https://facilitator.ultravioletadao.xyz/scroll.png | head -1
python scripts/verify_landing_canonical.py
```

---

## Automatic Deployment Decision

**Read this first: "deploy" here means `git push origin main`.** There is no
separate deploy button. CI builds the image and rolls production on that push, so
an automatic deploy is an automatic production release. The repository rule is
that the model does not compile or deploy on its own initiative: make the change,
say it is ready, and let the person push. "Automatic" below means *do not stop to
re-ask about each prerequisite*, not *ship without being asked*.

**Proceed without further questions when ALL conditions are met:**
- Logo exists in `static/`
- Mainnet wallet balance > 0.001 ETH equivalent
- Testnet wallet balance > 0
- EIP-3009 verified on USDC contracts
- User has not requested manual review

**Request user confirmation when:**
- Any prerequisite is missing
- Chain has unusual characteristics (no EIP-1559, special gas token)
- Premium RPC required (API key needed)
- This is the first time adding this type of chain

---

## Troubleshooting

### "Network not in /supported"

In order of how often it is the cause:

1. **The container never got the RPC URL.** Declaring `ENV_RPC_*` in
   `src/from_env.rs` does nothing on its own -- the value comes from the ECS task
   definition, generated by `terraform/environments/production/main.tf` (or
   `secrets.tf` for a URL with an API key). See §3.13.
2. **The network is missing from one or more of the four `variants()` copies**
   (`src/network.rs:346`, `:394`, `:440`, `:486`). Nothing warns about this; see
   the closure criterion above. Check all four, not the first one.
3. RPC unreachable from the task, or the endpoint rejects the facilitator's calls.
4. The deploy did not actually land -- compare
   `curl -s https://facilitator.ultravioletadao.xyz/version` against `git log -1`.

### "Logo 404"
- Verify file exists: `ls static/{network}.png`
- Verify handler added to handlers.rs
- Verify route added to router
- Rebuild Docker image

### "Balance shows Loading..."
- Check BALANCE_CONFIG has correct RPC URL
- Check data-balance attribute matches config key
- Test RPC endpoint manually

### "Invalid signature" on payments
- EIP-712 name doesn't match (check contract!)
- EIP-712 version wrong
- Chain uses different signature format

---

## File inventory (measured, not estimated)

Derived from `7dbe194e` -- Robinhood Chain, 2026-07-20, the last full network
alta: **24 files, 689 insertions, 216 deletions**
(`git show --stat 7dbe194e`). Five of those 24 were not alta work and are
excluded below: `src/caip2.rs`, `src/chain/xrpl.rs`, `src/facilitator_local.rs`
and `examples/x402-reqwest-example/src/main.rs` were whole-file `rustfmt` with
zero mentions of the new chain (`git show 7dbe194e -- examples/... | grep -ci
robinhood` -> 0), and `src/upto/permit2.rs` was an unrelated security fix riding
along.

Two files have joined the list since: `VERSION` (did not exist at `7dbe194e`;
`git cat-file -e 7dbe194e:VERSION` fails) and `static/x402.js`.

### Always -- 17 files

| # | File | What | Compiler catches an omission? |
|---|------|------|---|
| 1 | `src/network.rs` | enum + serde rename, `Display`, `FromStr`, `to_caip2`, `from_caip2`, `NetworkFamily`, **4x `variants()`**, token deployment, `usdc_deployments()` (~226 lines in the measured alta) | Partly. `variants()` x4, `FromStr` and `from_caip2` are NOT enforced |
| 2 | `src/from_env.rs` | `ENV_RPC_*` consts + `rpc_env_name_from_network()` | yes (match) |
| 3 | `src/chain/evm.rs` | `TryFrom<Network> for EvmChain` chain id, EIP-1559 flag; `eip1559_fee_floor` arm only if needed | chain id yes; fee floor NO (`_` arm) |
| 4 | `src/chain/solana.rs` | `UnsupportedNetwork` exclusion arms | yes (match) |
| 5 | `src/handlers.rs` | logo handler + `.route("/{network}.png", ...)` | no |
| 6 | `src/openapi.rs` | network prose and lists -- find them with `grep -n 'Scroll\|scroll\|mainnets\|networks (' src/openapi.rs` | no |
| 7 | `static/{network}.png` | logo, flat in `static/` (no `static/images/`) | build fails -- `include_bytes!` |
| 8 | `static/index.html` | 2 cards (mainnet + testnet) + CSS + balance config | no |
| 9 | `static/x402.js` | `ICONO_DE_RED` (`:14`), 4 keys: v1 mainnet, v1 testnet, both CAIP-2 | no -- falls back to a monogram, silently |
| 10 | `config/supported_tokens.json` | chainId, tokens, explorer, facilitatorWallet | no |
| 11 | `lambda/balances/handler.py` | `get_network_configs()`: RPC + wallet | no |
| 12 | `terraform/environments/production/main.tf` | `RPC_URL_*` in the task definition `environment` | no -- **and this is what keeps it out of `/supported`** |
| 13 | `scripts/verify_landing_canonical.py` | `--expect-mainnets` default (`:306`) + the docstring count (`:11`) | the guard itself, in CI |
| 14 | `.env.example` | both RPC URLs | no |
| 15 | `README.md` | network counts + tables | no |
| 16 | `VERSION` | the release bump | CI fails if empty |
| 17 | `docs/CHANGELOG.md` | the release entry | no |

### Conditional -- up to 7 more

| File | Only when |
|------|-----------|
| `terraform/environments/production/secrets.tf` | the mainnet RPC carries an API key |
| `src/types.rs` | the chain settles a stablecoin with no `TokenType` yet |
| `scripts/stablecoin_matrix.py` | same -- add the symbol to the allow-list |
| `static/{token}.png` + `ICONO_DE_TOKEN` in `static/x402.js` | same |
| `src/upto/types.rs` | the Permit2 proxy is genuinely deployed there (verify with `eth_getCode` on two RPCs) |
| `src/payment_operator/addresses.rs` | the chain joins escrow -- also bump the `len() == 11` test at `:452` |
| `src/erc8004/mod.rs` | the chain joins ERC-8004 -- also the landing stat card, EN + ES |

**Not** in the inventory, against a common assumption: `src/caip2.rs` (generic
over `eip155:<id>`, see §3.11b) and `static/networks.html` (generated from
`/supported`).

**Total: 17 files always, up to 24 with the conditional ones. ~500-700 changed
lines plus 1-2 PNGs, AWS config and wallet funding.**

---

## Closure criterion: `/supported`, never "it compiles"

An alta is finished when the network answers on `GET /supported`. Nothing else
counts -- not a green `cargo build`, not a green test suite, not a successful
deploy.

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[].network] | map(select(startswith("scroll")))'
# [] means NOT DONE, whatever else is green
```

### The proof: three networks that compile and are served nowhere

`Sei`, `SeiTestnet` and `XdcMainnet` are declared in the `Network` enum
(42 variants) and wired into every compiler-enforced site -- `Display`
(`src/network.rs:161`, `:174`, `:175`), `FromStr` (`:223`, `:236`, `:237`),
`to_caip2`, `from_caip2`, `NetworkFamily` (`:293`, `:306`, `:307`). They
serialize, they parse, both spellings resolve.

They are in **zero of the four `variants()` copies**. Measured on `dc109511`:

| | count |
|---|---|
| `Network` enum variants | 42 |
| union of the four `variants()` copies | 39 |
| in the enum, in no copy | 3 -- `Sei`, `SeiTestnet`, `XdcMainnet` |
| distinct v1 names in live `/supported` | 39 |
| the union vs live `/supported` | identical, name for name |

That last row is the whole point: `ProviderCache::from_env` iterates
`Network::variants()` (`src/provider_cache.rs:113`) and `/supported` walks the
provider map (`src/facilitator_local.rs:289`). A variant outside `variants()` gets
no provider and is advertised nowhere -- no error, no warning, no failing test.

Reproduce it:

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l   # 39
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '[.kinds[].network]|unique|.[]' | grep -iE 'sei|xdc'    # nothing
```

Note `/supported` lists each chain twice -- once by v1 name, once by CAIP-2 alias
-- so an unfiltered `length` counts identifier strings, not networks. Filter out
`:` for a v1-name count, and use `python scripts/verify_landing_canonical.py` for
the canonical mainnet count.

---

## Examples

### Example 1: Scroll (all prerequisites met)

```
User: add facilitator scroll
Claude: [Research phase - finds all data]
Claude: [Checks prerequisites]
  - Logo: static/scroll.png EXISTS
  - Mainnet: 0.05 ETH FUNDED
  - Testnet: 0.1 ETH FUNDED
Claude: All prerequisites met. Implementing the 17 files...
[Implements all changes, bumps VERSION, stages per file, commits]
Claude: Committed. Pushing to main deploys to production - your call.
User: push it
[Push; CI builds, applies to ECS, waits for the rollout]
Claude: Live. /supported lists scroll + scroll-sepolia; /version reports 2.30.0.
```

### Example 2: New chain (missing logo)

```
User: add facilitator linea
Claude: [Research phase]
Claude: [Checks prerequisites]
  - Logo: MISSING - Please provide static/linea.png
  - Mainnet: 0.02 ETH FUNDED
  - Testnet: 0.05 ETH FUNDED
Claude: I need the Linea logo (PNG, transparent background). Please place it at static/linea.png and let me know when ready.
User: [provides logo]
Claude: Logo found! Continuing with implementation and deploy...
```

### Example 3: Wallets not funded

```
User: add facilitator blast
Claude: [Research phase]
Claude: [Checks prerequisites]
  - Logo: static/blast.png EXISTS
  - Mainnet: 0 ETH NOT FUNDED
  - Testnet: 0 ETH NOT FUNDED
Claude: Wallets need funding:
  - Mainnet (0x103040545AC5031A11E8C03dd11324C7333a13C7): Send ETH
  - Testnet (0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8): Use faucet at https://blastfaucet.com
Let me know when funded!
```
