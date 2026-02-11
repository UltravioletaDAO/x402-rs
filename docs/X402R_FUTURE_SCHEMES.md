# x402r Escrow: Future Scheme Extensions

## Context

Conversation with Ali Abdoli (BackTrackCo) on February 7, 2026 regarding expanding x402r escrow beyond EIP-3009/USDC-only.

Key points from Ali:
- "Anything with 3009 should work but we haven't tested. The TVL limit won't though"
- "We could also support permit2, preapproval and coinbase spend permissions pretty easily"
- "The contracts can stay the same but the scheme has to change"

## Current State

The x402r escrow contracts use EIP-3009 `transferWithAuthorization` as the transfer mechanism. Currently tested only with USDC. The facilitator already supports multiple EIP-3009 tokens (USDC, EURC) for direct settlement but not yet through escrow.

### Supported EIP-3009 Tokens (Facilitator)

| Token | Networks | EIP-712 Domain Name |
|-------|----------|-------------------|
| USDC | All 18 EVM networks | "USD Coin" (most), "USDC" (Celo, Monad, HyperEVM, Unichain) |
| EURC | Base, Ethereum | "EURC" (Base), "Euro Coin" (Ethereum) |

## Extension 1: Multi-Token Escrow (EIP-3009)

### What Changes

**Contracts**: Nothing - Ali confirmed they work with any EIP-3009 token.

**Facilitator changes needed**:
- `src/payment_operator/addresses.rs` - Add token addresses per network for EURC, PYUSD, etc.
- `src/payment_operator/operator.rs` - Token validation (currently assumes USDC)
- `src/chain/evm.rs` - EIP-712 domain name resolution already handles multiple tokens

**Limitation**: TVL limits in the escrow contracts are USDC-specific. Other tokens won't have TVL caps, which could be a risk vector for high-value escrow.

### Effort: Low

This is the easiest extension. Most of the multi-token infrastructure already exists in the facilitator for direct settlement. The escrow path just needs to accept non-USDC token addresses.

## Extension 2: Permit2 Support

### What Is Permit2

