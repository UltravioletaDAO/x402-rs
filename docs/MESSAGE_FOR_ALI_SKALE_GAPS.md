Hey Ali,

We verified all 21 x402r CREATE3 contracts on SKALE Base (chain 1187947933) against the RPC. 18 are deployed and matching Base mainnet. 3 are missing:

| Contract | Expected Address (CREATE3) | SKALE Base Status |
|----------|---------------------------|-------------------|
| PaymentOperatorFactory | `0xdc41F932dF2d22346F218E4f5650694c650ab863` | NOT DEPLOYED |
| RefundRequestFactory | `0x9cD87Bb58553Ef5ad90Ed6260EBdB906a50D6b83` | NOT DEPLOYED |
| RefundRequestEvidenceFactory | `0x3769Be76BBEa31345A2B2d84EF90683E9A377e00` | NOT DEPLOYED |

These 3 block us from deploying a PaymentOperator on SKALE. Everything else is ready on our side -- the facilitator already has SKALE payment support (USDC.e, EIP-3009, legacy tx) and we're adding escrow scheme support.

Looks like `DeployAllChain.s.sol` doesn't include RefundRequestFactory or RefundRequestEvidenceFactory (they were added later in commits from March 16-18). And PaymentOperatorFactory might have failed during the batch deploy since it depends on the escrow + protocolFeeConfig addresses as constructor args.

CreateX and Multicall3 are both deployed on SKALE, so you should be able to run the CREATE3 deploys the same way you did for the other 18 contracts.

Once those 3 are up, we'll deploy our PaymentOperator via the factory and have SKALE escrow fully operational.

Also one quick question: did the `PaymentInfo` struct change in the CREATE3 redeployment? We have the 12-field version (operator, payer, receiver, token, maxAmount, preApprovalExpiry, authorizationExpiry, refundExpiry, minFeeBps, maxFeeBps, feeReceiver, salt). If anything changed we need to update our ABI.

Thanks,
Ultravioleta DAO (x402-rs facilitator)
