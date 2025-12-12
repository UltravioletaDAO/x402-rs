## Context: You are implementing a critical feature for the NEAR ecosystem

I need you to implement `signDelegateAction` support in MyNearWallet. This will make MyNearWallet the **first browser wallet** to support NEP-366 meta-transactions, enabling gasless payments across the NEAR ecosystem.

### Why This Matters: The x402 Protocol

x402 is a payment protocol (HTTP 402 Payment Required) that enables micropayments across blockchains. The x402 facilitator service allows users to make payments where a **relayer pays the gas fees** on behalf of the user. This is critical for:

- Micropayments (where gas > payment amount would be absurd)
- Onboarding new users who don't have native tokens for gas
- Seamless UX matching traditional payment systems

On EVM chains (Ethereum, Base, etc.), this works via EIP-3009 (transferWithAuthorization). On Solana, it works via pre-signed transactions. On NEAR, the equivalent is **NEP-366 meta-transactions** using `SignedDelegateAction`.

### The Current Problem

**No NEAR browser wallet supports `signDelegateAction`.**

The wallet-selector API only has:
- `signAndSendTransaction()` - user signs AND pays gas
- `signMessage()` - NEP-413 message signing (different format, incompatible)

We need:
- `signDelegateAction()` - user signs, **relayer pays gas**

### The Technical Gap

**NEP-413** (what wallets support - message signing):
```
Discriminant: 2^31 + 413 = 2,147,484,061
Format: SHA256(borsh(discriminant) || borsh(Payload{message, nonce, recipient}))
```

**NEP-366** (what we need - meta-transactions):
```
Discriminant: 2^30 + 366 = 1,073,742,190
Format: SHA256(borsh(SignableMessage{discriminant, DelegateAction}))
```

These produce **completely different signatures**. A NEP-413 signature CANNOT be used for NEP-366. The NEAR runtime verifies signatures on-chain using NEP-366 format.

### The Good News: near-api-js Already Has Everything

```javascript
import { buildDelegateAction, signDelegateAction } from '@near-js/transactions';

const delegateAction = buildDelegateAction({
    actions,
    maxBlockHeight: BigInt(currentHeight) + 100n,
    nonce: BigInt(accessKeyNonce) + 1n,
    publicKey,
    receiverId,
    senderId: accountId,
});

const { signedDelegateAction } = await signDelegateAction({
    delegateAction,
    signer: {
        sign: async (message) => {
            // This is where the wallet's private key is used
            return keyPair.sign(message);
        },
    }
});
```

The cryptographic heavy lifting is DONE. We just need to expose this functionality in the wallet UI.

---

## Your Task: Implement signDelegateAction in MyNearWallet

### Requirements

1. **New URL Route**: `/sign-delegate-action` that accepts delegate action parameters
2. **Approval UI**: Show user what they're signing (similar to transaction approval)
3. **Signing Logic**: Use near-api-js `buildDelegateAction` and `signDelegateAction`
4. **Callback**: Return the base64-encoded `SignedDelegateAction` to the calling app

### Flow

```
1. dApp redirects to: https://app.mynearwallet.com/sign-delegate-action?params=...
2. Wallet parses params, shows approval UI
3. User clicks "Approve"
4. Wallet builds DelegateAction, signs with user's key
5. Wallet redirects back to dApp with SignedDelegateAction
6. dApp sends SignedDelegateAction to relayer/facilitator
7. Relayer wraps in transaction and submits (paying gas)
```

### URL Parameters

```
/sign-delegate-action?
  receiverId=usdc.near&                    // Contract to call
  actions=[{"methodName":"ft_transfer","args":{"receiver_id":"merchant.near","amount":"1000000"},"gas":"30000000000000","deposit":"1"}]&
  callbackUrl=https://app.example.com/callback&
  meta={"referrer":"402milly"}             // Optional metadata
```

### Response (via callback URL)

```
https://app.example.com/callback?
  accountId=alice.near&
  publicKey=ed25519:ABC...&
  signedDelegateAction=BASE64_ENCODED_BORSH&
  signature=ed25519:XYZ...
```

---

## Implementation Guide

### Step 1: Create the Core Signing Utility

**Create file**: `packages/frontend/src/utils/wallet/delegateAction.js`

