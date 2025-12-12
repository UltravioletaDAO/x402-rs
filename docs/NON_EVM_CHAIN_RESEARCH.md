# Non-EVM Chain Integration Research: Stellar, Algorand, XRP Ledger

**Research Date**: December 5, 2025
**Objective**: Determine feasibility of integrating x402-rs Payment Facilitator with Stellar, Algorand, and XRP Ledger
**Author**: Claude Code Deep Research

---

## Executive Summary

| Chain | USDC Native | Rust SDK | EIP-3009 Equivalent | Web Wallet Support | Recommendation |
|-------|-------------|----------|---------------------|-------------------|----------------|
| **Stellar (Soroban)** | Yes ($200M+) | Official (soroban-sdk) | Yes (require_auth + pre-signed) | **EXCELLENT** (Freighter signAuthEntry) | **IMMEDIATE** |
| **Algorand** | Yes ($100M+) | Community (algonaut) | Partial (clawback + LogicSig) | **BLOCKED** (wallets refuse delegation) | Not viable |
| **XRP Ledger** | Yes (June 2025) | Community (xrpl-rust) | No (Hooks not on mainnet) | N/A (protocol limitation) | Wait for Hooks |

**Recommendation**: **Stellar/Soroban is the clear winner** for immediate integration. It has:
- Native USDC ($200M+ circulation)
- Official Rust SDK (`soroban-sdk`)
- Built-in authorization framework (functionally equivalent to EIP-3009)
- **CRITICAL**: Web wallet support via Freighter's `signAuthEntry` API

**WARNING**: Algorand is NOT viable due to wallet limitations. Major wallets (Pera, MyAlgo) deliberately refuse to sign delegated Logic Signatures for security reasons. This is the same problem we encountered with NEAR.

---

## Table of Contents

