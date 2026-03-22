# x402r SKALE Integration — Master Plan

**Date**: 2026-03-21
**Status**: PLANNING
**Goal**: Full x402r escrow scheme support on SKALE Base (chain 1187947933)
**Dependency**: Ali (x402r) must deploy 3 missing factory contracts on SKALE

---

## Context

Ali deployed x402r contracts on SKALE Base using CREATE3 (same addresses on all chains). 18 of 21 contracts verified on-chain. 3 factory contracts are missing (blocker). Our facilitator needs code updates to support SKALE as an escrow network and migrate to the new CREATE3 unified address model.

### Key Discovery: CREATE3 Unification

Ali's new SDK uses **unified addresses across ALL chains** via CREATE3. Our code has **stale per-chain addresses** from the old deployment. The migration is straightforward because:

1. Infrastructure contracts (escrow, tokenCollector, protocolFeeConfig) are now identical on every chain
2. PaymentOperator addresses remain per-chain (deployed by factory per-merchant)
3. The facilitator receives addresses via `requirements.extra` at request time — it doesn't hardcode them for the core settle/verify flow
4. Legacy tx handling for SKALE already works (`is_eip1559() -> false`)

### On-Chain Verification (21 contracts, SKALE Base)

| Status | Count | Details |
|--------|-------|---------|
| DEPLOYED | 18 | authCaptureEscrow, tokenCollector, protocolFeeConfig, usdcTvlLimit, arbiterRegistry, receiverRefundCollector, 9 factories, 3 condition singletons |
| NOT DEPLOYED | 3 | factories.paymentOperator, factories.refundRequest, factories.refundRequestEvidence |

The 3 missing contracts are **BLOCKERS** — Ali must deploy them before we can create PaymentOperators on SKALE.

---

## PHASE 1: Unblock (Ali's Side)

**Owner**: Ali (x402r)
**Dependency**: None
**Parallelizable**: Yes (independent of Phase 2)

### Task 1.1: Ali deploys 3 missing factory contracts on SKALE Base

Ali needs to deploy via CREATE3 (CreateX factory):

| Contract | Address (must match CREATE3) |
|----------|-----|
| factories.paymentOperator | `0xdc41F932dF2d22346F218E4f5650694c650ab863` |
| factories.refundRequest | `0x9cD87Bb58553Ef5ad90Ed6260EBdB906a50D6b83` |
| factories.refundRequestEvidence | `0x3769Be76BBEa31345A2B2d84EF90683E9A377e00` |

**Verification** (run after Ali deploys):
```bash
for addr in "0xdc41F932dF2d22346F218E4f5650694c650ab863" "0x9cD87Bb58553Ef5ad90Ed6260EBdB906a50D6b83" "0x3769Be76BBEa31345A2B2d84EF90683E9A377e00"; do
  echo -n "$addr: "
  curl -s "https://skale-base.skalenodes.com/v1/base" -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getCode\",\"params\":[\"$addr\",\"latest\"],\"id\":1}" \
    | python3 -c "import sys,json; r=json.load(sys.stdin); print('DEPLOYED' if len(r['result'])>2 else 'MISSING')"
done
```

### Task 1.2: Confirm PaymentInfo ABI struct unchanged

Ask Ali: did the `PaymentInfo` struct change in the CREATE3 redeployment? Our current ABI has:

```
PaymentInfo { operator, payer, receiver, token, maxAmount, preApprovalExpiry,
              authorizationExpiry, refundExpiry, minFeeBps, maxFeeBps,
              feeReceiver, salt }
```

If the struct changed, our ABI encoding will silently produce wrong calldata and all `authorize()` calls will revert.

---

## PHASE 2: Migrate to CREATE3 Unified Addresses

**Owner**: Us (facilitator)
**Dependency**: None (can start before Phase 1 completes)
**Parallelizable**: Yes

### Task 2.1: Refactor `addresses.rs` — unified CREATE3 constants

Replace the 10 per-chain submodules with a single `create3` module containing unified addresses:

**File**: `src/payment_operator/addresses.rs`

