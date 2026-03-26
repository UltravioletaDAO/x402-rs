# x402r SKALE Final Integration Plan

**Date**: 2026-03-26
**Status**: READY TO EXECUTE
**Blocker resolved**: Ali redeployed factories with Shanghai EVM for SKALE compatibility
**Branch**: Ali's `A1igator/sync-abis-data-param`

---

## What Changed

Ali redeployed all factory contracts and condition singletons with `evm_version = "shanghai"`
to fix the SKALE Cancun opcode incompatibility (TSTORE/TLOAD). Core contracts
(authCaptureEscrow, tokenCollector, protocolFeeConfig) kept the same addresses.

16 contract addresses changed. PaymentOperator can now be deployed on SKALE.

---

## Phase 1: Update Facilitator addresses.rs

**Owner**: Us
**Dependency**: None -- addresses are live on-chain
**Files**: `src/payment_operator/addresses.rs`

### Task 1.1: Update CREATE3 module with new addresses

All 16 changed addresses need updating in the `create3` module (used by SKALE):

```rust
pub mod create3 {
    // Core (UNCHANGED)
    pub const ESCROW: Address = address!("e050bB89eD43BB02d71343063824614A7fb80B77");
    pub const TOKEN_COLLECTOR: Address = address!("cE66Ab399EDA513BD12760b6427C87D6602344a7");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("7e868A42a458fa2443b6259419aA6A8a161E08c8");
    pub const ARBITER_REGISTRY: Address = address!("1c2d7d5978d46a943FA98aC9a649519C1424FB3e");
    pub const RECEIVER_REFUND_COLLECTOR: Address = address!("E5500a38BE45a6C598420fbd7867ac85EC451A07");

    // ALL BELOW CHANGED (new Shanghai-compatible deploy)
    pub const USDC_TVL_LIMIT: Address = address!("6CAcA05D19312d28787e93ad4249508ED11198be");
    pub const FACTORY_PAYMENT_OPERATOR: Address = address!("A13AD07eD53BFF6c4e9e6478C3A8FFA4D096B5A3");
    pub const FACTORY_ESCROW_PERIOD: Address = address!("Cf84F213d6e1b2E2dc0DbCBd7d81FaAC305d4E96");
    pub const FACTORY_FREEZE: Address = address!("af6700833bf414BEde7d450f9c6772e2FE76B21d");
    pub const FACTORY_STATIC_FEE_CALCULATOR: Address = address!("83B94258Daa50Dd08aED72e0Cda1daCC20286F52");
    pub const FACTORY_STATIC_ADDRESS_CONDITION: Address = address!("f9739BB422C93A9705cC636BA9D35B97F721e782");
    pub const FACTORY_AND_CONDITION: Address = address!("57d33f001a0d880Ca9e53e578c55CA74baB5C36A");
    pub const FACTORY_OR_CONDITION: Address = address!("efaD31Ab2a17092Bb4350C84324D59C80CeBB9F4");
    pub const FACTORY_NOT_CONDITION: Address = address!("8FE9EDE9a786e613723922aB9f512F54DAEfE3A8");
    pub const FACTORY_RECORDER_COMBINATOR: Address = address!("60C1492fbB1A53F5d968Ad6FDFA6b7672Bc6a34c");
    pub const FACTORY_SIGNATURE_CONDITION: Address = address!("99F11e8b407dAc9BCBf40B869D35071D74FE56f4");
    pub const FACTORY_REFUND_REQUEST: Address = address!("7996b1E7B5B28AF85093dcE3AE73b128133D3715");
    pub const FACTORY_REFUND_REQUEST_EVIDENCE: Address = address!("a454D7e0D521176c998309E4E6828156870EDf4B");
    pub const CONDITION_PAYER: Address = address!("c321156210E9c2D135454290dc13ca7A1A7533C6");
    pub const CONDITION_RECEIVER: Address = address!("d14242a812F9C7C81869F01867453e571cacEaba");
    pub const CONDITION_ALWAYS_TRUE: Address = address!("27E1576D4C7C5A6Ee919CB456f2284026177e9c6");
}
```

Note: Legacy per-chain addresses for existing networks (Base, Ethereum, etc.) stay UNCHANGED.
Only the `create3` module (used by SKALE) gets updated.

### Task 1.2: Update SKALE escrow entry in OperatorAddresses

The SKALE entry now uses the new factory address:
```rust
Network::SkaleBase => Some(Self {
    escrow: create3::ESCROW,
    factory: create3::FACTORY_PAYMENT_OPERATOR, // new: 0xA13AD07e...
    payment_operators: vec![], // Will be populated after Phase 2
    token_collector: create3::TOKEN_COLLECTOR,
    protocol_fee_config: create3::PROTOCOL_FEE_CONFIG,
    refund_request: create3::FACTORY_REFUND_REQUEST, // new: 0x7996b1E7...
}),
```

### Task 1.3: Update tests, compile, run tests

### Task 1.4: Commit + build + deploy facilitator

---

## Phase 2: Deploy PaymentOperator on SKALE

**Owner**: Us (or Ali)
**Dependency**: Phase 1 (facilitator must have correct factory address)
**Tool**: Ali's x402r-sdk `deployMarketplaceOperator()` or `cast`

Now that the factory (`0xA13AD07e...`) is Shanghai-compatible, `deployOperator()` should work on SKALE.

