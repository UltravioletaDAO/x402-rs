# NEAR Meta-Transaction Research: NEP-366 vs NEP-413

**Date**: 2025-12-04
**Status**: Research Complete
**Conclusion**: Direct bridging is NOT possible, but alternatives exist

## Executive Summary

The proposal to accept NEP-413 signatures and bridge them to NEP-366 meta-transactions is **NOT directly possible** due to fundamental cryptographic incompatibilities. However, there are viable alternative approaches.

## The Core Problem

Browser wallets (MyNearWallet, Meteor, HERE) support **NEP-413** (message signing), but the facilitator needs **NEP-366** (meta-transaction signing). These use different signing formats that produce incompatible signatures.

## Technical Deep Dive

### NEP-366 Signing Format (Meta-Transactions)

Source: [nearcore/core/primitives/src/action/delegate.rs](https://github.com/near/nearcore/blob/master/core/primitives/src/action/delegate.rs)

```rust
// How SignedDelegateAction is created
pub fn sign(signer: &Signer, delegate_action: DelegateAction) -> Self {
    let signature = signer.sign(delegate_action.get_nep461_hash().as_bytes());
    Self { delegate_action, signature }
}

// How verification works
pub fn verify(&self) -> bool {
    let hash = self.delegate_action.get_nep461_hash();
    self.signature.verify(hash.as_ref(), &self.delegate_action.public_key)
}
```

The `get_nep461_hash()` method wraps the DelegateAction in a SignableMessage:

```rust
pub fn get_nep461_hash(&self) -> CryptoHash {
    let signable = SignableMessage::new(self, SignableMessageType::DelegateAction);
    let bytes = borsh::to_vec(&signable).expect("Failed to serialize");
    hash(&bytes)
}
```

**Discriminant value**: `(1 << 30) + 366 = 1,073,742,190` (on-chain range)

**What gets signed**:
```
SHA256(borsh(SignableMessage {
    discriminant: 1073742190,
    msg: DelegateAction { sender_id, receiver_id, actions, nonce, max_block_height, public_key }
}))
```

### NEP-413 Signing Format (Message Signing)

Source: [NEP-413 Specification](https://github.com/near/NEPs/blob/master/neps/nep-0413.md)

**Discriminant value**: `(1 << 31) + 413 = 2,147,484,061` (off-chain range)

**What gets signed**:
```
SHA256(borsh(2147484061 as u32) || borsh(Payload {
    message: String,
    nonce: [u8; 32],
    recipient: String,
    callbackUrl: Option<String>
}))
```

### Why They're Incompatible

| Aspect | NEP-366 | NEP-413 |
|--------|---------|---------|
| Discriminant | 1,073,742,190 | 2,147,484,061 |
| Data Structure | `SignableMessage<DelegateAction>` | `Payload { message, nonce, recipient, callbackUrl }` |
| Range | On-chain (1<<30 to 1<<31) | Off-chain (1<<31 to u32::MAX) |
| Purpose | Meta-transaction execution | Off-chain authentication |

**The same private key signing the same intent produces completely different signatures.**

## Why Bridging Fails

### Proposed Approach
```
1. Frontend sends: { delegateAction, nep413Signature, nep413Params }
2. Facilitator verifies NEP-413 signature
3. Facilitator creates SignedDelegateAction with the NEP-413 signature
4. Submit to NEAR network
```

### Why It Fails

When the NEAR runtime processes `Action::Delegate(signed_delegate_action)`:

1. Runtime extracts `delegate_action` from `signed_delegate_action`
2. Runtime computes `hash = delegate_action.get_nep461_hash()` (NEP-366 format)
3. Runtime verifies `signature` against `hash`
4. **FAILS** because signature was created over NEP-413 hash, not NEP-366 hash

The on-chain verification is immutable - we cannot bypass it.

## Alternative Solutions

### Option 1: User Pays Gas (Simplest)

**How it works**:
- Frontend creates regular `ft_transfer` transaction
- User signs and submits directly (pays ~0.00001 NEAR gas)
- Facilitator not involved in transaction signing

**Pros**: Works today, no changes needed
**Cons**: Not truly gasless (though gas is tiny ~$0.001)

**Implementation**: Remove meta-transaction flow, use direct wallet transaction.

### Option 2: Function Call Access Keys

**How it works**:
1. User creates a limited access key that can ONLY call `ft_transfer` on USDC contract
2. User adds facilitator's public key as this limited key
3. Facilitator uses this key to call `ft_transfer` on behalf of user

**Setup transaction** (one-time per user):
```javascript
// User must sign this once
await wallet.signAndSendTransaction({
  receiverId: userAccountId,
  actions: [{
    type: 'AddKey',
    params: {
      publicKey: facilitatorPublicKey,
      accessKey: {
        permission: {
          FunctionCall: {
            receiverId: 'usdc.near',
            methodNames: ['ft_transfer'],
            allowance: '1000000000000000000000000' // 1 NEAR max gas
          }
        }
      }
    }
  }]
});
```

**Pros**: True gasless after setup
**Cons**: Requires one-time setup transaction per user, trust model

### Option 3: Escrow/Pre-deposit Pattern

**How it works**:
1. User deposits USDC to facilitator contract
2. User signs NEP-413 message authorizing transfer
3. Facilitator verifies and executes transfer from escrow

**Pros**: Works with existing wallet support
**Cons**: Requires deposits, capital lockup, trust

### Option 4: Permit Contract (Custom)

Deploy a custom contract that mimics EIP-2612 permits:

**Contract logic**:
```rust
pub fn permit_transfer(
    &mut self,
    from: AccountId,
    to: AccountId,
    amount: U128,
    deadline: u64,
    nep413_signature: String,
    nep413_params: Nep413Params,
) {
    // Verify NEP-413 signature over transfer intent
    // Call ft_transfer_call from pre-approved allowance
}
```

**Requires**:
- Deploy custom permit contract
- User must approve permit contract to spend their USDC (one-time setup)

**Pros**: Standard signature format
**Cons**: Complex, requires setup approval

### Option 5: Wait for Wallet Support

Eventually wallets may implement `signDelegateAction` (NEP-366 signing).

**Status**: No wallet has announced plans for this feature.

## Recommendation

For **immediate implementation** with minimal changes:

### Short-term: Option 1 (User Pays Gas)
- Implement direct transaction signing for NEAR
- Gas cost is negligible (~$0.001)
- Works with all existing wallets
- No trust assumptions

### Medium-term: Option 2 (Access Keys)
If true gasless is critical:
- Implement one-time access key delegation
- Users opt-in to "enable gasless" which adds facilitator key
- Facilitator can then execute transfers on their behalf

## Code Changes Required

### For Option 1 (Recommended)

**Frontend (pixel-mar)**:
```typescript
// Instead of creating SignedDelegateAction
// Create and submit regular transaction

const tx = await wallet.signAndSendTransaction({
  receiverId: USDC_CONTRACT,
  actions: [{
    type: 'FunctionCall',
    params: {
      methodName: 'ft_transfer',
      args: {
        receiver_id: recipient,
        amount: amount.toString(),
        memo: null
      },
      gas: '30000000000000',
      deposit: '1'
    }
  }]
});
```

**Facilitator (x402-rs)**:
- Keep existing NEP-366 code for future use
- Add fallback endpoint for direct transaction verification
- Monitor transaction status instead of submitting

### For Option 2 (Access Keys)

**Frontend**:
```typescript
// One-time setup
await setupFacilitatorAccessKey(facilitatorPublicKey);

// Then for each payment
const intent = { receiver, amount, nonce };
const signature = await wallet.signMessage(JSON.stringify(intent));
await fetch('/settle', { body: { intent, signature } });
```

**Facilitator**:
```rust
// Verify NEP-413 signature of intent
// Use stored access key to call ft_transfer
// This requires storing access keys per user
```

## References

- [NEP-366: Meta Transactions](https://github.com/near/NEPs/pull/366)
- [NEP-413: Message Signing](https://github.com/near/NEPs/blob/master/neps/nep-0413.md)
- [NEAR Meta Transactions Documentation](https://docs.near.org/concepts/abstraction/meta-transactions)
- [Pagoda Relayer Reference](https://github.com/near/pagoda-relayer-rs)
- [SignableMessage Implementation](https://github.com/near/nearcore/blob/master/core/primitives/src/signable_message.rs)
- [SignedDelegateAction Implementation](https://github.com/near/nearcore/blob/master/core/primitives/src/action/delegate.rs)

## Conclusion

**Direct NEP-413 to NEP-366 bridging is cryptographically impossible.** The NEAR runtime performs on-chain signature verification using NEP-366 format, which cannot be bypassed.

The recommended path forward is:
1. **Immediate**: Use direct transactions (user pays negligible gas)
2. **Future**: Consider access key delegation for true gasless experience
3. **Long-term**: Monitor wallet ecosystem for NEP-366 signing support
