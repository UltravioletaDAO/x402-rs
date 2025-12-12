# Implementation Plan: Add signDelegateAction to MyNearWallet

**Date**: 2025-12-04
**Status**: Feasibility Assessment Complete
**Verdict**: FEASIBLE - Library support already exists

## Executive Summary

Adding `signDelegateAction` support to MyNearWallet is **highly feasible** because:
1. `near-api-js` already provides `buildDelegateAction()` and `signDelegateAction()` functions
2. MyNearWallet already implements `signMessage()` (NEP-413) which is similar in complexity
3. The wallet has access to user's private keys (stored in localStorage)

This would be the **first browser wallet** to support NEP-366 meta-transactions, enabling gasless payments for x402 and the broader NEAR ecosystem.

## Technical Analysis

### What Already Exists in near-api-js

```javascript
import { buildDelegateAction, signDelegateAction } from '@near-js/transactions';

// Build the delegate action
const delegateAction = buildDelegateAction({
    actions,                    // Array of actions (e.g., ft_transfer)
    maxBlockHeight: BigInt(currentHeight) + BigInt(100),
    nonce: BigInt(accessKeyNonce) + 1n,
    publicKey,
    receiverId,                 // e.g., "usdc.near"
    senderId: accountId,        // e.g., "alice.near"
});

// Sign it (this is what the wallet needs to do)
const { signedDelegateAction } = await signDelegateAction({
    delegateAction,
    signer: {
        sign: async (message) => {
            // Wallet uses its stored private key here
            const { signature } = await signer.signMessage(message, senderId, networkId);
            return signature;
        },
    }
});
```

### What Needs to Be Added to MyNearWallet

#### 1. New Wallet Method: `signDelegateAction`

**Input Parameters**:
```typescript
interface SignDelegateActionParams {
    receiverId: string;           // Contract to call (e.g., "usdc.near")
    actions: Action[];            // Actions to execute
    maxBlockHeight?: bigint;      // Optional, defaults to current + 100
    nonce?: bigint;               // Optional, auto-fetched from chain
}
```

**Output**:
```typescript
interface SignedDelegateActionResult {
    signedDelegateAction: SignedDelegateAction;  // Borsh-serializable
    signature: string;                            // Base58 or Base64
    publicKey: string;
}
```

#### 2. New URL Route Handler

MyNearWallet uses URL-based communication. Need to add:
- Route: `/sign-delegate-action`
- Query params: Serialized DelegateAction details
- Callback: Return signed result to calling app

#### 3. UI Flow

1. App redirects to: `https://app.mynearwallet.com/sign-delegate-action?...`
2. Wallet shows approval screen:
   ```
   ┌─────────────────────────────────────┐
   │  Approve Meta-Transaction           │
   │                                     │
   │  App: 402milly.xyz                  │
   │  Action: Transfer 1.00 USDC        │
   │  To: merchant.near                  │
   │                                     │
   │  Note: A relayer will submit this  │
   │  transaction. You pay NO gas fees. │
   │                                     │
   │  [Cancel]          [Approve]       │
   └─────────────────────────────────────┘
   ```
3. On approve: Sign and return `SignedDelegateAction`
4. Redirect back to app with signed data

## Implementation Steps

### Phase 1: Core Signing Logic (1-2 days)

**File**: `packages/frontend/src/utils/signing/delegateAction.ts`

```typescript
import { buildDelegateAction, signDelegateAction } from '@near-js/transactions';
import { serialize } from 'borsh';

export async function createSignedDelegateAction(
    accountId: string,
    receiverId: string,
    actions: Action[],
    signer: Signer,
    connection: Connection
): Promise<SignedDelegateAction> {
    // Get current block height
    const block = await connection.provider.block({ finality: 'final' });
    const blockHeight = BigInt(block.header.height);

    // Get access key nonce
    const publicKey = await signer.getPublicKey(accountId, connection.networkId);
    const accessKey = await connection.provider.query({
        request_type: 'view_access_key',
        finality: 'final',
        account_id: accountId,
        public_key: publicKey.toString(),
    });

    // Build delegate action
    const delegateAction = buildDelegateAction({
        actions,
        maxBlockHeight: blockHeight + 100n,
        nonce: BigInt(accessKey.nonce) + 1n,
        publicKey,
        receiverId,
        senderId: accountId,
    });

    // Sign it
    const { signedDelegateAction } = await signDelegateAction({
        delegateAction,
        signer: {
            sign: async (message: Uint8Array) => {
                const { signature } = await signer.signMessage(
                    message,
                    accountId,
                    connection.networkId
                );
                return signature;
            },
        }
    });

    return signedDelegateAction;
}
```

### Phase 2: URL Route Handler (1 day)

**File**: `packages/frontend/src/routes/SignDelegateAction.tsx`

