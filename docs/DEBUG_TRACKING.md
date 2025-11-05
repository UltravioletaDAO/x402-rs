# Facilitator Debug Logging Tracking

This document tracks all custom debug logging added to the facilitator for easy management and removal.

## Debug Flag Configuration

**Environment Variable**: `FACILITATOR_ENHANCED_DEBUG=true`

When set to `false` or unset, all enhanced debug logging is disabled.

## Enhanced Debug Logging Added

### 1. Transaction Submission Timing (Added: 2025-11-01)

**Location**: `src/chain/evm.rs`
**Flag Check**: `is_enhanced_debug_enabled()`
**Log Markers**:
- `[SETTLEMENT] Request`
- `[TX-CONFIRMED] Transaction on-chain`
- `[SETTLEMENT-SUCCESS]`

**Purpose**: Measure and log exact timing for transaction submission and confirmation.

**Lines Added**: ~30 lines in `settle()` function

---

### 2. EIP-3009 Authorization Validation (Added: 2025-11-01)

**Location**: `src/chain/evm.rs` (in validation logic)
**Flag Check**: `is_enhanced_debug_enabled()`
**Log Markers**:
- `================================================`
- `[VALIDATION] EIP-3009 transferWithAuthorization`
- Authorization Details section
- Signature Verification section
- Timestamp Validation section
- Nonce Validation section

**Purpose**: Log every field of the EIP-3009 authorization to catch type mismatches, timestamp errors, or signature issues.

**Lines Added**: ~50 lines in validation flow

---

### 3. Settlement Success/Failure Tracking (Added: 2025-11-01)

**Location**: `src/handlers.rs` in `post_settle()`
**Flag Check**: `is_enhanced_debug_enabled()`
**Log Markers**:
- `[SETTLEMENT] Request`
- `[SETTLEMENT-SUCCESS]`
- `[SETTLEMENT-FAILED]`
- `[CAUSE]` (automatic failure diagnosis)

**Purpose**: Comprehensive logging of settlement outcomes with context for diagnosing failures.

**Lines Added**: ~80 lines in settlement handler

---

### 4. Enhanced Debug Logging in post_settle Handler (Added: 2025-11-01)

**Location**: `src/handlers.rs` in `post_settle()`
**Flag Check**: Always enabled (uses `error!()` for visibility)
**Log Markers**:
- `=== SETTLE REQUEST DEBUG ===`
- `✓ Deserialization SUCCEEDED`
- `✗ Deserialization FAILED`
- Field-by-field type analysis

**Purpose**: Debug payload deserialization issues.

**Lines Added**: ~100 lines in payload validation

---

## Helper Functions Added

### `is_enhanced_debug_enabled()`
**Location**: `src/config.rs` or utility module
**Purpose**: Check if enhanced debug logging is enabled via environment variable

### `format_usdc_amount(value: U256) -> String`
**Location**: `src/utils.rs`
**Purpose**: Format USDC micro-units to human-readable dollar amount

### `timestamp_to_readable(unix_timestamp: u64) -> String`
**Location**: `src/utils.rs`
**Purpose**: Convert Unix timestamp to RFC3339 readable format

### `extract_revert_reason(error: &Error) -> Option<String>`
**Location**: `src/chain/evm.rs`
**Purpose**: Decode contract revert reasons from errors

---

## How to Disable All Enhanced Debug Logging

### Option 1: Environment Variable (Recommended)
```bash
# In .env file
FACILITATOR_ENHANCED_DEBUG=false

# Or unset it
unset FACILITATOR_ENHANCED_DEBUG
```

### Option 2: Code-Level Disable
Edit `src/debug_utils.rs` and change the default:
```rust
pub fn is_enhanced_debug_enabled() -> bool {
    env::var("FACILITATOR_ENHANCED_DEBUG")
        .unwrap_or_else(|_| "false".to_string()) // Changed to false
        .eq_ignore_ascii_case("true")
}
```

### Option 3: Remove All Debug Code
Use this grep command to find all enhanced debug locations:
```bash
grep -r "is_enhanced_debug_enabled" src/
grep -r "\[SETTLEMENT\]\|\[VALIDATION\]\|\[TX-CONFIRMED\]" src/
```

---

## Log Level Configuration

To see all debug logs in production:
```bash
RUST_LOG=x402_rs=debug
```

To see only enhanced debug logs:
```bash
RUST_LOG=x402_rs=info,x402_rs::chain::evm=debug
```

---

## Testing Enhanced Debug Logging

### Successful Payment Test
Expected log sequence:
```
[VALIDATION] EIP-3009 transferWithAuthorization
[SETTLEMENT] Request - Payer: 0x6bdc... -> Seller: 0x4dFB..., Amount: $0.010000
[TX-CONFIRMED] Transaction on-chain - TX: 0xe6f1..., Block: 37621104, Total Time: 23.45s
[SETTLEMENT-SUCCESS] TX: 0xe6f1..., Payer: 0x6bdc... -> Seller: 0x4dFB..., Amount: $0.010000, Duration: 24.68s
```

### Failed Payment Test
Expected log sequence:
```
[VALIDATION] EIP-3009 transferWithAuthorization
[SETTLEMENT] Request - Payer: 0x20Bb... -> Seller: 0x4dFB..., Amount: $0.010000
[SETTLEMENT-FAILED] Payer: 0x20Bb... -> Seller: 0x4dFB..., Amount: $0.010000, TX: 0x...
  [CAUSE] Transaction reverted on-chain
```

---

## Metrics We Can Track

With this logging in place, we can extract:
1. Average transaction submission time
2. Average confirmation time
3. Settlement success rate
4. Common failure causes distribution
5. Network-specific latency issues

---

## Removal Checklist

When disabling enhanced debug logging:
- [ ] Set `FACILITATOR_ENHANCED_DEBUG=false` in environment
- [ ] Remove timing instrumentation from `src/chain/evm.rs`
- [ ] Remove authorization logging from validation flow
- [ ] Remove settlement tracking from `src/handlers.rs`
- [ ] Remove helper functions if unused elsewhere
- [ ] Remove debug configuration code
- [ ] Update this document with removal date
- [ ] Test that normal logging still works
