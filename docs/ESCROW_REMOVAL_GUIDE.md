# x402r Escrow Removal Guide

This document provides a complete checklist for removing all escrow/x402r functionality from the facilitator codebase. Follow this guide if you want to fully disable and remove the escrow feature without leaving any traces.

## Summary

The x402r escrow extension was added in v1.14.0 to support trustless refunds via escrow proxy contracts. Removing it requires changes to:

- **7 source files** in `src/`
- **2 ABI files** in `abi/`
- **1 test file** in `tests/`
- **2 documentation files** in `docs/`
- **1 landing page** (`static/index.html`)
- **1 Terraform config** in `terraform/`
- **1 environment example** (`.env.example`)
- **1 README section**

---

## Phase 1: Delete Standalone Files

These files can be deleted entirely:

### Source Files

| File | Lines | Description |
|------|-------|-------------|
| `src/escrow.rs` | ~915 | Main escrow module with CREATE3 computation, proxy verification, settlement logic |

### ABI Files

| File | Description |
|------|-------------|
| `abi/DepositRelay.json` | Proxy contract ABI for delegatecall |
| `abi/DepositRelayFactory.json` | Factory contract ABI for proxy deployment |

### Test Files

| File | Lines | Description |
|------|-------|-------------|
| `tests/escrow_integration.rs` | ~172 | Integration tests for CREATE3 computation, factory addresses, feature flag |

### Documentation Files

| File | Lines | Description |
|------|-------|-------------|
| `docs/X402R_ESCROW.md` | ~215 | Technical deep-dive documentation |
| `docs/X402R_ESCROW_TESTING.md` | ~289 | Testing guide with examples |

**Commands:**
```bash
rm -f src/escrow.rs
rm -f abi/DepositRelay.json
rm -f abi/DepositRelayFactory.json
rm -f tests/escrow_integration.rs
rm -f docs/X402R_ESCROW.md
rm -f docs/X402R_ESCROW_TESTING.md
```

---

## Phase 2: Remove Module References

### src/main.rs

**Line 46** - Remove module declaration:
```rust
// DELETE THIS LINE:
mod escrow;
```

### src/lib.rs

**Line 24** - Remove public module export:
```rust
// DELETE THIS LINE:
pub mod escrow;
```

---

## Phase 3: Update handlers.rs

### src/handlers.rs

Remove escrow settlement routing from `post_settle` function.

**Lines 911-1061** - Remove escrow detection and routing:

```rust
// DELETE THIS ENTIRE BLOCK (approximately lines 1026-1063):

        // Check for x402r escrow/refund extension
        if let Some(extensions) = envelope.extensions() {
            if extensions.contains_key("refund") {
                // Check if escrow feature is enabled
                if !crate::escrow::is_escrow_enabled() {
                    warn!("Escrow settlement requested but ENABLE_ESCROW is not set to true");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": "Escrow settlement is disabled. Set ENABLE_ESCROW=true to enable."
                        })),
                    ).into_response();
                }

                info!("Detected x402r refund extension, routing to escrow settlement");

                match crate::escrow::settle_with_escrow(body_str, &facilitator).await {
                    Ok(escrow_response) => {
                        info!("Escrow settlement complete");
                        return (StatusCode::OK, Json(escrow_response)).into_response();
                    }
                    Err(e) => {
                        error!(error = %e, "Escrow settlement failed");
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "success": false,
                                "errorReason": format!("Escrow error: {}", e)
                            })),
                        ).into_response();
                    }
                }
            }
        }
```

Also update the doc comment on line 911:
```rust
// BEFORE:
/// Also supports x402r escrow settlement when the `refund` extension is present.

// AFTER:
// (delete the line entirely)
```

---

## Phase 4: Update facilitator_local.rs

### src/facilitator_local.rs

Remove escrow-related comments and HasProviderMap implementation.

**Lines 54-61** - Remove escrow-related documentation:
```rust
// DELETE THESE COMMENTS:
    /// This is used by the escrow module to access network-specific providers
    /// for x402r escrow settlement.
```

**Lines 61-67** - Consider removing `HasProviderMap` trait implementation if no longer needed:
```rust
// EVALUATE: This impl may still be needed for other purposes
// Implement HasProviderMap to allow escrow module to access providers
impl HasProviderMap for FacilitatorLocal {
    fn provider_map(&self) -> &ProviderMap {
        &self.evm_provider_cache
    }
}
```