```typescript
// Parse incoming request
const params = new URLSearchParams(window.location.search);
const receiverId = params.get('receiverId');
const actions = JSON.parse(params.get('actions'));
const callbackUrl = params.get('callbackUrl');

// After user approves
const signedDelegateAction = await createSignedDelegateAction(...);
const serialized = Buffer.from(serialize(SCHEMA, signedDelegateAction)).toString('base64');

// Redirect back
window.location.href = `${callbackUrl}?signedDelegateAction=${serialized}`;
```

### Phase 3: Approval UI (1 day)

- Copy existing transaction approval UI
- Modify messaging to indicate "meta-transaction" / "gasless"
- Show relayer info if provided

### Phase 4: Wallet Selector Integration (1 day)

Add method to wallet-selector module:

**File**: `packages/my-near-wallet/src/lib/my-near-wallet.ts`

```typescript
async signDelegateAction(params: SignDelegateActionParams): Promise<SignedDelegateAction> {
    // Build URL with serialized params
    const url = new URL('/sign-delegate-action', this.walletUrl);
    url.searchParams.set('receiverId', params.receiverId);
    url.searchParams.set('actions', JSON.stringify(params.actions));

    // Redirect and wait for callback
    return this.requestSignature(url);
}
```

## Files to Modify/Create

### New Files
```
packages/frontend/src/
├── utils/signing/delegateAction.ts      # Core signing logic
├── routes/SignDelegateAction/
│   ├── index.tsx                        # Main route component
│   ├── SignDelegateActionContainer.tsx  # Container with state
│   └── SignDelegateActionForm.tsx       # Approval UI
└── components/DelegateActionPreview.tsx # Action preview component
```

### Modified Files
```
packages/frontend/src/
├── routes/index.tsx                     # Add new route
├── utils/wallet.ts                      # Export new method
└── translations/en.json                 # Add UI strings
```

## Risks & Mitigations

### Risk 1: Security Review Required
Meta-transactions introduce new attack vectors (replay attacks, nonce issues).

**Mitigation**:
- Use `maxBlockHeight` with reasonable TTL (~100 blocks = ~2 minutes)
- Validate nonce is recent
- Clearly show user what they're signing

### Risk 2: UX Complexity
Users may not understand "meta-transaction" concept.

**Mitigation**:
- Use simple messaging: "Approve gasless transaction"
- Show clear preview of what will happen
- Indicate who pays gas (the relayer)

### Risk 3: Breaking Changes to Wallet Selector
Need to add new method to wallet interface.

**Mitigation**:
- Make method optional in interface
- Apps should feature-detect support
- Coordinate with wallet-selector maintainers

## Testing Plan

1. **Unit Tests**: Signing logic produces valid SignedDelegateAction
2. **Integration Tests**: Full flow from app -> wallet -> callback
3. **E2E Tests**:
   - Happy path: Sign and submit to relayer
   - Rejection: User cancels
   - Error: Invalid params
4. **Security Audit**: Review for replay/nonce attacks

## Contribution Strategy

### Option A: Direct PR to MyNearWallet
- Fork https://github.com/mynearwallet/my-near-wallet
- Implement feature
- Submit PR with detailed description
- Work with maintainers on review

### Option B: Fork and Deploy
If upstream is unresponsive:
- Fork and maintain our own version
- Deploy at `wallet.ultravioletadao.xyz`
- Integrate with wallet-selector as custom wallet

### Recommended: Option A First
1. Open issue explaining use case (x402 gasless payments)
2. Propose implementation plan
3. Gauge maintainer interest
4. Implement if receptive

## Timeline Estimate

| Phase | Task | Duration |
|-------|------|----------|
| 1 | Core signing logic | 1-2 days |
| 2 | URL route handler | 1 day |
| 3 | Approval UI | 1 day |
| 4 | Wallet selector integration | 1 day |
| 5 | Testing | 2-3 days |
| 6 | Documentation | 1 day |
| **Total** | | **7-10 days** |

## Impact

### For x402 Ecosystem
- Enables true gasless NEAR payments via browser wallets
- Users sign once, facilitator pays gas
- Seamless UX matching EVM chains

### For NEAR Ecosystem
- First wallet to support NEP-366 in browser
- Opens meta-transactions to all NEAR dApps
- Reference implementation for other wallets

## Next Steps

1. [ ] Open issue on mynearwallet/my-near-wallet explaining use case
2. [ ] Fork repository and set up development environment
3. [ ] Implement Phase 1 (core signing logic) as proof of concept
4. [ ] Create draft PR with implementation plan
5. [ ] Engage with maintainers for feedback
6. [ ] Complete implementation based on feedback
7. [ ] Submit final PR
8. [ ] Update x402 facilitator and pixel-mar to use new method

## References

- [near-api-js buildDelegateAction](https://github.com/near/near-api-js/blob/master/packages/accounts/src/account.ts)
- [NEP-366 Meta Transactions](https://github.com/near/NEPs/pull/366)
- [NEAR Meta Transactions Docs](https://docs.near.org/chain-abstraction/meta-transactions)
- [MyNearWallet Repository](https://github.com/mynearwallet/my-near-wallet)
- [Wallet Selector Issue #456](https://github.com/near/wallet-selector/issues/456)
