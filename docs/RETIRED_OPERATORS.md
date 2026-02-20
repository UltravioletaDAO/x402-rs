# Retired PaymentOperators

Operator addresses removed from `src/payment_operator/addresses.rs` but still deployed on-chain.
If an old escrow needs release/refund, temporarily re-add the address to the `payment_operators` vec.

## Base Mainnet (eip155:8453)

| Address | Phase | Conditions | Fee | Retired |
|---------|-------|-----------|-----|---------|
| `0xd5149049e7c212ce5436a9581b4307EB9595df95` | Fase 3 clean | OR(Payer\|Facilitator) release+refund | 0 (feeCalculator=address(0)) | 2026-02-13 |
| `0x8D3DeCBAe68F6BA6f8104B60De1a42cE1869c2E6` | Fase 3 | OR(Payer\|Facilitator) | 1% (feeCalculator) | 2026-02-13 |
| `0xb9635f544665758019159c04c08a3d583dadd723` | Fase 2 | Facilitator-only | N/A | 2026-02-13 |

**Note:** `0xd514...df95` retired due to frontrunning vulnerability (Ali report): OrCondition on refund allows payer to call refundInEscrow() directly, bypassing facilitator.

## Current Active Operators

| Address | Phase | Conditions | Fee |
|---------|-------|-----------|-----|
| `0x271f9fa7f8907aCf178CCFB470076D9129D8F0Eb` | Fase 5 trustless fee split | OR(Payer\|Facilitator) release, FacilitatorOnly refund | 13% (StaticFeeCalculator 1300bps) |
| `0x030353642B936c9D4213caD7BcB0fB8a1489cBe5` | Fase 4 secure | OR(Payer\|Facilitator) release, FacilitatorOnly refund | 0 (feeCalculator=address(0)) |