```rust
pub mod create3 {
    use super::*;
    // Infrastructure (same on ALL chains)
    pub const ESCROW: Address = address!("e050bB89eD43BB02d71343063824614A7fb80B77");
    pub const TOKEN_COLLECTOR: Address = address!("cE66Ab399EDA513BD12760b6427C87D6602344a7");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("7e868A42a458fa2443b6259419aA6A8a161E08c8");
    pub const FACTORY: Address = address!("dc41F932dF2d22346F218E4f5650694c650ab863");
    pub const REFUND_REQUEST: Address = address!("9cD87Bb58553Ef5ad90Ed6260EBdB906a50D6b83");
    // New contracts (not yet used in our code, captured for future)
    pub const USDC_TVL_LIMIT: Address = address!("0F1F26719219CfAdC8a1C80D2216098A0534a091");
    pub const ARBITER_REGISTRY: Address = address!("1c2d7d5978d46a943FA98aC9a649519C1424FB3e");
    pub const RECEIVER_REFUND_COLLECTOR: Address = address!("E5500a38BE45a6C598420fbd7867ac85EC451A07");
}
```

### Task 2.2: Simplify `OperatorAddresses::for_network()`

All networks now share the same infrastructure. Only `payment_operators` vec differs per chain:

```rust
fn shared(operators: Vec<Address>) -> Self {
    Self {
        escrow: create3::ESCROW,
        factory: create3::FACTORY,
        token_collector: create3::TOKEN_COLLECTOR,
        protocol_fee_config: create3::PROTOCOL_FEE_CONFIG,
        refund_request: create3::REFUND_REQUEST,
        payment_operators: operators,
    }
}
```

Then each match arm becomes:
```rust
Network::Base => Some(Self::shared(vec![
    address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb"),
    address!("030353642B936c9D4213caD7BcB0fB8a1489cBe5"),
])),
Network::SkaleBase => Some(Self::shared(vec![
    // Empty until PaymentOperator deployed in Phase 3
])),
```

### Task 2.3: Add SKALE to `ESCROW_NETWORKS`

```rust
pub const ESCROW_NETWORKS: &[Network] = &[
    // ... existing 10 ...
    Network::SkaleBase,
];
```

### Task 2.4: Update helper functions

All `escrow_for_network()`, `factory_for_network()`, `token_collector_for_network()`, etc. — add `Network::SkaleBase` arm pointing to `create3::` constants.

### Task 2.5: Update tests

- `test_escrow_networks_count`: 10 -> 11
- `test_is_supported`: add `assert!(is_supported(Network::SkaleBase))`
- `test_base_mainnet_addresses`: update expected addresses to CREATE3
- Add `test_skale_base_addresses`

### Task 2.6: Backward compatibility assessment

**Risk**: Existing escrows on old addresses (Base, Ethereum, etc.) would break if we replace infrastructure addresses.

**Decision needed**: Ask Ali if any active/uncaptured escrows exist on the old contract addresses.
- If NO active escrows: clean cutover to CREATE3 (simplest)
- If YES: add `legacy_escrow: Option<Address>` to `OperatorAddresses` and accept both in `validate_addresses()`

**SKALE has zero risk** — no existing escrows on SKALE.

---

## PHASE 3: Deploy PaymentOperator on SKALE