```javascript
import { buildDelegateAction, signDelegateAction } from '@near-js/transactions';
import { serialize } from 'borsh';
import { transactions } from 'near-api-js';

/**
 * Creates a SignedDelegateAction for meta-transaction submission.
 * The user signs this off-chain, and a relayer submits it (paying gas).
 *
 * @param {Object} params
 * @param {string} params.accountId - The user's account ID (sender)
 * @param {string} params.receiverId - The contract to call
 * @param {Array} params.actions - Actions to execute
 * @param {Object} params.signer - The signer with access to private key
 * @param {Object} params.connection - NEAR connection
 * @param {number} params.blockHeightTtl - Block height TTL (default 100)
 * @returns {Promise<{signedDelegateAction: SignedDelegateAction, serialized: string}>}
 */
export async function createSignedDelegateAction({
    accountId,
    receiverId,
    actions,
    signer,
    connection,
    blockHeightTtl = 100
}) {
    // Get current block height for maxBlockHeight
    const block = await connection.provider.block({ finality: 'final' });
    const blockHeight = BigInt(block.header.height);

    // Get user's public key
    const publicKey = await signer.getPublicKey(accountId, connection.networkId);

    // Get access key nonce
    const accessKey = await connection.provider.query({
        request_type: 'view_access_key',
        finality: 'final',
        account_id: accountId,
        public_key: publicKey.toString(),
    });

    // Convert actions to NEAR action format
    const nearActions = actions.map(action => {
        if (action.methodName) {
            return transactions.functionCall(
                action.methodName,
                action.args || {},
                action.gas || '30000000000000',
                action.deposit || '0'
            );
        }
        // Add other action types as needed (transfer, etc.)
        throw new Error(`Unsupported action type: ${JSON.stringify(action)}`);
    });

    // Build the delegate action
    const delegateAction = buildDelegateAction({
        actions: nearActions,
        maxBlockHeight: blockHeight + BigInt(blockHeightTtl),
        nonce: BigInt(accessKey.nonce) + 1n,
        publicKey,
        receiverId,
        senderId: accountId,
    });

    // Sign the delegate action
    const { signedDelegateAction } = await signDelegateAction({
        delegateAction,
        signer: {
            sign: async (message) => {
                const { signature } = await signer.signMessage(
                    message,
                    accountId,
                    connection.networkId
                );
                return signature;
            },
        }
    });

    // Serialize to base64 for transport
    const serialized = Buffer.from(
        serialize(transactions.SCHEMA, signedDelegateAction)
    ).toString('base64');

    return {
        signedDelegateAction,
        serialized,
        publicKey: publicKey.toString(),
    };
}

/**
 * Parses actions from URL query parameter
 */
export function parseActionsFromQuery(actionsParam) {
    try {
        const actions = JSON.parse(decodeURIComponent(actionsParam));
        if (!Array.isArray(actions)) {
            throw new Error('Actions must be an array');
        }
        return actions;
    } catch (error) {
        throw new Error(`Invalid actions parameter: ${error.message}`);
    }
}

/**
 * Validates delegate action parameters
 */
export function validateDelegateActionParams({ receiverId, actions }) {
    if (!receiverId || typeof receiverId !== 'string') {
        throw new Error('receiverId is required and must be a string');
    }
    if (!actions || !Array.isArray(actions) || actions.length === 0) {
        throw new Error('actions is required and must be a non-empty array');
    }
    // Validate each action
    for (const action of actions) {
        if (!action.methodName) {
            throw new Error('Each action must have a methodName');
        }
    }
    return true;
}
```

### Step 2: Create the Route Component

**Create file**: `packages/frontend/src/routes/SignDelegateAction/SignDelegateActionContainer.js`

