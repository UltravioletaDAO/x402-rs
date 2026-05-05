# Commerce Scheme SDK Integration Handoff

**Date**: 2026-04-05
**From**: Facilitator (x402-rs)
**To**: Python SDK, TypeScript SDK
**Facilitator version**: v1.43.0 (deployed)

---

## Context

The facilitator now supports `"commerce"` as a scheme alongside `"escrow"`. Both are functionally identical -- same contracts, same ABI, same ERC-3009 flow. The `"commerce"` alias was introduced by x402r for marketplace integrations (Execution Market, arbiter examples).

**Production is live.** The `/supported` endpoint already returns entries with `scheme: "commerce"` on all 11 escrow networks:

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[] | select(.scheme == "commerce")] | length'
# Returns: 14
```

The SDKs need to accept `"commerce"` as a valid scheme value so clients can use it without hitting validation errors.

---

## What Changed in the Facilitator

1. `Scheme` enum gained a `Commerce` variant (serializes to `"commerce"`)
2. All verify/settle handlers accept both `"escrow"` and `"commerce"` via `is_escrow_scheme()` helper
3. `/supported` advertises both schemes per network (14 escrow + 14 commerce entries)
4. OpenAPI docs updated

**Existing behavior is unchanged.** `"exact"` and `"escrow"` work exactly as before.

---

## What Each SDK Needs to Change

### The Pattern

Both SDKs currently restrict `scheme` to a narrow set of literal values. The fix is the same in both: widen the accepted values to include `"commerce"`.

**Important: defaults stay as `"exact"`.** The `buildPaymentRequirements()` / `create_authorization()` functions should keep defaulting to `"exact"` for standard payments. `"escrow"` and `"commerce"` are only used by advanced escrow flows (AdvancedEscrowClient / AdvancedPaymentOperator).

---

### Python SDK (v0.21.0)

#### File: `src/uvd_x402_sdk/models.py`

**Change 1 -- Widen scheme Literal (3 locations):**

```python
# PaymentPayload (line ~212):
# BEFORE:
scheme: Literal["exact"] = Field(default="exact", description="Payment scheme (only 'exact' supported)")
# AFTER:
scheme: Literal["exact", "escrow", "commerce"] = Field(default="exact", description="Payment scheme")

# PaymentRequirements (line ~282):
# BEFORE:
scheme: Literal["exact"] = Field(default="exact")
# AFTER:
scheme: Literal["exact", "escrow", "commerce"] = Field(default="exact")

# PaymentRequirementsV2 (line ~326):
# BEFORE:
scheme: Literal["exact"] = Field(default="exact")
# AFTER:
scheme: Literal["exact", "escrow", "commerce"] = Field(default="exact")
```

**Change 2 -- Update the validator (lines ~225-230):**

```python
# BEFORE:
@field_validator("scheme")
@classmethod
def validate_scheme(cls, v: str) -> str:
    if v != "exact":
        raise ValueError(f"Unsupported scheme: {v}. Only 'exact' is supported")
    return v

# AFTER:
SUPPORTED_SCHEMES = {"exact", "escrow", "commerce"}

@field_validator("scheme")
@classmethod
def validate_scheme(cls, v: str) -> str:
    if v not in SUPPORTED_SCHEMES:
        raise ValueError(f"Unsupported scheme: {v}. Supported: {SUPPORTED_SCHEMES}")
    return v
```

**Tests to add** (in `tests/test_client.py`):

```python
def test_commerce_scheme_accepted():
    """Commerce scheme should pass validation."""
    payload_data = {
        "x402Version": 1,
        "scheme": "commerce",
        "network": "eip155:84532",
        "payload": { ... }  # valid payload
    }
    # Should not raise
    payload = PaymentPayload(**payload_data)
    assert payload.scheme == "commerce"

def test_escrow_scheme_accepted():
    """Escrow scheme should pass validation."""
    payload_data = {
        "x402Version": 1,
        "scheme": "escrow",
        "network": "eip155:84532",
        "payload": { ... }
    }
    payload = PaymentPayload(**payload_data)
    assert payload.scheme == "escrow"