**Owner**: Us (facilitator)
**Dependency**: Phase 1 (Ali's 3 contracts must be deployed first)
**Parallelizable**: No

### Task 3.1: Update `deploy_operator.py` for SKALE

Add SKALE to the deployment script:

```python
FACTORY_ADDRESSES = {
    # ... existing ...
    "skale-base": "0xdc41F932dF2d22346F218E4f5650694c650ab863",
}
DEFAULT_RPCS = {
    # ... existing ...
    "skale-base": "https://skale-base.skalenodes.com/v1/base",
}
```

Ensure the script handles legacy transactions (SKALE has no EIP-1559). May need `--legacy` flag or auto-detection.

### Task 3.2: Deploy testnet operator (SKALE Base Sepolia)

```bash
python scripts/deploy_operator.py \
  --network skale-base-sepolia \
  --fee-recipient 0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8 \
  --private-key $EVM_PRIVATE_KEY_TESTNET
```

Permissionless config (all conditions = address(0)) for testing.

### Task 3.3: Deploy mainnet operator (SKALE Base) — Fase 5 config

```bash
python scripts/deploy_operator.py \
  --network skale-base \
  --fee-recipient 0x103040545AC5031A11E8C03dd11324C7333a13C7 \
  --config fase5 \
  --private-key $EVM_PRIVATE_KEY_MAINNET
```

Fase 5 = 1300bps (13% fee), OR release (payer|facilitator), facilitator-only refund.

Alternative via `cast` if script doesn't support legacy tx:
```bash
cast send --legacy \
  --rpc-url https://skale-base.skalenodes.com/v1/base \
  --private-key $PRIVATE_KEY \
  0xdc41F932dF2d22346F218E4f5650694c650ab863 \
  "deployOperator((address,address,address,address,address,address,address,address,address,address,address,address))" \
  "(0x103040545AC5031A11E8C03dd11324C7333a13C7,0x0,...)"
```

### Task 3.4: Register deployed operator address in code

Add the deployed address to `OperatorAddresses::for_network(Network::SkaleBase)`:

```rust
Network::SkaleBase => Some(Self::shared(vec![
    address!("DEPLOYED_OPERATOR_ADDRESS"),
])),
```

---

## PHASE 4: Build, Test, Deploy

**Owner**: Us
**Dependency**: Phase 2 + Phase 3
**Parallelizable**: No

### Task 4.1: Compile and run tests

```bash
cargo test -p x402-rs -- payment_operator
cargo test -p x402-rs -- escrow
```

All tests must pass with new CREATE3 addresses.

### Task 4.2: Version bump

```bash
# Check deployed version first
curl -s https://facilitator.ultravioletadao.xyz/version
# Bump Cargo.toml from deployed version
```

### Task 4.3: Build and push Docker image

```bash
./scripts/fast-build.sh v1.40.0 --push
```

### Task 4.4: Deploy to ECS

```bash
aws ecs describe-task-definition --task-definition facilitator-production --region us-east-2 --query 'taskDefinition' | \
  jq 'del(.taskDefinitionArn, .revision, .status, .requiresAttributes, .placementConstraints, .compatibilities, .registeredAt, .registeredBy) | .containerDefinitions[0].image = "518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.40.0"' > /tmp/task-def-v1.40.0.json && \
  aws ecs register-task-definition --cli-input-json file:///tmp/task-def-v1.40.0.json --region us-east-2 && \
  aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2
```

### Task 4.5: Production verification

```bash
# Version
curl -s https://facilitator.ultravioletadao.xyz/version

# SKALE in escrow supported networks
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[] | select(.network | contains("skale")) | select(.scheme == "escrow")]'

# Escrow state query on SKALE
curl -X POST https://facilitator.ultravioletadao.xyz/escrow/state \
  -H "Content-Type: application/json" \
  -d '{"network": "skale-base", "operatorAddress": "DEPLOYED_OPERATOR", "paymentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"}'
```

---

## PHASE 5: End-to-End Test with Ali

**Owner**: Us + Ali
**Dependency**: Phase 4
**Parallelizable**: No

### Task 5.1: Testnet round-trip

1. Ali's x402r SDK creates an escrow payment on SKALE testnet
2. Our facilitator receives the `POST /settle` with `scheme: "escrow"` and `network: "skale-base-sepolia"`
3. Facilitator calls `PaymentOperator.authorize()` on SKALE (legacy tx, CREDIT gas)
4. Verify escrow state via `POST /escrow/state`
5. Ali's system calls `charge()` or `release()`

### Task 5.2: Mainnet validation

Same flow on SKALE Base mainnet with real USDC.e.

### Task 5.3: Update handoff document

Update `docs/SKALE_GAS_HANDOFF_FOR_ALI.md` with:
- Deployed PaymentOperator address on SKALE
- Escrow endpoints confirmed working
- Any integration notes from testing

---

## Blocker Summary

| Blocker | Owner | Status |
|---------|-------|--------|
| 3 factory contracts missing on SKALE | Ali | WAITING |
| PaymentInfo ABI struct unchanged? | Ali (confirm) | WAITING |
| Active escrows on old addresses? | Ali (confirm) | WAITING |

---

## Risk Matrix

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| ABI struct changed | HIGH — silent encoding mismatch, all authorize() calls revert | LOW | Task 1.2: ask Ali to confirm |
| Active escrows on old addresses | MEDIUM — breaks existing escrows on other chains | LOW | Task 2.6: backward compat strategy |
| Factory deployment fails on SKALE | MEDIUM — blocks Phase 3 | LOW | Ali handles via CREATE3/CreateX |
| Legacy tx in deploy script | LOW — script may fail | MEDIUM | Task 3.1: add --legacy flag or use cast |
| CREDIT balance insufficient | ZERO — 40 CREDIT, gasless | ZERO | Already funded |

---

## Timeline Estimate

| Phase | Can Start | Depends On |
|-------|-----------|------------|
| Phase 1 (Ali deploys) | NOW | Nothing |
| Phase 2 (CREATE3 migration) | NOW | Nothing |
| Phase 3 (Deploy operator) | After Phase 1 | Ali's 3 contracts |
| Phase 4 (Build + deploy) | After Phase 2 + 3 | Code + operator address |
| Phase 5 (E2E test) | After Phase 4 | Deployed facilitator |

**Phase 1 and Phase 2 run in parallel.**
