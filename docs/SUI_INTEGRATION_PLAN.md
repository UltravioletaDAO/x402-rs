# Sui Blockchain Integration Plan

> **Status**: Research Complete - Ready for Implementation
> **Created**: 2025-11-26
> **Estimated Effort**: 4-5 weeks
> **Priority**: Deferred (recommend waiting 6-12 months for SDK maturity)

## Executive Summary

Integrating Sui blockchain into the x402-rs payment facilitator is **technically feasible** and can follow patterns already established in the Solana implementation. Both chains use serialized transactions as payloads and require the facilitator to act as a gas/fee sponsor.

### Key Decision

**Recommendation: DEFER 6-12 months** due to:
- Sui Rust SDK still in beta (no stable crates.io release)
- Must use git dependencies which complicates version management
- gRPC SDK migration in progress (JSON-RPC being deprecated)

However, if proceeding now, the implementation is straightforward by following Solana patterns.

---

## USDC on Sui

### Contract Addresses

| Network | Address | Notes |
|---------|---------|-------|
| **Mainnet** | `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC` | Native Circle USDC |
| **Testnet** | `0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC` | Native Circle USDC |

**Important**: Do NOT use wUSDC (Wormhole-bridged) - only use native Circle USDC.

### References
- [Circle Announcement](https://www.circle.com/blog/now-available-native-usdc-on-sui)
- [USDC Contract Addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)

---

## Architecture Comparison

### Solana vs Sui vs EVM

| Aspect | Solana (Implemented) | Sui (Proposed) | EVM (Implemented) |
|--------|---------------------|----------------|-------------------|
| **User creates** | Full serialized transaction | Full serialized PTB | Authorization params only |
| **User pre-signs** | Yes | Yes | Yes (EIP-712) |
| **Facilitator role** | Fee payer (signer position 0) | Gas sponsor | Relayer |
| **Serialization** | Bincode → Base64 | BCS → Base64 | JSON typed data |
| **Validation** | Transaction introspection | PTB introspection | Signature verification |
| **Facilitator signs** | As fee payer | As gas sponsor | N/A (just submits) |
| **Replay protection** | Blockhash + nonce | Object versioning + epoch | Nonce in authorization |

### Key Insight

Solana and Sui have **very similar models**:
1. User constructs a complete transaction
2. User signs their portion
3. Facilitator validates via introspection
4. Facilitator signs as fee/gas payer
5. Transaction is submitted

This means we can **reuse the Solana architecture patterns** for Sui.

---

## Technical Implementation

### Dependencies

```toml
# Cargo.toml - Sui dependencies (git-based, no stable crates.io release)
[dependencies]
sui-sdk = { git = "https://github.com/mystenlabs/sui", package = "sui-sdk" }
sui-types = { git = "https://github.com/mystenlabs/sui", package = "sui-types" }
sui-keys = { git = "https://github.com/mystenlabs/sui", package = "sui-keys" }
shared-crypto = { git = "https://github.com/mystenlabs/sui", package = "shared-crypto" }
bcs = "0.1"  # Binary Canonical Serialization
```

**Warning**: Pin to specific git commits for reproducible builds.

### Type Definitions

#### Network Enum (src/network.rs)
```rust
pub enum Network {
    // ... existing networks ...

    /// Sui mainnet
    #[serde(rename = "sui")]
    Sui,
    /// Sui testnet
    #[serde(rename = "sui-testnet")]
    SuiTestnet,
}
```

#### Address Type (src/types.rs)
```rust
pub enum MixedAddress {
    Evm(Address),
    Solana(Pubkey),
    Sui(SuiAddress),  // NEW
    Offchain(String),
}
```

#### Payload Type (src/types.rs)
```rust
/// Sui payment payload - mirrors Solana's structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactSuiPayload {
    /// Base64-encoded BCS-serialized TransactionData
    pub transaction: String,
}

pub enum ExactPaymentPayload {
    Evm(ExactEvmPayload),
    Solana(ExactSolanaPayload),
    Sui(ExactSuiPayload),  // NEW
}
```

### Provider Implementation (src/chain/sui.rs)

```rust
use sui_sdk::SuiClient;
use sui_types::base_types::{ObjectID, SuiAddress};
use sui_types::transaction::{TransactionData, Command};
use sui_types::crypto::{SuiKeyPair, Signature};

pub struct SuiChain {
    pub network: Network,
}

pub struct SuiProvider {
    keypair: Arc<SuiKeyPair>,
    chain: SuiChain,
    sui_client: Arc<SuiClient>,
    max_gas_budget: u64,
}

impl SuiProvider {
    pub fn try_new(
        keypair: SuiKeyPair,
        rpc_url: String,
        network: Network,
        max_gas_budget: u64,
    ) -> Result<Self, FacilitatorLocalError> {
        // Similar to SolanaProvider::try_new
    }

    /// Validate gas budget in transaction
    pub fn verify_gas_budget(
        &self,
        tx: &TransactionData,
    ) -> Result<u64, FacilitatorLocalError> {
        let gas_budget = tx.gas_data().budget;
        if gas_budget > self.max_gas_budget {
            return Err(FacilitatorLocalError::DecodingError(
                "gas budget exceeds facilitator maximum".to_string(),
            ));
        }
        Ok(gas_budget)
    }

    /// Introspect PTB commands to find and validate transfer
    pub fn verify_transfer_command(
        &self,
        tx: &TransactionData,
        requirements: &PaymentRequirements,
    ) -> Result<TransferDetails, FacilitatorLocalError> {
        // Similar pattern to Solana's verify_transfer_instruction
        // 1. Find TransferObjects or Pay command
        // 2. Verify recipient matches requirements.pay_to
        // 3. Verify amount matches requirements.max_amount_required
        // 4. Verify asset is USDC
    }

    /// Ensure sponsor (facilitator) is not being exploited
    pub fn verify_sponsor_safety(
        &self,
        tx: &TransactionData,
    ) -> Result<(), FacilitatorLocalError> {
        // Similar to Solana's fee_payer safety check
        // Ensure facilitator's objects are not being transferred
    }

    async fn verify_transfer(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerifyTransferResult, FacilitatorLocalError> {
        // 1. Deserialize PTB from BCS
        // 2. Verify gas budget
        // 3. Verify transfer command
        // 4. Verify sponsor safety
        // 5. Dry-run simulation
    }
}

impl Facilitator for SuiProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let verification = self.verify_transfer(request).await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let verification = self.verify_transfer(request).await?;

        // Sign as sponsor
        let signature = self.keypair.sign(&verification.transaction);

        // Submit transaction
        let response = self.sui_client
            .quorum_driver_api()
            .execute_transaction_block(
                Transaction::new(verification.transaction, vec![signature]),
                SuiTransactionBlockResponseOptions::default(),
                Some(ExecuteTransactionRequestType::WaitForLocalExecution),
            )
            .await?;

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.into(),
            transaction: Some(TransactionHash::Sui(response.digest)),
            network: self.network(),
        })
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: self.signer_address(),
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}
```

### Environment Configuration

```bash
# .env.example additions
RPC_URL_SUI=https://fullnode.mainnet.sui.io:443
RPC_URL_SUI_TESTNET=https://fullnode.testnet.sui.io:443

# Sui wallet (Base64 encoded keypair)
SUI_PRIVATE_KEY_MAINNET=
SUI_PRIVATE_KEY_TESTNET=

# Gas budget limits
X402_SUI_MAX_GAS_BUDGET_SUI=50000000  # 0.05 SUI in MIST
X402_SUI_MAX_GAS_BUDGET_SUI_TESTNET=100000000
```

---

## Sponsored Transactions on Sui

### How It Works

Sui has **protocol-level sponsored transactions** (unlike EVM where it's token-contract level).

#### Transaction Structure
```
TransactionData {
    kind: TransactionKind::ProgrammableTransaction(PTB),
    sender: user_address,
    gas_data: GasData {
        payment: vec![gas_coin_object],  // Sponsor's gas coins
        owner: sponsor_address,           // Facilitator
        price: gas_price,
        budget: gas_budget,
    },
    expiration: TransactionExpiration::Epoch(current_epoch + 1),
}
```

#### Signature Requirements
1. **User signature**: Signs the full TransactionData (authorizing the transfer)
2. **Sponsor signature**: Signs the full TransactionData (authorizing gas payment)

Both signatures are required for execution.

### Flow for x402

```
1. User constructs TransactionData:
   - PTB with USDC transfer command
   - Gas data with facilitator as sponsor
   - User signs

2. User sends to facilitator (/verify):
   - Base64 BCS-encoded TransactionData
   - User's signature

3. Facilitator validates:
   - Deserialize and introspect PTB
   - Verify transfer details match requirements
   - Verify sponsor safety
   - Dry-run simulation

4. Facilitator settles (/settle):
   - Re-validate
   - Sign as sponsor
   - Submit to Sui network
   - Return transaction digest
```

---

## Security Considerations

### 1. Sponsor Safety (Critical)

The facilitator must ensure it's not being tricked into:
- Transferring its own tokens
- Paying excessive gas
- Executing malicious Move calls

**Validation checklist**:
- [ ] Gas budget within limits
- [ ] Only expected commands in PTB (TransferObjects for USDC)
- [ ] No commands involving facilitator's objects
- [ ] Recipient matches payment requirements
- [ ] Amount matches payment requirements

### 2. Replay Protection

Sui uses:
- **Object versioning**: Each object has a version; using an old version fails
- **Transaction expiration**: Epoch-based expiry

The facilitator should:
- Verify transaction epoch is current or next
- Not cache/replay transactions

### 3. Signature Verification

Sui supports multiple signature schemes:
- Ed25519
- ECDSA Secp256k1
- ECDSA Secp256r1
- Multisig
- zkLogin

The facilitator should validate signatures match the declared sender.

---

## Implementation Checklist

### Phase 1: Setup (Week 1)
- [ ] Add sui-sdk git dependency to Cargo.toml
- [ ] Pin to specific commit hash
- [ ] Create `src/chain/sui.rs` skeleton
- [ ] Add `Network::Sui` and `Network::SuiTestnet`
- [ ] Add `MixedAddress::Sui` variant
- [ ] Add `ExactSuiPayload` to types.rs
- [ ] Add RPC env vars to from_env.rs

### Phase 2: Provider (Week 2-3)
- [ ] Implement `SuiChain` struct
- [ ] Implement `SuiProvider` struct
- [ ] Implement `try_new()` with RPC client setup
- [ ] Implement `verify_gas_budget()`
- [ ] Implement `verify_transfer_command()`
- [ ] Implement `verify_sponsor_safety()`
- [ ] Implement `verify_transfer()` combining all checks

### Phase 3: Facilitator Trait (Week 3-4)
- [ ] Implement `Facilitator::verify()`
- [ ] Implement `Facilitator::settle()`
- [ ] Implement `Facilitator::supported()`
- [ ] Add to `FacilitatorLocal` provider cache

### Phase 4: Testing (Week 4-5)
- [ ] Unit tests for transaction introspection
- [ ] Integration tests on Sui testnet
- [ ] Test with native USDC transfers
- [ ] Load testing
- [ ] Security review

### Phase 5: Documentation & Deployment
- [ ] Update CLAUDE.md with Sui networks
- [ ] Update .env.example
- [ ] Update static/index.html with Sui network card
- [ ] Add Sui logo to static/
- [ ] Update /supported endpoint
- [ ] Deploy to production (testnet first)

---

## Estimated Lines of Code

| Component | LOC | Notes |
|-----------|-----|-------|
| src/chain/sui.rs | ~500 | Main provider implementation |
| src/network.rs | ~30 | Network enum additions |
| src/types.rs | ~20 | Payload and address types |
| src/from_env.rs | ~20 | RPC env configuration |
| src/handlers.rs | ~10 | Logo handler |
| static/index.html | ~50 | Network card |
| **Total** | **~630** | Comparable to Solana (~843 LOC) |

---

## Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| SDK breaking changes | High | Pin to specific git commit |
| gRPC migration disruption | Medium | Monitor Sui updates, prepare migration |
| PTB introspection complexity | Medium | Extensive testing, limit allowed commands |
| Sponsor exploitation | High | Strict validation, gas budget limits |
| Low adoption | Low | Start with testnet, monitor usage |

---

## References

### Official Documentation
- [Sui Rust SDK](https://docs.sui.io/references/rust-sdk)
- [Sponsored Transactions](https://docs.sui.io/guides/developer/sui-101/sponsor-txn)
- [Programmable Transaction Blocks](https://docs.sui.io/concepts/transactions/prog-txn-blocks)
- [Offline Signing](https://docs.sui.io/concepts/cryptography/transaction-auth/offline-signing)

### USDC
- [Native USDC on Sui](https://www.circle.com/blog/now-available-native-usdc-on-sui)
- [Contract Addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)

### Code Examples
- [Sui Rust Examples](https://github.com/MystenLabs/sui/tree/main/crates/sui-sdk/examples)
- [Sponsored Transaction Example](https://docs.sui.io/guides/developer/sui-101/sponsor-txn#user-initiated)

---

## Conclusion

Sui integration is **feasible and follows established patterns** from the Solana implementation. The main challenges are:

1. **SDK maturity**: No stable crates.io release yet
2. **Sponsored transaction flow**: Slightly different from Solana's fee payer model
3. **BCS serialization**: Different from Solana's bincode

However, the core architecture (transaction introspection, facilitator as sponsor, user pre-signing) is nearly identical to Solana, making this a moderate-complexity integration.

**Recommendation**: Wait 6-12 months for SDK stability, then implement following this plan.