def test_invalid_scheme_rejected():
    """Unknown schemes should still fail."""
    with pytest.raises(ValidationError):
        PaymentPayload(scheme="unknown", ...)
```

#### Files NOT to touch:
- `client.py` -- `create_authorization()` keeps defaulting to `"exact"` (correct)
- `advanced_escrow.py` -- already sends `"escrow"` (correct, and now `"commerce"` also works)
- `escrow.py` -- EscrowClient is API-level, doesn't depend on the scheme string
- `response.py` -- 402 response builder keeps using `"exact"` (correct for standard payments)

---

### TypeScript SDK (v2.36.0)

#### File: `src/types/index.ts`

**Change 1 -- Add scheme type:**

```typescript
/** Supported x402 payment schemes */
export type X402Scheme = 'exact' | 'escrow' | 'commerce';
```

**Change 2 -- Update header interfaces:**

```typescript
// X402HeaderV1 (line ~476):
// BEFORE:
scheme: 'exact';
// AFTER:
scheme: X402Scheme;

// X402HeaderV2 (line ~496):
// BEFORE:
scheme: 'exact';
// AFTER:
scheme: X402Scheme;
```

#### File: `src/backend/index.ts`

**Change 3 -- Update PaymentRequirements interface (line ~83):**

```typescript
// BEFORE:
scheme: 'exact';
// AFTER:
scheme: X402Scheme;
```

Import `X402Scheme` from types if needed.

**Tests to add** (in `src/backend/index.test.ts`):

```typescript
it('should accept commerce scheme in payment header', () => {
  const header = encodePaymentHeader({
    x402Version: 2,
    scheme: 'commerce',
    network: 'eip155:84532',
    payload: { /* valid payload */ },
  });
  const decoded = decodePaymentHeader(header);
  expect(decoded.scheme).toBe('commerce');
});

it('should accept escrow scheme in payment header', () => {
  const header = encodePaymentHeader({
    x402Version: 2,
    scheme: 'escrow',
    network: 'eip155:84532',
    payload: { /* valid payload */ },
  });
  const decoded = decodePaymentHeader(header);
  expect(decoded.scheme).toBe('escrow');
});

it('should default to exact scheme in buildPaymentRequirements', () => {
  const requirements = buildPaymentRequirements({ /* params */ });
  expect(requirements.scheme).toBe('exact');
});
```

#### Files NOT to touch:
- `buildPaymentRequirements()` -- keeps defaulting to `"exact"` (correct)
- `AdvancedPaymentOperator` -- already sends `"escrow"` (correct)
- `FacilitatorClient.getSupported()` -- returns raw JSON, already sees commerce entries
- All providers (EVM, Solana, Stellar, NEAR, Algorand, Sui) -- keep using `"exact"`

---

## What Already Works Without Changes

These components handle `"commerce"` correctly today with zero modifications:

| Component | Why it works |
|-----------|-------------|
| `/supported` parsing | Both SDKs return raw JSON -- commerce entries are already visible |
| `/verify` and `/settle` calls | They send whatever scheme is in the payload -- facilitator routes it |
| AdvancedEscrowClient (Python) | Sends `"escrow"` -- still works, and `"commerce"` will too after type fix |
| AdvancedPaymentOperator (TS) | Sends `"escrow"` -- still works, and `"commerce"` will too after type fix |
| EscrowClient (both SDKs) | API-level escrow, doesn't depend on the x402 scheme string |

---

## Verification

After updating each SDK, verify against production:

```bash
# 1. Check that /supported returns commerce entries
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.scheme == "commerce") | .network' | head -5

# 2. Verify the SDK can parse a commerce scheme payload without error
# (use your SDK's PaymentPayload/X402Header constructor with scheme="commerce")

# 3. Confirm default behavior unchanged
# (buildPaymentRequirements / create_authorization still defaults to "exact")
```

---

## Summary

| SDK | Files to change | Lines changed | Complexity |
|-----|----------------|---------------|------------|
| Python | `models.py` + tests | ~25 | Widen Literal + update validator |
| TypeScript | `types/index.ts` + `backend/index.ts` + tests | ~18 | Add type union + update interfaces |

Both changes are purely type/validation updates. Zero new logic. Defaults unchanged.