### Task 2.1: Deploy via Ali's SDK (recommended)

Ask Ali to deploy a PaymentOperator for us on SKALE Base using his SDK:
```typescript
await deployMarketplaceOperator(walletClient, publicClient, {
  chainId: 1187947933,
  feeRecipient: '0x103040545AC5031A11E8C03dd11324C7333a13C7', // our facilitator
  arbiter: '0x...', // ask Ali for arbiter address
  escrowPeriodSeconds: 604800n, // 7 days
});
```

### Task 2.2: Register deployed operator address

Once deployed, add the operator address to `addresses.rs`:
```rust
Network::SkaleBase => Some(Self::with_operators(vec![
    address!("DEPLOYED_OPERATOR_ADDRESS"),
])),
```

### Task 2.3: Rebuild + deploy facilitator with operator address

---

## Phase 3: Verify End-to-End on SKALE

**Owner**: Us
**Dependency**: Phase 2

### Task 3.1: Test escrow state query
```bash
curl -X POST https://facilitator.ultravioletadao.xyz/escrow/state \
  -H "Content-Type: application/json" \
  -d '{"network":"skale-base","operatorAddress":"DEPLOYED_OPERATOR","paymentHash":"0x00..."}'
```

### Task 3.2: Test escrow appears in /supported
```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[] | select(.network | contains("skale")) | select(.scheme == "escrow")]'
```

---

## Phase 4: Update SDKs

**Owner**: SDK agents (IRC)
**Dependency**: Phase 2 (need operator address)

### Task 4.1: TypeScript SDK
- Add SKALE escrow config with new operator address
- Verify createHonoMiddleware works with escrow scheme on SKALE
- Publish new version

### Task 4.2: Python SDK
- Add SKALE escrow config
- Verify @require_payment works with escrow on SKALE
- Publish new version

---

## Phase 5: Execution Market on SKALE

**Owner**: Execution Market agent + us
**Dependency**: Phase 3 + 4

### Task 5.1: Execution Market uses SKALE for escrow payments
- Configure Execution Market to use `network: "skale-base"` for worker payments
- Use new operator address
- Test task assignment -> escrow authorize -> work completion -> release flow

### Task 5.2: ERC-8004 reputation on SKALE
- Already live (v1.40.1)
- Register agents on SKALE Identity Registry
- Submit feedback to SKALE Reputation Registry
- All zero gas cost

---

## New CREATE3 Address Reference

### Unchanged (5)
| Contract | Address |
|----------|---------|
| authCaptureEscrow | `0xe050bB89eD43BB02d71343063824614A7fb80B77` |
| tokenCollector | `0xcE66Ab399EDA513BD12760b6427C87D6602344a7` |
| protocolFeeConfig | `0x7e868A42a458fa2443b6259419aA6A8a161E08c8` |
| arbiterRegistry | `0x1c2d7d5978d46a943FA98aC9a649519C1424FB3e` |
| receiverRefundCollector | `0xE5500a38BE45a6C598420fbd7867ac85EC451A07` |

### Changed (16 -- Shanghai recompile)
| Contract | New Address |
|----------|-------------|
| usdcTvlLimit | `0x6CAcA05D19312d28787e93ad4249508ED11198be` |
| factories.paymentOperator | `0xA13AD07eD53BFF6c4e9e6478C3A8FFA4D096B5A3` |
| factories.escrowPeriod | `0xCf84F213d6e1b2E2dc0DbCBd7d81FaAC305d4E96` |
| factories.freeze | `0xaf6700833bf414BEde7d450f9c6772e2FE76B21d` |
| factories.staticFeeCalculator | `0x83B94258Daa50Dd08aED72e0Cda1daCC20286F52` |
| factories.staticAddressCondition | `0xf9739BB422C93A9705cC636BA9D35B97F721e782` |
| factories.andCondition | `0x57d33f001a0d880Ca9e53e578c55CA74baB5C36A` |
| factories.orCondition | `0xefaD31Ab2a17092Bb4350C84324D59C80CeBB9F4` |
| factories.notCondition | `0x8FE9EDE9a786e613723922aB9f512F54DAEfE3A8` |
| factories.recorderCombinator | `0x60C1492fbB1A53F5d968Ad6FDFA6b7672Bc6a34c` |
| factories.signatureCondition | `0x99F11e8b407dAc9BCBf40B869D35071D74FE56f4` |
| factories.refundRequest | `0x7996b1E7B5B28AF85093dcE3AE73b128133D3715` |
| factories.refundRequestEvidence | `0xa454D7e0D521176c998309E4E6828156870EDf4B` |
| conditions.payer | `0xc321156210E9c2D135454290dc13ca7A1A7533C6` |
| conditions.receiver | `0xd14242a812F9C7C81869F01867453e571cacEaba` |
| conditions.alwaysTrue | `0x27E1576D4C7C5A6Ee919CB456f2284026177e9c6` |

### Important Note on Legacy Networks
Existing networks (Base, Ethereum, Polygon, etc.) keep their OLD per-chain addresses.
The new addresses are CREATE3 unified but ONLY apply to SKALE (and future new networks).
When Ali merges this branch to main AND existing operators are retired, we can migrate
existing networks too (see docs/plans/create3-full-migration-plan.md).