1. [How x402 Works (EIP-3009 Context)](#how-x402-works-eip-3009-context)
2. [Stellar / Soroban Deep Dive](#stellar--soroban-deep-dive)
3. [Algorand Deep Dive](#algorand-deep-dive)
4. [XRP Ledger Deep Dive](#xrp-ledger-deep-dive)
5. [Technical Comparison Matrix](#technical-comparison-matrix)
6. [Web Wallet Support Analysis (CRITICAL)](#web-wallet-support-analysis-critical)
7. [Implementation Roadmap](#implementation-roadmap)
8. [Sources and References](#sources-and-references)

---

## How x402 Works (EIP-3009 Context)

The x402-rs facilitator relies on **EIP-3009: Transfer With Authorization** for gasless micropayments. Understanding this mechanism is crucial for evaluating non-EVM alternatives.

### EIP-3009 Core Mechanism

```
1. User signs off-chain authorization (typed EIP-712 signature)
2. Authorization includes: from, to, value, validAfter, validBefore, nonce
3. Facilitator calls transferWithAuthorization(from, to, value, validAfter, validBefore, nonce, signature)
4. Token contract verifies signature and executes transfer
5. User never pays gas - facilitator pays and is compensated via the payment itself
```

### Required Equivalent Features in Non-EVM Chains

For a non-EVM chain to support x402-style payments, it needs:

1. **Off-chain signature capability**: User can sign authorization without broadcasting
2. **Third-party submission**: Someone other than signer can submit to network
3. **Replay protection**: Nonces or similar mechanism to prevent double-spending
4. **Timestamp/expiration validation**: Signatures should be time-bounded
5. **USDC availability**: Native stablecoin support (not bridged/wrapped)
6. **Rust SDK**: For integration with x402-rs codebase

---

## Stellar / Soroban Deep Dive

### Overview

Stellar launched **Soroban**, its smart contract platform, on mainnet in February 2024. Soroban is a Rust-based, WebAssembly-powered smart contract runtime that provides sophisticated authorization capabilities.

### USDC Availability

| Metric | Value |
|--------|-------|
| **Native USDC** | Yes - Circle-issued |
| **Circulation** | $200M+ (March 2025) |
| **Asset Issuer** | `GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN` |
| **Soroban Contract** | `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75` |
| **CCTP V2** | Coming to Stellar (cross-chain transfers to 15+ chains) |

**Source**: [Circle USDC on Stellar](https://www.circle.com/multi-chain-usdc/stellar)

### Rust SDK Status

| SDK | Status | Maintainer |
|-----|--------|------------|
| **soroban-sdk** | Official, Active | Stellar Development Foundation |
| **stellar-client** | Community | choubacha |
| **stellar_sdk** | Community | matinkaboli |

The official `soroban-sdk` is actively maintained with frequent releases. Latest features include constructor support and enhanced authorization entries.

**Crate**: [soroban-sdk on crates.io](https://crates.io/crates/soroban-sdk)

### EIP-3009 Equivalent: Soroban Authorization Framework

**THIS IS THE KEY FINDING**: Soroban has a built-in authorization framework that is functionally equivalent to EIP-3009.

#### How It Works

```rust
// Contract uses require_auth for authorization
fn transfer(env: Env, from: Address, to: Address, amount: i128) {
    from.require_auth();  // Soroban handles signature verification
    // Transfer logic...
}
```

#### Off-Chain Pre-Authorization

Soroban supports **pre-signed authorization entries**:

1. **Simulation**: Use `simulateTransaction` RPC to get required authorizations
2. **Signing**: User signs authorization entries off-chain with their private key
3. **Submission**: Facilitator submits transaction with signed authorization entries
4. **Execution**: Soroban host verifies signatures and executes if valid

```javascript
// Example authorization entry structure
{
    credentials: {
        address: "G...",
        nonce: 12345,
        signature_expiration_ledger: 50000000
    },
    root_invocation: {
        contract_id: "CCW67...",
        function_name: "transfer",
        args: [from, to, amount]
    }
}
```

#### Replay Protection

- **Nonce consumption**: Nonces are consumed only after successful authentication
- **Signature expiration**: `signature_expiration_ledger` enforces time bounds
- **One-time use**: Nonce stored in ledger prevents replay

#### Key Advantages Over EIP-3009

| Feature | EIP-3009 | Soroban Auth |
|---------|----------|--------------|
| Replay protection | Manual nonce tracking | Automatic by host |
| Expiration | Unix timestamps | Ledger numbers |
| Signature format | EIP-712 typed data | Native Stellar format |
| Gas estimation | Manual | Built into simulation |

### Token Interface (SEP-41)

Stellar tokens follow **SEP-41**, which is similar to ERC-20:

```rust
trait TokenInterface {
    fn transfer(env: Env, from: Address, to: Address, amount: i128);
    fn approve(env: Env, from: Address, spender: Address, amount: i128, expiration_ledger: u32);
    fn allowance(env: Env, from: Address, spender: Address) -> i128;
    fn balance(env: Env, id: Address) -> i128;
    // ... more functions
}
```

The **Stellar Asset Contract (SAC)** wraps native Stellar assets (like USDC) for use in Soroban contracts, implementing SEP-41.

### Fee Structure

| Component | Typical Cost |
|-----------|-------------|
| Resource fee (avg) | ~215,000 stroops (~$0.02) |
| Inclusion fee | Variable, low |
| Total transaction | $0.02 - $0.10 |

Fees are estimated via `simulateTransaction` RPC before submission.

### Integration Architecture for x402

```
                        ┌─────────────────────────┐
                        │     x402-rs Facilitator │
                        └───────────┬─────────────┘
                                    │
            ┌───────────────────────┼───────────────────────┐
            │                       │                       │
    ┌───────▼───────┐      ┌───────▼───────┐      ┌───────▼───────┐
    │   EVM Module  │      │ Solana Module │      │ Stellar Module │
    │  (existing)   │      │  (existing)   │      │    (NEW)       │
    └───────────────┘      └───────────────┘      └───────────────┘
                                                          │
                                                  ┌───────▼───────┐
                                                  │  soroban-sdk  │
                                                  │  + RPC client │
                                                  └───────────────┘
```

### Estimated Implementation Effort

| Task | Complexity | Lines of Code |
|------|------------|---------------|
| Add `NetworkFamily::Stellar` | Low | ~50 |
| Implement Stellar provider cache | Medium | ~150 |
| Implement `verify()` for Stellar | Medium | ~200 |
| Implement `settle()` for Stellar | Medium | ~250 |
| Add USDC contract addresses | Low | ~30 |
| Add RPC configuration | Low | ~40 |
| Integration tests | Medium | ~300 |
| **Total** | **Medium** | **~1,000** |

### Stellar Verdict: HIGHLY FEASIBLE

**Pros**:
- Official Rust SDK with active development
- Native USDC with high liquidity ($200M+)
- Built-in authorization framework equivalent to EIP-3009
- Transaction simulation for fee estimation
- Low fees (~$0.02 per transaction)
- SEP-41 token standard similar to ERC-20

**Cons**:
- Soroban is relatively new (mainnet Feb 2024)
- Different signature format than EIP-712
- Ledger-based expiration vs Unix timestamps
- Smaller ecosystem than EVM

**Recommendation**: **INTEGRATE IMMEDIATELY**

---

## Algorand Deep Dive

### Overview

Algorand is a proof-of-stake blockchain with native asset support (ASAs - Algorand Standard Assets) and smart contract capabilities via TEAL (Transaction Execution Approval Language).

### USDC Availability

| Metric | Value |
|--------|-------|
| **Native USDC** | Yes - Circle-issued |
| **Circulation** | $100M+ (March 2025) |
| **Mainnet ASA ID** | `31566704` |
| **Testnet ASA ID** | `10458941` |
| **Launch Date** | September 2020 |

**Source**: [Circle USDC on Algorand](https://www.circle.com/multi-chain-usdc/algorand)

### Rust SDK Status

| SDK | Status | Last Update |
|-----|--------|-------------|
| **algonaut** | Community, Active | Ongoing |
| **algorand_rust_sdk** | Experimental | Archived |

The primary Rust SDK is `algonaut` by manuelmauro, which provides:
- Transaction building and signing
- ASA operations
- Smart contract interaction
- ED25519 cryptography

**Crate**: [algonaut on crates.io](https://crates.io/crates/algonaut)

**Modules**:
- `algonaut_transaction` - Transaction building
- `algonaut_abi` - Smart contract ABI
- `algonaut_core` - Core types (Address, MicroAlgos)
- `algonaut_crypto` - ED25519, mnemonics

### EIP-3009 Equivalent: Clawback + Logic Signatures

Algorand does **NOT** have a direct EIP-3009 equivalent, but there are patterns that can achieve similar functionality:

#### Pattern 1: Clawback Address with Logic Signature

```
1. Create USDC as default-frozen (Circle controls this for real USDC)
2. Clawback address set to a Logic Signature (smart signature)
3. User signs authorization that satisfies Logic Signature conditions
4. Facilitator submits clawback transaction with user's authorization
5. Logic Signature validates and allows transfer
```

**CRITICAL ISSUE**: Circle's USDC on Algorand has **Circle-controlled clawback address**. We cannot use this pattern without Circle's involvement.

#### Pattern 2: Delegated Logic Signatures

```rust
// Delegated Logic Signature concept
// User creates and signs a Logic Signature that authorizes specific transfers

// LogicSig conditions could include:
// - Recipient address
// - Maximum amount
// - Expiration block
// - Nonce (custom implementation needed)
```

**Problem**: Logic Signatures in Algorand don't have built-in nonce support. Replay protection must be implemented manually using:
- Application state (stateful smart contract)
- Block range checks (expiration only, not true nonce)

#### Pattern 3: Atomic Transfers with Escrow

```
1. User deposits USDC to an escrow smart contract
2. User signs authorization for facilitator to release funds
3. Facilitator submits atomic transaction group
4. Smart contract validates and releases funds
```

**Drawback**: Requires upfront deposit, not true "pay from wallet" flow.

### Technical Challenges

| Challenge | Severity | Workaround |
|-----------|----------|------------|
| No native EIP-3009 equivalent | High | Custom smart contract |
| USDC clawback controlled by Circle | High | Would need Circle partnership |
| Logic Signatures lack nonces | Medium | Stateful contract for nonce tracking |
| Opt-in requirement for ASAs | Medium | User must opt-in to USDC first |
| Minimum balance requirements | Low | 0.201 ALGO per ASA |

### ASA Opt-In Requirement

Unlike EVM tokens, Algorand requires users to **explicitly opt-in** to receive ASAs:

```
Minimum balance = 0.1 ALGO (base) + 0.1 ALGO (per ASA) + 0.001 ALGO (tx fee)
                = 0.201 ALGO for single ASA
```

This is a UX hurdle that doesn't exist on EVM chains.

### Fee Structure

| Transaction Type | Cost |
|-----------------|------|
| Standard transaction | 0.001 ALGO (~$0.0003) |
| ASA transfer | 0.001 ALGO |
| Smart contract call | 0.001-0.01 ALGO |

Fees are extremely low but the architecture complexity is the main concern.

### Integration Architecture for x402

Would require a custom approach:

```
Option A: Custom Escrow Contract
┌──────────────────────────────────────────────────────────────┐
│  User deposits USDC → Escrow Contract → Facilitator releases │
│  (Breaks "pay from wallet" UX of x402)                       │
└──────────────────────────────────────────────────────────────┘

Option B: Delegated Logic Signature (Complex)
┌──────────────────────────────────────────────────────────────┐
│  User signs LogicSig → Facilitator submits → Custom nonce    │
│  tracking via separate smart contract                        │
└──────────────────────────────────────────────────────────────┘
```

### Estimated Implementation Effort

| Task | Complexity | Notes |
|------|------------|-------|
| Design custom authorization scheme | Very High | Need novel approach |
| Implement nonce tracking contract | High | Stateful smart contract in TEAL/PyTeal |
| Integrate algonaut SDK | Medium | SDK is functional but less mature |
| Handle ASA opt-in flow | Medium | Additional UX step |
| **Total** | **Very High** | Not a direct port |

### Algorand Verdict: POSSIBLE BUT COMPLEX

**Pros**:
- Native USDC with decent liquidity ($100M+)
- Extremely low fees
- Fast finality (~4 seconds)
- Atomic transfers built-in
- Active Rust SDK (algonaut)

**Cons**:
- No EIP-3009 equivalent mechanism
- USDC clawback controlled by Circle
- Logic Signatures lack nonce support
- Requires custom smart contract development
- ASA opt-in requirement adds UX friction
- Would need to design novel authorization scheme

**Recommendation**: **DEFER** - Requires significant R&D to design a secure authorization scheme. Consider revisiting if Algorand adds native meta-transaction support.

---

## XRP Ledger Deep Dive

### Overview

The XRP Ledger (XRPL) is a decentralized blockchain known for fast, low-cost cross-border payments. It uses a unique consensus mechanism and has native support for issued currencies (tokens) via trust lines.

### USDC Availability

| Metric | Value |
|--------|-------|
| **Native USDC** | Yes - Circle-issued (June 2025) |
| **Issuer Address** | `rGm7WCVp9gb4jZHWTEtGUr4dd74z2XuWhE` |
| **Launch Date** | June 12, 2025 |
| **Total Chains** | XRPL is USDC's 22nd supported chain |

**Source**: [Circle USDC on XRPL](https://www.circle.com/blog/now-available-usdc-on-the-xrpl)

Additionally, Ripple's own stablecoin **RLUSD** is available on XRPL.

### Rust SDK Status

| SDK | Status | Maintainer |
|-----|--------|------------|
| **xrpl-rust** | Community, Active | sephynox (XRPL Grant Winner) |
| **xrpl-rs** | Community | Various |
| **xrpl-sdk-rust** | Community, Pre-alpha | gmosx/Keyrock |
| **xpring-rs** | Community | elmurci |

The most promising is `xrpl-rust` by sephynox, which won an XRPL grant. However, all Rust SDKs are community-maintained.

**Crate**: [xrpl on crates.io](https://crates.io/crates/xrpl)

### EIP-3009 Equivalent: NONE (Hooks Not on Mainnet)

**CRITICAL FINDING**: XRPL does **NOT** have smart contract capabilities on mainnet. The "Hooks" amendment that would add this is still on testnet.

#### Current XRPL Capabilities

| Feature | Status | EIP-3009 Relevance |
|---------|--------|-------------------|
| **Native XRP transfers** | Live | Can't authorize off-chain |
| **Trust line tokens (USDC)** | Live | Standard transfers only |
| **Escrow** | Live | Time/condition based, not signature based |
| **Payment channels** | Live | Off-chain streaming, not authorization |
| **Multi-signing** | Live | Multiple signers, not delegated |
| **Hooks (smart contracts)** | TESTNET ONLY | Would enable EIP-3009 equivalent |

#### Hooks Amendment Status

```
Current Status: TESTNET ONLY (as of December 2025)
Expected Mainnet: Unknown (no announced date)
Technology: WebAssembly modules attached to accounts
```

Hooks would allow:
- Pre-transaction logic (before a payment executes)
- Post-transaction logic (after a payment executes)
- Conditional payments based on arbitrary logic
- Custom authorization schemes

**Without Hooks, there is NO WAY to implement EIP-3009 equivalent on XRPL.**

#### Escrow: Not Suitable

XRPL's escrow feature uses **crypto-conditions** (PREIMAGE-SHA-256):

```javascript
// Escrow creation
{
    TransactionType: "EscrowCreate",
    Condition: "A0258020E3B0C44298FC1C149AFBF4C8996FB924...",  // SHA-256 condition
    ...
}

// Escrow finish (requires preimage)
{
    TransactionType: "EscrowFinish",
    Fulfillment: "A0028000",  // Preimage that hashes to condition
    ...
}
```

**Why escrow doesn't work for x402**:
1. Requires funds locked upfront (not pay-from-wallet)
2. Uses hash preimages, not cryptographic signatures
3. No replay protection mechanism
4. Anyone with the preimage can finish the escrow

#### Payment Channels: Not Suitable

Payment channels enable off-chain transactions but:
- Require channel creation (upfront lockup)
- Designed for streaming payments between two parties
- Not suitable for arbitrary facilitator settlement

### Technical Blockers

| Blocker | Severity | Can Be Worked Around? |
|---------|----------|----------------------|
| Hooks not on mainnet | **CRITICAL** | No |
| No signature-based authorization | **CRITICAL** | No |
| Trust line model (not account model) | Medium | Yes, but adds complexity |
| No native nonce system | High | Would need Hooks |

### Trust Line Model

XRPL uses trust lines instead of simple token balances:

```
Account A ←───Trust Line───→ Issuer
              (USDC balance)
```

Each trust line:
- Requires 0.2 XRP reserve
- Is bidirectional (issuer and holder)
- Can be frozen by issuer
- Requires explicit creation

This is different from EVM where tokens are simple mappings.

### Fee Structure

| Transaction Type | Cost |
|-----------------|------|
| Standard payment | ~0.00001 XRP (~$0.00002) |
| Trust line creation | 0.2 XRP reserve (~$0.40) |
| Escrow creation | ~0.00001 XRP + reserve |

Fees are extremely low, but reserves add friction.

### When Would XRPL Be Viable?

XRPL would become viable for x402 integration when:

1. **Hooks go live on mainnet** - This is the critical blocker
2. **Authorization Hook pattern established** - Community develops best practices
3. **USDC supports Hook-based transfers** - Circle enables this for their token

Estimated timeline: **Unknown** - Hooks have been in development since 2021.

### XRP Ledger Verdict: NOT FEASIBLE (YET)

**Pros**:
- Native USDC available (June 2025)
- Extremely low fees
- Fast finality (~3-5 seconds)
- Active community Rust SDKs
- Ripple's institutional focus

**Cons**:
- **No smart contracts on mainnet** (Hooks still testnet)
- No EIP-3009 equivalent mechanism possible today
- Trust line model adds complexity
- Reserve requirements for trust lines
- All Rust SDKs are community-maintained

**Recommendation**: **WAIT** - Monitor Hooks amendment progress. Revisit when Hooks are live on mainnet and have proven stability.

---

## Technical Comparison Matrix

### Authorization Mechanism Comparison

| Feature | EVM (EIP-3009) | Stellar (Soroban) | Algorand | XRPL |
|---------|----------------|-------------------|----------|------|
| Off-chain signing | EIP-712 typed data | Native auth entries | LogicSig | None (without Hooks) |
| Third-party submission | Yes | Yes | Yes | No |
| Nonce/replay protection | Contract-level | Host-level | Must implement | None |
| Expiration | Unix timestamps | Ledger numbers | Block range | N/A |
| Gas abstraction | Via transfer | Via simulation | Partial | N/A |

### SDK Comparison

| Feature | Stellar | Algorand | XRPL |
|---------|---------|----------|------|
| **Primary SDK** | soroban-sdk | algonaut | xrpl-rust |
| **Maintainer** | SDF (Official) | Community | Community |
| **Maturity** | High | Medium | Medium |
| **Documentation** | Excellent | Good | Good |
| **Active Development** | Yes | Yes | Yes |
| **crates.io** | Yes | Yes | Yes |

### USDC Comparison

| Feature | Stellar | Algorand | XRPL |
|---------|---------|----------|------|
| **Circulation** | $200M+ | $100M+ | $2M+ (new) |
| **Launch Date** | 2021 | 2020 | June 2025 |
| **Circle APIs** | Full support | Full support | Full support |
| **CCTP** | Coming (V2) | Unknown | Unknown |

### Fee Comparison

| Chain | Typical Fee | USD Equivalent |
|-------|-------------|----------------|
| Stellar (Soroban) | ~0.02 XLM | ~$0.002 |
| Algorand | 0.001 ALGO | ~$0.0003 |
| XRPL | 0.00001 XRP | ~$0.00002 |
| Base (for reference) | ~0.001 ETH L2 | ~$0.01-0.10 |

---

## Web Wallet Support Analysis (CRITICAL)

> **IMPORTANT**: This section addresses a critical integration concern. When we integrated NEAR Protocol, we discovered that popular wallets (Meteor, MyNearWallet, etc.) did NOT support `signDelegateAction` - the key function needed for meta-transactions. This section analyzes wallet support for each chain to avoid similar issues.

### What We Need from Wallets

For x402-style payments, web wallets must support:

1. **Off-chain Authorization Signing**: Sign authorization data without broadcasting
2. **dApp Integration API**: Programmatic access for web applications
3. **User-Friendly Approval Flow**: Clear display of what user is authorizing
4. **Support for the Specific Function**: Each chain has its own equivalent to EIP-3009

### Stellar Wallet Support: EXCELLENT

| Wallet | Soroban Support | Authorization Entry Signing | dApp Integration |
|--------|----------------|---------------------------|-----------------|
| **Freighter** | Full | `signAuthEntry` API | Excellent |
| **Albedo** | Full | Supported | Good |
| **XBull** | Full | Supported | Good |
| **LOBSTR** | Partial | Unknown | Limited |
| **Hana** | Full | Supported | Good |

#### Freighter: THE KEY WALLET

Freighter is the **official Stellar Development Foundation wallet** and has **first-class Soroban support**:

```javascript
import { signAuthEntry, signTransaction } from '@stellar/freighter-api';

// Sign authorization entries for Soroban contracts
const signedAuth = await signAuthEntry({
    entryXdr: authEntryXdr,
    networkPassphrase: 'Public Global Stellar Network ; September 2015'
});
```

**Key Features**:
- `signAuthEntry()` - **THIS IS EXACTLY WHAT WE NEED** - signs Soroban authorization entries
- `signTransaction()` - Signs full transactions
- `signBlob()` - Signs arbitrary data
- Transaction simulation preview shows users what they're authorizing
- React integration with `@stellar/freighter-api`

**Documentation**: [Sign Authorization Entries](https://developers.stellar.org/docs/build/guides/freighter/sign-auth-entries)

#### Stellar Wallets Kit

For multi-wallet support, use [Stellar Wallets Kit](https://stellarwalletskit.dev/):

```javascript
// Supports: Albedo, Freighter, Rabet, WalletConnect, Lobstr, Hana, Hot Wallet, Klever, xBull
import { StellarWalletsKit } from '@creit.tech/stellar-wallets-kit';
```

**STELLAR WALLET VERDICT**: **READY FOR INTEGRATION**

Multiple production wallets support Soroban authorization entry signing. Freighter's `signAuthEntry` API is the exact equivalent of what we need for x402.

---

### Algorand Wallet Support: PROBLEMATIC

| Wallet | LogicSig Signing | Delegated Signatures | ARC-47 Support |
|--------|-----------------|---------------------|----------------|
| **Pera Wallet** | Standard only | **NO** | Unknown |
| **MyAlgo** | Standard only | **NO** | Unknown |
| **Defly** | Standard only | **NO** | Unknown |

#### Critical Finding: Wallets REFUSE Delegated LogicSigs

> **"Currently, most Algorand wallets do not enable the signing of logic signature programs for the purpose of delegation. The rationale is to prevent users from signing malicious programs."**
>
> — [ARC-47 Standard](https://arc.algorand.foundation/ARCs/arc-0047)

**Why This Is a Problem**:

Delegated Logic Signatures are the Algorand equivalent of EIP-3009, but wallets deliberately block them:

1. **Security Concern**: A malicious LogicSig could drain user's account indefinitely
2. **No Expiration by Default**: LogicSigs don't expire unless explicitly programmed
3. **User Can't Understand**: Unlike Soroban's clear authorization display, LogicSig bytecode is opaque

#### ARC-47: Proposed Solution (NOT WIDELY ADOPTED)

ARC-47 proposes "Templated Logic Signatures" to allow safe delegated signing:

```javascript
// ARC-47 wallet method (IF supported)
const result = await wallet.algo_templatedLsig({
    lsigJson: canonicalizedArc47Json,
    // ... parameters
});
```

**Problem**: There is **NO CONFIRMATION** that Pera Wallet, MyAlgo, or other major wallets have implemented ARC-47.

#### Workaround Attempts

1. **Atomic Transactions with Wallet Signing**: Works but requires user to sign each transaction (not gasless)
2. **Escrow Pattern**: Requires upfront deposit (breaks x402 UX)
3. **Custom Wallet/dApp**: Would need to build our own wallet integration

**ALGORAND WALLET VERDICT**: **NOT READY**

Major Algorand wallets deliberately do not support delegated signature signing. ARC-47 exists as a standard but adoption is unclear. This is a **fundamental blocker** that cannot be worked around without either:
- Circle implementing a different authorization mechanism for USDC
- Wallets adopting ARC-47 (uncertain timeline)
- Building custom wallet integration (high effort, poor UX)

---

### XRP Ledger Wallet Support: IRRELEVANT (Protocol Limitation)

| Wallet | Transaction Signing | Offline Signing | Hooks Support |
|--------|--------------------|-----------------|--------------|
| **Xumm/Xaman** | Full | Partial | N/A |
| **GemWallet** | Full | Yes | N/A |
| **Crossmark** | Full | Yes | N/A |

#### The Problem is NOT the Wallets

XRP Ledger wallets are actually quite capable:

- **Xumm/Xaman**: Signs all XRPL transaction types, has dApp integration
- **GemWallet**: Browser extension with API for web3 integration
- **Crossmark**: Clean SDK, easy integration

**BUT**: The limitation is at the **protocol level**, not wallet level.

Without Hooks on mainnet, there is NO transaction type that enables EIP-3009 style authorization. The wallets could sign whatever we need - but there's nothing to sign that would work.

```
Current XRPL Transaction Types:
- Payment (requires sender signature at submit time)
- EscrowCreate/Finish (requires preimage, not signature)
- PaymentChannelCreate/Claim (requires channel setup)
- TrustSet (only for trust line management)

Missing: AuthorizedTransfer, DelegatedPayment, or similar
```

**XRP WALLET VERDICT**: **PROTOCOL BLOCKED**

Wallets are technically capable, but XRPL lacks the protocol-level transaction type needed. This will change when Hooks launch on mainnet.

---

### Wallet Support Summary

| Chain | Wallet Ready? | Required Function | Blocker |
|-------|--------------|-------------------|---------|
| **Stellar** | **YES** | `signAuthEntry` | None |
| **Algorand** | **NO** | `algo_templatedLsig` (ARC-47) | Wallets refuse delegated signing |
| **XRPL** | N/A | N/A | Protocol lacks feature |

### Lessons Learned from NEAR Integration

Our NEAR integration revealed that:

1. **Protocol support ≠ Wallet support**: NEAR has `signDelegateAction` at protocol level, but wallets don't expose it
2. **Check wallet APIs first**: Before investing in integration, verify wallet support
3. **Official wallets are key**: SDF (Stellar) maintains Freighter with full Soroban support; NEAR Foundation wallets lag

**Stellar avoids this problem** because:
- Freighter is maintained by Stellar Development Foundation (official)
- Soroban was designed with wallet integration in mind
- `signAuthEntry` API was part of the initial Soroban launch

---

## Implementation Roadmap

### Phase 1: Stellar Integration (IMMEDIATE)

**Timeline Estimate**: 2-3 weeks development + 1 week testing

```
Week 1:
├── Add NetworkFamily::Stellar variant
├── Implement Stellar provider/RPC client
├── Add USDC contract addresses (mainnet + testnet)
└── Set up Soroban RPC connections

Week 2:
├── Implement verify() for Stellar authorization
├── Implement settle() for Stellar token transfers
├── Add transaction simulation for fees
└── Handle authorization entry signing

Week 3:
├── Integration tests with Stellar testnet
├── Test with real USDC on mainnet (small amounts)
├── Documentation and code review
└── Deploy to production
```

**Key Implementation Files**:
- `src/chain/stellar.rs` (new)
- `src/network.rs` (add Stellar networks)
- `src/from_env.rs` (add Stellar RPC URLs)
- `src/facilitator_local.rs` (add Stellar delegation)

### Phase 2: Monitor Algorand & XRPL (DEFER)

**Algorand**:
- Monitor for native meta-transaction support
- Track algonaut SDK development
- Consider if Circle adds delegated transfer support

**XRPL**:
- Monitor Hooks amendment progress toward mainnet
- Track community authorization patterns on Hooks testnet
- Revisit when Hooks are stable on mainnet

---

## Sources and References

### Stellar / Soroban

- [Circle USDC on Stellar](https://www.circle.com/multi-chain-usdc/stellar)
- [Soroban SDK GitHub](https://github.com/stellar/rs-soroban-sdk)
- [Soroban Authorization Documentation](https://developers.stellar.org/docs/learn/smart-contract-internals/authorization)
- [SEP-41 Token Interface](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0041.md)
- [Stellar Asset Contract](https://developers.stellar.org/docs/tokens/stellar-asset-contract)
- [Transaction Simulation](https://developers.stellar.org/docs/learn/fundamentals/contract-development/contract-interactions/transaction-simulation)
- [Soroban Fees Guide](https://cheesecakelabs.com/blog/how-much-do-soroban-fees-cost/)

### Algorand

- [Circle USDC on Algorand](https://www.circle.com/multi-chain-usdc/algorand)
- [Algonaut Rust SDK](https://github.com/manuelmauro/algonaut)
- [Algorand ASA Documentation](https://developer.algorand.org/docs/get-details/asa/)
- [Algorand Atomic Transfers](https://developer.algorand.org/docs/get-details/atomic_transfers/)
- [Algorand Smart Signatures](https://developer.algorand.org/docs/get-details/dapps/smart-contracts/smartsigs/)
- [Assets and Custom Transfer Logic](https://developer.algorand.org/solutions/assets-and-custom-transfer-logic/)

### XRP Ledger

- [Circle USDC on XRPL](https://www.circle.com/blog/now-available-usdc-on-the-xrpl)
- [Ripple USDC Announcement](https://ripple.com/ripple-press/ripple-and-circle-launch-usdc-on-the-xrp-ledger/)
- [xrpl-rust SDK](https://github.com/sephynox/xrpl-rust)
- [XRPL Hooks](https://xrpl-hooks.readme.io/)
- [XRPL Hooks Status](https://hooks.xrpl.org/)
- [XRPL Escrow](https://xrpl.org/docs/concepts/payment-types/escrow)
- [XRPL Trust Lines](https://xrpl.org/docs/concepts/tokens/fungible-tokens)

### EIP-3009 (Reference)

- [EIP-3009 Specification](https://eips.ethereum.org/EIPS/eip-3009)
- [Circle EIP-3009 Implementation](https://github.com/CoinbaseStablecoin/eip-3009)

---

## Appendix A: Stellar Code Snippets

### Authorization Entry Structure

```rust
use soroban_sdk::{Address, Env, token};

// Creating a token client
let client = token::TokenClient::new(&env, &usdc_contract_id);

// Transfer requires authorization from 'from' address
// The authorization is verified by the Soroban host
client.transfer(&from, &to, &amount);
```

### Simulating Transaction for Authorization

```bash
# Get authorization requirements
stellar contract invoke \
  --network mainnet \
  --source-account $FACILITATOR \
  --id CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75 \
  -- transfer \
  --from $USER \
  --to $RECIPIENT \
  --amount 1000000 \
  --simulate-only
```

### Fee Estimation

```rust
// Simulation returns resource estimates
let simulation = rpc_client.simulate_transaction(&tx).await?;

let resource_fee = simulation.min_resource_fee;
let refundable_fee = simulation.refundable_fee;
let total_fee = resource_fee + refundable_fee;
```

---

## Appendix B: Decision Matrix

### Weighted Scoring (1-5 scale)

| Criterion | Weight | Stellar | Algorand | XRPL |
|-----------|--------|---------|----------|------|
| EIP-3009 equivalent | 25% | 5 | 2 | 0 |
| **Web Wallet Support** | **25%** | **5** | **0** | **0** |
| USDC availability | 15% | 5 | 4 | 3 |
| Rust SDK quality | 15% | 5 | 3 | 3 |
| Implementation complexity | 10% | 4 | 2 | 1 |
| Ecosystem maturity | 10% | 4 | 4 | 4 |
| **Weighted Score** | 100% | **4.75** | **1.90** | **1.40** |

**Winner: Stellar/Soroban with a decisive lead**

> **Note on Web Wallet Support Weight**: After our NEAR integration experience, we now weight wallet support equally with protocol support. A chain with perfect protocol support but no wallet support is **unusable** for end users.

### Why Algorand Score Dropped

In the original analysis, Algorand scored 2.85. After including wallet support as a criterion:
- Protocol support: 2/5 (has LogicSig pattern but requires custom work)
- **Wallet support: 0/5** (wallets deliberately refuse delegated signing)

This drops Algorand from "possible with effort" to "not viable without wallet ecosystem changes."

---

## Conclusion

After extensive research, **Stellar/Soroban is the clear choice** for immediate integration with x402-rs:

1. **Native USDC**: $200M+ circulation, Circle-issued
2. **Official Rust SDK**: `soroban-sdk` actively maintained by Stellar Development Foundation
3. **Built-in Authorization**: Soroban's `require_auth` + pre-signed entries closely mirror EIP-3009
4. **Transaction Simulation**: Fee estimation and authorization preview built into RPC
5. **Low Fees**: ~$0.02 per transaction
6. **CRITICAL - Web Wallet Support**: Freighter wallet has `signAuthEntry` API - exactly what we need

### Why NOT Algorand or XRPL

| Chain | Technical Blocker | Wallet Blocker |
|-------|-------------------|----------------|
| **Algorand** | Partial (needs custom work) | **FATAL**: Wallets refuse delegated signing |
| **XRPL** | **FATAL**: Hooks not on mainnet | N/A (protocol issue) |

**Algorand Lesson**: Even with protocol support, wallet ecosystem matters. Pera and MyAlgo deliberately refuse to sign delegated Logic Signatures for security reasons - the exact same problem we hit with NEAR wallets not supporting `signDelegateAction`.

**XRPL Outlook**: When Hooks launch on mainnet, XRPL becomes viable. Until then, there's no technical path forward regardless of wallet support.

**Recommended Next Step**: Begin Stellar integration immediately following the implementation roadmap in this document. The wallet ecosystem is ready, the protocol is ready, and the SDK is official.