---

## Phase 5: Update provider_cache.rs

### src/provider_cache.rs

**Lines 55-56** - Remove escrow-related comments:
```rust
// DELETE THESE COMMENTS:
/// This is used by the escrow module to access network-specific providers
/// for x402r escrow settlement without coupling to the FacilitatorLocal type.
```

---

## Phase 6: Update types_v2.rs

### src/types_v2.rs

The x402r types are more complex because they're used for payload parsing. The types can be retained for backward compatibility or removed entirely.

**To fully remove x402r types, delete:**

| Lines | Type | Description |
|-------|------|-------------|
| 363-385 | `X402rAuthorization`, `X402rPayload` | Inner x402r payload types |
| 388-402 | `VerifyRequestX402r` | Top-level x402r verify request |
| 403-449 | `X402rPaymentPayloadNested`, `VerifyRequestX402rNested` | Nested format types |
| 451-541 | `impl VerifyRequestX402rNested` | Conversion methods |
| 543-637 | `impl VerifyRequestX402r` | Conversion methods |
| 640-650 | `X402rNested`, `X402r` variants in `VerifyRequestEnvelope` | Enum variants |
| 661-691 | Match arms for `X402r`, `X402rNested` | In impl blocks |

**If removing x402r types, update `VerifyRequestEnvelope`:**

```rust
// BEFORE:
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerifyRequestEnvelope {
    X402rNested(VerifyRequestX402rNested),
    V2(VerifyRequestV2),
    X402r(VerifyRequestX402r),
    V1(VerifyRequest),
}

// AFTER:
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerifyRequestEnvelope {
    V2(VerifyRequestV2),
    V1(VerifyRequest),
}
```

**Update all match statements** that reference `X402r` or `X402rNested` variants.

---

## Phase 7: Update static/index.html

### static/index.html

Remove escrow section from the landing page.

**Lines 1837-1855** - Delete the x402r Escrow endpoint card:
```html
<!-- DELETE THIS ENTIRE BLOCK: -->
                <!-- x402r Escrow Extension -->
                <div class="endpoint-card">
                    <h4 data-i18n="endpoints.group.escrow">x402r Escrow (Refund Support)</h4>
                    <div class="endpoint-item">
                        <span class="method method-post">POST</span>
                        <code>/settle</code>
                        <span class="endpoint-desc" data-i18n="endpoints.escrowSettle">Settle to escrow proxy when "refund" extension is present</span>
                    </div>
                    <p style="color: var(--text-muted); font-size: 0.8rem; margin: 0.5rem 0 0 0; padding-left: 1rem;" data-i18n="endpoints.escrowNote">
                        Add {"refund": {"window": 86400}} to extensions for trustless refund support via DepositRelay proxies.
                    </p>
                    <div style="margin-top: 0.5rem; padding-left: 1rem;">
                        <a href="https://github.com/coinbase/x402/issues/864" target="_blank"
                           style="color: var(--accent-color); font-size: 0.75rem; text-decoration: none;">
                            x402r Proposal
                        </a>
                        <a href="https://github.com/BackTrackCo/x402r-contracts" target="_blank"
                           style="color: var(--accent-color); font-size: 0.75rem; text-decoration: none; margin-left: 1rem;">
                            Contracts
                        </a>
                    </div>
                </div>
```

**Lines 1934-1937** - Remove English i18n translations:
```javascript
// DELETE THESE LINES from translations.en:
                "endpoints.group.escrow": "x402r Escrow (Refund Support)",
                "endpoints.escrowSettle": "Settle to escrow proxy when 'refund' extension is present",
                "endpoints.escrowNote": "Add {\"refund\": {\"window\": 86400}} to extensions for trustless refund support via DepositRelay proxies.",
```

**Lines 1973-1976** - Remove Spanish i18n translations:
```javascript
// DELETE THESE LINES from translations.es:
                "endpoints.group.escrow": "x402r Escrow (Soporte de Reembolsos)",
                "endpoints.escrowSettle": "Liquidar a proxy escrow cuando la extension 'refund' esta presente",
                "endpoints.escrowNote": "Agrega {\"refund\": {\"window\": 86400}} a extensions para soporte de reembolsos sin confianza via proxies DepositRelay.",
```