```javascript
import React, { useState, useEffect } from 'react';
import { useSelector } from 'react-redux';
import { useLocation, useHistory } from 'react-router-dom';
import {
    createSignedDelegateAction,
    parseActionsFromQuery,
    validateDelegateActionParams
} from '../../utils/wallet/delegateAction';
import SignDelegateActionView from './SignDelegateActionView';
import { selectAccountId } from '../../redux/slices/account';

const SignDelegateActionContainer = () => {
    const location = useLocation();
    const history = useHistory();
    const accountId = useSelector(selectAccountId);

    const [loading, setLoading] = useState(true);
    const [signing, setSigning] = useState(false);
    const [error, setError] = useState(null);
    const [params, setParams] = useState(null);

    useEffect(() => {
        try {
            const searchParams = new URLSearchParams(location.search);

            const receiverId = searchParams.get('receiverId');
            const actionsParam = searchParams.get('actions');
            const callbackUrl = searchParams.get('callbackUrl');
            const meta = searchParams.get('meta');

            if (!receiverId || !actionsParam || !callbackUrl) {
                throw new Error('Missing required parameters: receiverId, actions, callbackUrl');
            }

            const actions = parseActionsFromQuery(actionsParam);
            validateDelegateActionParams({ receiverId, actions });

            setParams({
                receiverId,
                actions,
                callbackUrl,
                meta: meta ? JSON.parse(meta) : null,
            });
            setLoading(false);
        } catch (err) {
            setError(err.message);
            setLoading(false);
        }
    }, [location.search]);

    const handleApprove = async () => {
        setSigning(true);
        setError(null);

        try {
            // Get signer and connection from wallet state
            const { wallet } = window; // Or however MyNearWallet accesses wallet
            const connection = wallet.connection;
            const signer = wallet.account().signer;

            const result = await createSignedDelegateAction({
                accountId,
                receiverId: params.receiverId,
                actions: params.actions,
                signer,
                connection,
            });

            // Build callback URL with result
            const callbackUrl = new URL(params.callbackUrl);
            callbackUrl.searchParams.set('accountId', accountId);
            callbackUrl.searchParams.set('publicKey', result.publicKey);
            callbackUrl.searchParams.set('signedDelegateAction', result.serialized);

            // Redirect back to dApp
            window.location.href = callbackUrl.toString();
        } catch (err) {
            console.error('Failed to sign delegate action:', err);
            setError(err.message);
            setSigning(false);
        }
    };

    const handleReject = () => {
        if (params?.callbackUrl) {
            const callbackUrl = new URL(params.callbackUrl);
            callbackUrl.searchParams.set('error', 'User rejected');
            window.location.href = callbackUrl.toString();
        } else {
            history.push('/');
        }
    };

    if (loading) {
        return <div>Loading...</div>;
    }

    return (
        <SignDelegateActionView
            accountId={accountId}
            params={params}
            error={error}
            signing={signing}
            onApprove={handleApprove}
            onReject={handleReject}
        />
    );
};

export default SignDelegateActionContainer;
```

### Step 3: Create the Approval UI View

**Create file**: `packages/frontend/src/routes/SignDelegateAction/SignDelegateActionView.js`

```javascript
import React from 'react';
import styled from 'styled-components';
import FormButton from '../../components/common/FormButton';
import Container from '../../components/common/styled/Container.css';

const StyledContainer = styled(Container)`
    .header {
        text-align: center;
        margin-bottom: 24px;
    }

    .title {
        font-size: 24px;
        font-weight: 600;
        color: #24272a;
    }

    .subtitle {
        font-size: 14px;
        color: #72727a;
        margin-top: 8px;
    }

    .gasless-badge {
        display: inline-block;
        background: #00ec97;
        color: #000;
        padding: 4px 12px;
        border-radius: 16px;
        font-size: 12px;
        font-weight: 600;
        margin-top: 12px;
    }

    .details-card {
        background: #f8f9fa;
        border-radius: 12px;
        padding: 20px;
        margin: 20px 0;
    }

    .detail-row {
        display: flex;
        justify-content: space-between;
        padding: 12px 0;
        border-bottom: 1px solid #e5e5e5;

        &:last-child {
            border-bottom: none;
        }
    }

    .detail-label {
        color: #72727a;
        font-size: 14px;
    }

    .detail-value {
        color: #24272a;
        font-size: 14px;
        font-weight: 500;
        word-break: break-all;
        text-align: right;
        max-width: 200px;
    }

    .actions-section {
        margin: 20px 0;
    }

    .action-item {
        background: #fff;
        border: 1px solid #e5e5e5;
        border-radius: 8px;
        padding: 16px;
        margin-bottom: 12px;
    }

    .action-method {
        font-weight: 600;
        color: #24272a;
    }

    .action-args {
        font-size: 12px;
        color: #72727a;
        margin-top: 8px;
        font-family: monospace;
        word-break: break-all;
    }

    .warning {
        background: #fff3cd;
        border: 1px solid #ffc107;
        border-radius: 8px;
        padding: 12px;
        margin: 20px 0;
        font-size: 13px;
        color: #856404;
    }

    .error {
        background: #f8d7da;
        border: 1px solid #f5c6cb;
        border-radius: 8px;
        padding: 12px;
        margin: 20px 0;
        font-size: 13px;
        color: #721c24;
    }

    .buttons {
        display: flex;
        gap: 12px;
        margin-top: 24px;
    }