[Uniswap Permit2](https://github.com/Uniswap/permit2) is a universal token approval system. Instead of each protocol needing its own approval mechanism, tokens approve the Permit2 contract once, and then Permit2 handles granular permissions.

**Permit2 contract**: `0x000000000022D473030F116dDEE9F6B43aC78BA3` (same address on all EVM chains via CREATE2)

### Why This Matters

EIP-3009 is rare. Only a handful of tokens implement `transferWithAuthorization`:
- USDC (Circle)
- EURC (Circle)
- PYUSD (PayPal) - on some chains
- A few others

Permit2 works with ANY ERC-20 token that has standard `approve()`. This unlocks:
- **USDT** (Tether) - largest stablecoin by market cap
- **DAI** (MakerDAO)
- **FRAX**, **LUSD**, **GHO**, and other DeFi stablecoins
- Non-stablecoin tokens (WETH, WBTC, etc.)

### How It Works

1. User approves Permit2 contract for the token (one-time, max approval)
2. User signs a Permit2 `PermitTransferFrom` message (off-chain, gasless)
3. Escrow contract calls `permit2.permitTransferFrom(...)` to pull tokens

### What Changes

**Contracts**: Ali says contracts stay the same, but the **scheme** must change. Likely a new method on the escrow contract that accepts Permit2 signatures instead of EIP-3009 signatures.

**Facilitator changes needed**:
- New scheme type: `escrow-permit2` or `escrow` with a `transferMethod` field
- `src/types.rs` / `src/types_v2.rs` - New payload fields for Permit2 signatures
- `src/chain/evm.rs` - Permit2 signature construction and verification
- `src/payment_operator/operator.rs` - Route to correct transfer method based on scheme
- SDK updates for signing Permit2 messages

**User experience trade-off**: Unlike EIP-3009 (fully gasless, no prior approval), Permit2 requires one approval transaction first. After that, all subsequent transfers are gasless via signatures.

### Permit2 Signature Format

```solidity
struct PermitTransferFrom {
    TokenPermissions permitted;  // token address + max amount
    uint256 nonce;               // unique nonce
    uint256 deadline;            // expiration timestamp
}

struct TokenPermissions {
    address token;
    uint256 amount;
}
```

The facilitator would need to verify and submit these signatures to the Permit2 contract through the escrow.

### Effort: Medium

Requires new signature types, verification logic, and SDK changes. But the pattern is well-established (Uniswap, 1inch, CoW Protocol all use Permit2).

## Extension 3: Coinbase Spend Permissions

### What Is It

Coinbase Smart Wallet allows users to set "spend permissions" - pre-authorized spending limits for specific contracts. This is part of Coinbase's account abstraction (ERC-4337) implementation.

### How It Works

1. User configures spend permission in Coinbase Wallet (e.g., "allow this app to spend up to $100/day")
2. The permitted contract can pull tokens without per-transaction signatures
3. Works through Coinbase's smart contract wallet infrastructure

### What Changes

**Contracts**: New interaction pattern with Coinbase's SpendPermission contract.

**Facilitator changes needed**:
- Integration with Coinbase Smart Wallet SDK
- New scheme type for spend permissions
- Verification that the facilitator's operator is an authorized spender

### Limitations

- Only works with Coinbase Smart Wallet users
- Primarily Base network (though expanding)
- Requires users to proactively set permissions

### Effort: Medium-High

More niche audience but high-value users. Would require understanding Coinbase's smart wallet architecture and SpendPermission contracts.

## Extension 4: EIP-2612 Permit

### What Is It

Standard `permit()` function defined in EIP-2612. Similar to EIP-3009 but uses `permit` + `transferFrom` pattern instead of `transferWithAuthorization`.

### Tokens That Support It

Much broader than EIP-3009:
- DAI (original permit implementation)
- Most modern ERC-20s deployed after 2021
- Many DeFi tokens (AAVE, UNI, COMP, etc.)

### How It Works

1. User signs an EIP-2612 permit message (off-chain)
2. Contract calls `token.permit(owner, spender, value, deadline, v, r, s)`
3. This sets an allowance
4. Contract then calls `token.transferFrom(owner, escrow, value)`

Two-step (permit + transfer) vs EIP-3009's single-step (transferWithAuthorization).

### Effort: Low-Medium

Well-understood pattern, but requires the escrow contract to do two calls (permit + transferFrom) atomically.

## Priority Recommendation

| Extension | Impact | Effort | Priority |
|-----------|--------|--------|----------|
| Multi-token EIP-3009 | Low (few tokens) | Low | P2 - Do when needed |
| Permit2 | High (any ERC-20) | Medium | **P1 - Highest value** |
| EIP-2612 Permit | Medium (many tokens) | Low-Medium | P3 - Superseded by Permit2 |
| Coinbase Spend | Low-Medium (niche) | Medium-High | P4 - Nice to have |

**Permit2 should be the priority** - it provides the most universal token coverage with reasonable implementation effort. It effectively makes EIP-2612 support unnecessary since Permit2 already works with any approved ERC-20.

## Architecture Notes

### Scheme Identification

When implementing new transfer methods, the x402 protocol needs a way to distinguish them. Options:

1. **New scheme values**: `escrow-3009`, `escrow-permit2`, `escrow-spend`
2. **Sub-field in existing scheme**: `scheme: "escrow"` + `extra.transferMethod: "permit2"`
3. **Auto-detection**: Facilitator detects based on signature format

Option 2 is cleanest - keeps backward compatibility with existing `escrow` scheme.

### Facilitator Router Pattern

```
POST /settle
  -> parse scheme
  -> if scheme == "escrow":
       -> check extra.transferMethod (default: "eip3009")
       -> route to appropriate handler:
          - eip3009: current flow (transferWithAuthorization)
          - permit2: new flow (permitTransferFrom)
          - spend: coinbase spend permission flow
```

### Contract Interface (Speculative)

The escrow contract would need additional methods or a generic interface:

```solidity
// Current (EIP-3009)
function authorizePayment(bytes calldata eip3009Auth) external;

// Future (Permit2)
function authorizePaymentPermit2(
    IPermit2.PermitTransferFrom calldata permit,
    bytes calldata signature
) external;
```

## References

- [Uniswap Permit2](https://github.com/Uniswap/permit2)
- [EIP-3009: Transfer With Authorization](https://eips.ethereum.org/EIPS/eip-3009)
- [EIP-2612: Permit](https://eips.ethereum.org/EIPS/eip-2612)
- [Coinbase Smart Wallet](https://www.smartwallet.dev/)
- [x402r SDK - A1igator/multichain-config](https://github.com/BackTrackCo/x402r-sdk/blob/A1igator/multichain-config/)

## Status

**Created**: February 7, 2026
**Status**: Planning only - no implementation scheduled
**Depends on**: x402r team (BackTrackCo) updating contract interfaces and scheme definitions
