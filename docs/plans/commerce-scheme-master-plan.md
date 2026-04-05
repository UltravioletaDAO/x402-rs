# Commerce Scheme Integration Master Plan

**Date**: 2026-04-05
**Status**: READY TO EXECUTE
**Estimated effort**: ~50 lines of Rust + docs + tests
**Triggered by**: Ali (BackTrackCo) shared `ultravioleta-commerce-integration.md` + arbiter example PR

---

## Part 1: Handoff - Strategic Benefits

### What is Execution Market?

Execution Market (https://execution.market) is Ali's production-deployed **Universal Execution Layer** -- a decentralized marketplace where AI agents publish paid tasks (bounties), human executors complete them, and payment settles instantly on-chain via **our facilitator**.

- **Live in production** with 105 REST endpoints, 27 MCP tools, mobile app (Expo), XMTP bot
- **9 mainnet chains** supported (Base, Ethereum, Polygon, Arbitrum, Avalanche, Optimism, Celo, Monad, Solana)
- **Fee model**: 87% worker / 13% platform (on-chain StaticFeeCalculator)
- **1,950+ tests** (Python + JS + E2E)

### Why "commerce" instead of "escrow"?

x402r renamed `escrow` -> `commerce` in their SDK (`@x402r/evm`). Merchants send `scheme: "commerce"` in payment requirements. Our facilitator only recognizes `"escrow"`, so the scheme match **fails silently**. Ali is running a temporary JS facilitator as a workaround.

### Infrastructure is Already Shared

| Contract | Ali's proposed address | Our `addresses.rs` |
|---|---|---|
| escrowAddress | `0xBC151792...A60e245` | `create3::ESCROW` -- **IDENTICAL** |
| tokenCollector | `0x9A12A116...2f30` | `create3::TOKEN_COLLECTOR` -- **IDENTICAL** |

This is not a new integration. It's removing a naming barrier on shared infrastructure.

### Benefits for Ultravioleta DAO

1. **We become THE production facilitator for x402r Execution Market**
   - Ali retires their temporary JS facilitator
   - Every EM payment routes through us (authorize, release, refund)
   - Direct revenue: gas fees on every settlement

2. **Arbiter ecosystem runs on top of us**
   - The AI garbage detection arbiter (BackTrackCo/arbiter-examples#3) uses our facilitator for settlement
   - Post-settlement arbiter evaluates API response quality -> release or refund
   - We enable quality-guaranteed API monetization without writing arbiter logic

3. **Multi-chain from day one**
   - x402r CREATE3 contracts on Base, Base Sepolia, Arbitrum Sepolia, SKALE Base
   - All chains we already support

4. **Execution Market's value prop amplifies ours**
   - 1,950+ tests validate our facilitator API surface
   - Their SDKs (Python + TypeScript) become our ecosystem libraries
   - Their mobile app, dashboard, XMTP bot all depend on our uptime

5. **Strategic positioning**
   - First facilitator to support `commerce` scheme
   - Reference implementation for x402r ecosystem
   - When other merchants build on x402r, they default to our facilitator

### What the Arbiter Does (Does NOT Touch Our Facilitator)

```
Client pays -> Our Facilitator settles (authorize into escrow) -> OUR JOB ENDS HERE
                                                                |
                                                 Merchant's onAfterSettle hook
                                                                |
                                                 Arbiter evaluates response body
                                                                |
                                         PASS: arbiter calls release() on operator
                                         FAIL: arbiter calls refundInEscrow() immediately
```

The facilitator never talks to the arbiter. The arbiter interacts directly with smart contracts.

---

## Part 2: Implementation Plan

### Phase 1 - Core Scheme Support (Rust Code Changes)

**Goal**: Accept `"commerce"` as a valid scheme alongside `"escrow"`.

#### Step 1.1: Add serde alias to Scheme enum

**File**: `src/types.rs` line 110

```rust
// BEFORE:
#[serde(rename = "escrow")]
Escrow,

// AFTER:
#[serde(rename = "escrow", alias = "commerce")]
Escrow,
```

This makes serde deserialize both `"escrow"` and `"commerce"` into `Scheme::Escrow`. Serialization always outputs `"escrow"` (the `rename` value). No other enum changes needed.

#### Step 1.2: Add COMMERCE_SCHEME constant and helper function

**File**: `src/payment_operator/operator.rs` near line 46

```rust
// BEFORE:
pub const ESCROW_SCHEME: &str = "escrow";

// AFTER:
pub const ESCROW_SCHEME: &str = "escrow";
pub const COMMERCE_SCHEME: &str = "commerce";

/// Check if a scheme string is an escrow-family scheme (escrow or commerce).
pub fn is_escrow_scheme(s: Option<&str>) -> bool {
    matches!(s, Some(ESCROW_SCHEME) | Some(COMMERCE_SCHEME))
}
```

#### Step 1.3: Update all scheme comparisons in handlers.rs (4 locations)

**File**: `src/handlers.rs`

| Line | Current | Change to |
|------|---------|-----------|
| 1064 | `scheme == Some(crate::payment_operator::ESCROW_SCHEME)` | `crate::payment_operator::is_escrow_scheme(scheme)` |
| 1100 | `top_level_scheme == Some(crate::payment_operator::ESCROW_SCHEME)` | `crate::payment_operator::is_escrow_scheme(top_level_scheme)` |
| 1496 | `scheme == Some(crate::payment_operator::ESCROW_SCHEME)` | `crate::payment_operator::is_escrow_scheme(scheme)` |
| 1533 | `top_level_scheme == Some(crate::payment_operator::ESCROW_SCHEME)` | `crate::payment_operator::is_escrow_scheme(top_level_scheme)` |

#### Step 1.4: Update all scheme comparisons in operator.rs (4 locations)

**File**: `src/payment_operator/operator.rs`

| Line | Current | Change to |
|------|---------|-----------|
| 250 | `== Some(ESCROW_SCHEME)` | `is_escrow_scheme(...)` (use the function with the extracted scheme) |
| 265 | `== Some(ESCROW_SCHEME)` | `is_escrow_scheme(scheme)` |
| 559 | `== Some(ESCROW_SCHEME)` | `is_escrow_scheme(...)` |
| 584 | `== Some(ESCROW_SCHEME)` | `is_escrow_scheme(scheme)` |

**Note on operator.rs refactor**: The comparisons at lines 250 and 559 extract the scheme inline. Refactor to:
```rust
let scheme = json.get("scheme").and_then(|s| s.as_str());
if is_escrow_scheme(scheme) {
```

#### Step 1.5: Export `is_escrow_scheme` from payment_operator module

**File**: `src/payment_operator/mod.rs`

Add `is_escrow_scheme` to the public exports.

---

### Phase 2 - Advertise Commerce in /supported

**Goal**: Return `commerce` scheme entries alongside `escrow` in the `/supported` endpoint.

#### Step 2.1: Duplicate escrow entries with commerce scheme

**File**: `src/facilitator_local.rs` around line 219

After the existing loop that pushes `Scheme::Escrow` entries, add a second pass that pushes `Scheme::Commerce` entries with the same data. This requires adding a `Commerce` variant OR using a simpler approach:

**Decision**: Since serde alias means `Scheme::Escrow` always serializes as `"escrow"`, we need a separate approach for `/supported` to also advertise `"commerce"`. Two options:

**Option A** (Recommended): Add `Scheme::Commerce` as a distinct variant that serializes to `"commerce"` but routes identically to escrow:
```rust
#[serde(rename = "commerce")]
Commerce,
```

Then in `facilitator_local.rs`, push entries for both `Scheme::Escrow` AND `Scheme::Commerce` in the same loop.

**Option B**: Keep single variant with alias, manually construct JSON for `/supported`.

**Go with Option A** - cleaner, type-safe, and the `/supported` endpoint naturally advertises both.

#### Step 2.2: Update Scheme enum (revised from Step 1.1)

**File**: `src/types.rs`

```rust
pub enum Scheme {
    Exact,
    #[serde(rename = "fhe-transfer")]
    FheTransfer,
    #[serde(rename = "escrow", alias = "commerce")]
    Escrow,
    /// Commerce scheme (x402r) - functionally identical to Escrow.
    /// Separate variant to advertise "commerce" in /supported endpoint.
    #[serde(rename = "commerce")]
    Commerce,
    #[serde(rename = "upto")]
    Upto,
}
```

Wait -- this creates ambiguity: both `Escrow` and `Commerce` would match "commerce" on deserialization. Serde will use the first match, so `Escrow` (with `alias = "commerce"`) would win. Then `Commerce` variant is only for serialization in `/supported`.

**Cleaner approach**: Remove the alias from Escrow, add Commerce as its own variant, and update `is_escrow_scheme` and `Display`:

```rust
pub enum Scheme {
    Exact,
    #[serde(rename = "fhe-transfer")]
    FheTransfer,
    #[serde(rename = "escrow")]
    Escrow,
    #[serde(rename = "commerce")]
    Commerce,
    #[serde(rename = "upto")]
    Upto,
}

impl Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Scheme::Exact => "exact",
            Scheme::FheTransfer => "fhe-transfer",
            Scheme::Escrow => "escrow",
            Scheme::Commerce => "commerce",
            Scheme::Upto => "upto",
        };
        write!(f, "{s}")
    }
}
```

Then `is_escrow_scheme` checks for both:
```rust
pub fn is_escrow_scheme(s: Option<&str>) -> bool {
    matches!(s, Some("escrow") | Some("commerce"))
}
```

And anywhere we match `Scheme::Escrow` in Rust code, we also match `Scheme::Commerce`:
```rust
// In facilitator_local.rs SchemeMismatch check:
Scheme::Escrow | Scheme::Commerce => { /* escrow logic */ }
```

#### Step 2.3: Advertise commerce in /supported

**File**: `src/facilitator_local.rs` around line 219

In the existing escrow loop, push TWO entries per network/operator -- one with `Scheme::Escrow`, one with `Scheme::Commerce`:

```rust
for scheme in [Scheme::Escrow, Scheme::Commerce] {
    kinds.push(SupportedPaymentKind {
        x402_version: X402Version::V2,
        scheme,
        network: network.to_caip2(),
        extra: Some(escrow_extra.clone()),
    });
}
```

---

### Phase 3 - Handler Routing Updates

**Goal**: Ensure `Scheme::Commerce` routes to the same handlers as `Scheme::Escrow`.

#### Step 3.1: Update scheme mismatch checks

**File**: `src/chain/evm.rs` (if applicable)

Search for any `requirements.scheme` comparison against `Scheme::Escrow` and add `Scheme::Commerce`:
```rust
if payload.scheme != requirements.scheme {
    // This already uses enum comparison, which will work if both sides are Commerce
}
```

Actually, since the scheme comes from the request and both `Scheme::Escrow` and `Scheme::Commerce` deserialize correctly, the mismatch check will work as-is. If a merchant sends `"commerce"` in both payload and requirements, both deserialize to `Scheme::Commerce` and match.

The only case to watch: if payload says `"escrow"` but requirements says `"commerce"`. These would deserialize to different variants. But this should be a legitimate mismatch -- the merchant chose one scheme, the client must match it.

---

### Phase 4 - OpenAPI Documentation

**Goal**: Document the commerce scheme in Swagger.

#### Step 4.1: Update src/openapi.rs

Add commerce scheme to:
- Escrow lifecycle description (mention "commerce" as alias)
- Example payloads (add commerce variant)
- Scheme enum documentation

---

### Phase 5 - Tests

**Goal**: Verify commerce scheme works end-to-end.

#### Step 5.1: Add unit tests in operator.rs

Add test cases mirroring existing escrow tests but with `"scheme": "commerce"`:
- `test_parse_commerce_request_authorize`
- `test_parse_commerce_request_release`
- `test_parse_commerce_request_refund`

#### Step 5.2: Add is_escrow_scheme tests

```rust
#[test]
fn test_is_escrow_scheme() {
    assert!(is_escrow_scheme(Some("escrow")));
    assert!(is_escrow_scheme(Some("commerce")));
    assert!(!is_escrow_scheme(Some("exact")));
    assert!(!is_escrow_scheme(None));
}
```

#### Step 5.3: Verify serde round-trip

```rust
#[test]
fn test_commerce_scheme_deserialize() {
    let json = r#""commerce""#;
    let scheme: Scheme = serde_json::from_str(json).unwrap();
    assert_eq!(scheme, Scheme::Commerce);
    assert_eq!(serde_json::to_string(&scheme).unwrap(), r#""commerce""#);
}
```

---

### Phase 6 - Build and Deploy

#### Step 6.1: Version bump

```bash
curl -s https://facilitator.ultravioletadao.xyz/version
# Bump from current deployed version
```

#### Step 6.2: Format and lint

```bash
just format-all
just clippy-all
```

#### Step 6.3: Build Docker image

```bash
./scripts/fast-build.sh v<NEW_VERSION> --push
```

#### Step 6.4: Deploy to ECS

```bash
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

#### Step 6.5: Verify in production

```bash
# Check commerce appears in /supported
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.scheme == "commerce")'

# Verify escrow still works
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.scheme == "escrow")'
```

---

### Phase 7 - Coordination with Ali

#### Step 7.1: Notify Ali

Tell Ali:
- Commerce scheme is live on `facilitator.ultravioletadao.xyz`
- Both `"escrow"` and `"commerce"` are accepted
- `/supported` advertises both
- Temporary facilitator can be retired

#### Step 7.2: Ali switches FACILITATOR_URL

Ali updates Execution Market and arbiter example to use our facilitator URL instead of their temporary one.

#### Step 7.3: E2E validation

Run Ali's arbiter E2E test (BackTrackCo/arbiter-examples#3) against our facilitator:
- `/weather` (valid content) -> 200 -> arbiter PASS -> release
- `/garbage` (error JSON) -> 200 -> arbiter FAIL -> immediate refund

---

## Part 3: File Change Matrix

| File | Lines Changed | What |
|------|---------------|------|
| `src/types.rs` | ~8 | Add `Commerce` variant + Display arm |
| `src/payment_operator/operator.rs` | ~15 | Add constant + helper + update 4 comparisons |
| `src/payment_operator/mod.rs` | ~2 | Export `is_escrow_scheme` |
| `src/handlers.rs` | ~4 | Update 4 scheme comparisons |
| `src/facilitator_local.rs` | ~5 | Advertise Commerce in /supported |
| `src/openapi.rs` | ~10 | Document commerce scheme |
| `src/payment_operator/operator.rs` (tests) | ~40 | Commerce scheme test cases |
| **Total** | **~84 lines** | |

---

## Part 4: Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Scheme mismatch (escrow vs commerce) | Low | Medium | `is_escrow_scheme()` helper covers all comparison points |
| Missing a comparison location | Low | High | Exhaustive audit done (8 locations identified + verified) |
| /supported bloat (double entries) | None | None | Clients filter by scheme; having both is expected |
| Breaking existing escrow clients | None | None | `Scheme::Escrow` unchanged; commerce is additive |
| Serde deserialization conflict | None | None | Each variant has unique `rename`; no ambiguity |

---

## Part 5: Success Criteria

- [ ] `curl .../supported | jq '.kinds[] | select(.scheme == "commerce")' | head` returns entries
- [ ] `curl .../supported | jq '.kinds[] | select(.scheme == "escrow")' | head` still returns entries
- [ ] POST to `/verify` with `"scheme": "commerce"` routes to escrow handler
- [ ] POST to `/settle` with `"scheme": "commerce"` routes to escrow handler
- [ ] Ali's E2E test passes against our facilitator
- [ ] Ali retires temporary JS facilitator