`;

const SignDelegateActionView = ({
    accountId,
    params,
    error,
    signing,
    onApprove,
    onReject
}) => {
    if (!params) {
        return (
            <StyledContainer>
                <div className="error">
                    {error || 'Invalid request parameters'}
                </div>
            </StyledContainer>
        );
    }

    const { receiverId, actions, meta } = params;

    // Format action for display
    const formatActionArgs = (args) => {
        if (!args) return null;
        if (typeof args === 'string') return args;
        return JSON.stringify(args, null, 2);
    };

    // Check if this looks like a USDC transfer
    const isUsdcTransfer = actions.some(a =>
        a.methodName === 'ft_transfer' &&
        (receiverId.includes('usdc') || receiverId.includes('usdt'))
    );

    return (
        <StyledContainer>
            <div className="header">
                <div className="title">Approve Meta-Transaction</div>
                <div className="subtitle">
                    Sign this action to be submitted by a relayer
                </div>
                <div className="gasless-badge">
                    GASLESS - You pay $0 in fees
                </div>
            </div>

            <div className="details-card">
                <div className="detail-row">
                    <span className="detail-label">From Account</span>
                    <span className="detail-value">{accountId}</span>
                </div>
                <div className="detail-row">
                    <span className="detail-label">Contract</span>
                    <span className="detail-value">{receiverId}</span>
                </div>
                {meta?.referrer && (
                    <div className="detail-row">
                        <span className="detail-label">Requested By</span>
                        <span className="detail-value">{meta.referrer}</span>
                    </div>
                )}
            </div>

            <div className="actions-section">
                <h3>Actions to Execute</h3>
                {actions.map((action, index) => (
                    <div key={index} className="action-item">
                        <div className="action-method">
                            {action.methodName}
                        </div>
                        {action.args && (
                            <div className="action-args">
                                {formatActionArgs(action.args)}
                            </div>
                        )}
                    </div>
                ))}
            </div>

            <div className="warning">
                <strong>How this works:</strong> You are signing authorization for these actions.
                A relayer service will submit this transaction to NEAR and pay the gas fees on your behalf.
                Your account will NOT be charged any NEAR for gas.
            </div>

            {error && (
                <div className="error">
                    {error}
                </div>
            )}

            <div className="buttons">
                <FormButton
                    onClick={onReject}
                    color="gray"
                    disabled={signing}
                >
                    Reject
                </FormButton>
                <FormButton
                    onClick={onApprove}
                    disabled={signing}
                    sending={signing}
                    sendingString="Signing..."
                >
                    Approve
                </FormButton>
            </div>
        </StyledContainer>
    );
};

export default SignDelegateActionView;
```

### Step 4: Register the Route

**Modify file**: `packages/frontend/src/Routes.js` (or wherever routes are defined)

Add the new route:

```javascript
import SignDelegateActionContainer from './routes/SignDelegateAction/SignDelegateActionContainer';

// In the routes array/switch:
<Route
    exact
    path="/sign-delegate-action"
    component={SignDelegateActionContainer}
/>
```

### Step 5: Add Translations (if using i18n)

**Modify file**: `packages/frontend/src/translations/en.json`

```json
{
    "signDelegateAction": {
        "title": "Approve Meta-Transaction",
        "subtitle": "Sign this action to be submitted by a relayer",
        "gaslessBadge": "GASLESS - You pay $0 in fees",
        "fromAccount": "From Account",
        "contract": "Contract",
        "requestedBy": "Requested By",
        "actionsToExecute": "Actions to Execute",
        "howItWorks": "How this works:",
        "howItWorksDescription": "You are signing authorization for these actions. A relayer service will submit this transaction to NEAR and pay the gas fees on your behalf. Your account will NOT be charged any NEAR for gas.",
        "approve": "Approve",
        "reject": "Reject",
        "signing": "Signing...",
        "errorInvalidParams": "Invalid request parameters",
        "errorMissingParams": "Missing required parameters: receiverId, actions, callbackUrl"
    }
}
```

---

## Testing

### Manual Test Flow

1. Start MyNearWallet locally: `yarn start`

2. Create a test URL:
```
http://localhost:3000/sign-delegate-action?
receiverId=usdc.near&
actions=[{"methodName":"ft_transfer","args":{"receiver_id":"test.near","amount":"1000000"},"gas":"30000000000000","deposit":"1"}]&
callbackUrl=http://localhost:8080/callback
```

3. Verify:
   - Approval UI shows correctly
   - Account ID displays
   - Contract (receiverId) displays
   - Actions display with formatted args
   - "Gasless" badge appears
   - Warning message explains meta-transaction

4. Test approve flow:
   - Click Approve
   - Should redirect to callback URL with signedDelegateAction param
   - Decode base64, verify it's valid borsh

5. Test reject flow:
   - Click Reject
   - Should redirect to callback URL with error param

### Unit Tests

**Create file**: `packages/frontend/src/utils/wallet/__tests__/delegateAction.test.js`