---

## Phase 8: Update Terraform Configuration

### terraform/modules/facilitator-service/main.tf

Remove `ENABLE_ESCROW` environment variable from ECS task definition.

Find and delete:
```hcl
{
  name  = "ENABLE_ESCROW"
  value = "true"
}
```

---

## Phase 9: Update Environment Example

### .env.example

Remove escrow-related environment variable documentation:

```bash
# DELETE THIS SECTION:
# Escrow/x402r Extension
ENABLE_ESCROW=false  # Set to true to enable escrow settlement
```

---

## Phase 10: Update README

### README.md

Remove the entire x402r Escrow section (lines 121-234 after previous edits).

Delete from:
```markdown
## x402r Escrow Extension (Trustless Refunds)
```

To (and including):
```markdown
- **x402r Contracts:** https://github.com/BackTrackCo/x402r-contracts
```

---

## Phase 11: Update CLAUDE.md

### CLAUDE.md

Remove references to escrow in project documentation:

1. Search for "escrow" and remove related instructions
2. Remove `/settle` escrow extension documentation
3. Update endpoint list to remove escrow mention

---

## Verification Checklist

After removal, verify:

- [ ] `cargo build --release` compiles without errors
- [ ] `cargo test` passes all remaining tests
- [ ] `cargo clippy --all-features` shows no warnings
- [ ] No references to "escrow", "x402r", "DepositRelay" remain (except in git history)
- [ ] Landing page renders correctly without escrow section
- [ ] `/settle` endpoint works for normal payments

**Verification commands:**
```bash
# Check for remaining references
grep -r "escrow" src/ --include="*.rs"
grep -r "x402r" src/ --include="*.rs"
grep -r "DepositRelay" src/ --include="*.rs"
grep -r "ENABLE_ESCROW" .

# Build and test
cargo build --release --features solana,near
cargo test
cargo clippy --all-features

# Check landing page
cargo run --release &
curl http://localhost:8080/ | grep -i escrow  # Should return nothing
```

---

## Files Summary

### Files to DELETE (6 files):

| File | Type |
|------|------|
| `src/escrow.rs` | Source |
| `abi/DepositRelay.json` | ABI |
| `abi/DepositRelayFactory.json` | ABI |
| `tests/escrow_integration.rs` | Test |
| `docs/X402R_ESCROW.md` | Docs |
| `docs/X402R_ESCROW_TESTING.md` | Docs |

### Files to EDIT (9 files):

| File | Changes |
|------|---------|
| `src/main.rs` | Remove `mod escrow` |
| `src/lib.rs` | Remove `pub mod escrow` |
| `src/handlers.rs` | Remove escrow routing (~50 lines) |
| `src/facilitator_local.rs` | Remove escrow comments |
| `src/provider_cache.rs` | Remove escrow comments |
| `src/types_v2.rs` | Remove x402r types (~280 lines) |
| `static/index.html` | Remove escrow UI section |
| `terraform/modules/facilitator-service/main.tf` | Remove ENABLE_ESCROW |
| `README.md` | Remove escrow documentation |

---

## Estimated Effort

- **Time:** 1-2 hours
- **Risk:** Low (escrow is isolated and feature-flagged)
- **Testing:** Run full test suite after removal

---

## Rollback

If you need to restore escrow functionality, use git to restore the files:

```bash
# Find the commit before removal
git log --oneline | head -20

# Restore specific files
git checkout <commit-hash> -- src/escrow.rs abi/DepositRelay.json abi/DepositRelayFactory.json tests/escrow_integration.rs docs/X402R_ESCROW.md docs/X402R_ESCROW_TESTING.md

# Or revert the removal commit entirely
git revert <removal-commit-hash>
```

---

## Version History

| Version | Date | Description |
|---------|------|-------------|
| 1.14.0 | 2024-12-24 | Initial escrow implementation |
| 1.14.7 | 2024-12-25 | Added nested format support |
| 1.14.8 | 2024-12-25 | Made resource field optional |
| 1.14.9 | 2024-12-25 | Fixed escrow parser for Ali's SDK format |

---

**Created:** 2024-12-25
**Last Updated:** 2024-12-25
**Author:** Claude Code (via Ultravioleta DAO)