```javascript
import {
    parseActionsFromQuery,
    validateDelegateActionParams
} from '../delegateAction';

describe('delegateAction utils', () => {
    describe('parseActionsFromQuery', () => {
        it('parses valid actions JSON', () => {
            const input = encodeURIComponent(JSON.stringify([
                { methodName: 'ft_transfer', args: { receiver_id: 'test.near', amount: '100' } }
            ]));
            const result = parseActionsFromQuery(input);
            expect(result).toHaveLength(1);
            expect(result[0].methodName).toBe('ft_transfer');
        });

        it('throws on invalid JSON', () => {
            expect(() => parseActionsFromQuery('not-json')).toThrow();
        });

        it('throws on non-array', () => {
            const input = encodeURIComponent(JSON.stringify({ methodName: 'test' }));
            expect(() => parseActionsFromQuery(input)).toThrow('must be an array');
        });
    });

    describe('validateDelegateActionParams', () => {
        it('accepts valid params', () => {
            expect(validateDelegateActionParams({
                receiverId: 'contract.near',
                actions: [{ methodName: 'test' }]
            })).toBe(true);
        });

        it('rejects missing receiverId', () => {
            expect(() => validateDelegateActionParams({
                actions: [{ methodName: 'test' }]
            })).toThrow('receiverId');
        });

        it('rejects empty actions', () => {
            expect(() => validateDelegateActionParams({
                receiverId: 'contract.near',
                actions: []
            })).toThrow('non-empty');
        });
    });
});
```

---

## Integration with Wallet Selector (Future)

Once this is implemented in MyNearWallet, the wallet-selector module needs updating:

**File to modify**: `packages/my-near-wallet/src/lib/my-near-wallet.ts` (in wallet-selector repo)

```typescript
async signDelegateAction({
    receiverId,
    actions,
}: SignDelegateActionParams): Promise<SignedDelegateAction> {
    const currentUrl = new URL(window.location.href);
    const newUrl = new URL('sign-delegate-action', this._walletBaseUrl);

    newUrl.searchParams.set('receiverId', receiverId);
    newUrl.searchParams.set('actions', JSON.stringify(actions));
    newUrl.searchParams.set('callbackUrl', currentUrl.href);

    // Redirect to wallet
    window.location.assign(newUrl.toString());

    // This returns after redirect back
    return new Promise((resolve, reject) => {
        // Handle callback params...
    });
}
```

---

## Security Considerations

1. **Validate all URL parameters** - Never trust input
2. **Show clear UI** - User must understand what they're signing
3. **maxBlockHeight TTL** - Default to ~100 blocks (~2 minutes) to prevent replay
4. **Nonce validation** - Use access key nonce to prevent double-submission
5. **callbackUrl validation** - Consider validating callback domain

---

## PR Description Template

When submitting the PR, use this description:

```markdown
## Summary

Adds support for signing NEP-366 DelegateActions (meta-transactions), enabling gasless transactions on NEAR.

## Motivation

Currently, no NEAR browser wallet supports meta-transaction signing. This prevents dApps from implementing gasless transactions where a relayer pays gas on behalf of users.

Use cases:
- Micropayments (x402 protocol)
- Onboarding users without NEAR for gas
- Improved UX for token transfers

## Changes

- New route `/sign-delegate-action` for meta-transaction approval
- Approval UI showing transaction details with "gasless" indicator
- Core signing logic using `near-api-js` `buildDelegateAction` and `signDelegateAction`
- URL-based callback flow for dApp integration

## Technical Details

Uses existing `near-api-js` functions:
- `buildDelegateAction()` - constructs the DelegateAction
- `signDelegateAction()` - signs with NEP-366 format

The signed result is returned as base64-encoded borsh to the callback URL.

## Testing

- [ ] Manual testing with test URLs
- [ ] Unit tests for parsing/validation
- [ ] E2E test with relayer submission

## Related

- NEP-366: https://github.com/near/NEPs/pull/366
- wallet-selector issue #456: https://github.com/near/wallet-selector/issues/456
- x402 protocol: https://x402.org
```

---

## Summary

You are implementing `signDelegateAction` in MyNearWallet to enable NEP-366 meta-transactions. The core functionality exists in `near-api-js` - you're building the UI and URL routing to expose it.

Key files to create:
1. `utils/wallet/delegateAction.js` - Core signing logic
2. `routes/SignDelegateAction/SignDelegateActionContainer.js` - Route handler
3. `routes/SignDelegateAction/SignDelegateActionView.js` - Approval UI

Key points:
- Use `buildDelegateAction` and `signDelegateAction` from `@near-js/transactions`
- Return base64-encoded borsh serialization via callback URL
- Show clear "gasless" messaging to users
- Validate all inputs, handle errors gracefully

This will make MyNearWallet the first browser wallet to support gasless NEAR transactions!
