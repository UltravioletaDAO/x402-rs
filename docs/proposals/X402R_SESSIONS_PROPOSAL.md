# x402r Sessions: Partial Refunds & Pay-Per-Use Extension

**Proposal for x402r Protocol Enhancement**

**Author:** Ultravioleta DAO
**Date:** December 25, 2024
**Status:** Draft Proposal
**Target:** BackTrack / Ali Abdoli (x402r-contracts)

---

## Executive Summary

We propose extending x402r to support **session-based payments** with **partial refunds**. This enables use cases like:

- Pay-per-message consulting ($5 per response, refund unused balance)
- API usage credits (pay $100, use $30, get $70 back)
- Streaming content with refund protection
- Service subscriptions with pro-rata refunds

**Key Innovation:** Single deposit transaction + single refund transaction, regardless of how many "units" are consumed in between.

---

## Problem Statement

### Current x402r Limitation

```
TODAY: Binary outcome only

User deposits $50
     |
     +---> Full refund ($50) to buyer
     |        OR
     +---> Full release ($50) to seller

NO middle ground.
```

### Real-World Need

```
DESIRED: Partial consumption

User deposits $50 for consulting session
     |
     |-- Uses 2 questions @ $5 each = $10 consumed
     |
     +---> $10 to seller (for work done)
     +---> $40 to buyer (unused balance)
```

---

## Proposed Solution: SessionEscrow

### New Contract: SessionEscrow.sol

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";

/**
 * @title SessionEscrow
 * @notice Escrow contract supporting partial refunds and pay-per-use sessions
 * @dev Extension of x402r for session-based payments
 */
contract SessionEscrow {
    using SafeERC20 for IERC20;
    using ECDSA for bytes32;
    using MessageHashUtils for bytes32;

    // ============ Structs ============

    struct Session {
        address buyer;
        address seller;
        address token;
        uint256 totalDeposit;      // Total amount deposited (e.g., $50)
        uint256 consumedAmount;    // Amount used so far (e.g., $10)
        uint256 pricePerUnit;      // Cost per unit of service (e.g., $5)
        uint256 maxUnits;          // Maximum units purchasable
        uint256 unitsUsed;         // Units consumed so far
        uint256 createdAt;
        uint256 expiresAt;         // Session expiration timestamp
        SessionStatus status;
    }

    enum SessionStatus {
        Active,
        Completed,
        Expired,
        Disputed
    }

    // ============ State ============

    mapping(bytes32 => Session) public sessions;
    mapping(address => bool) public registeredSellers;
    mapping(address => address) public sellerArbiters;

    uint256 public sessionCount;

    // ============ Events ============

    event SessionCreated(
        bytes32 indexed sessionId,
        address indexed buyer,
        address indexed seller,
        uint256 totalDeposit,
        uint256 pricePerUnit,
        uint256 maxUnits,
        uint256 expiresAt
    );

    event UnitsConsumed(
        bytes32 indexed sessionId,
        uint256 units,
        uint256 amount,
        uint256 totalConsumed
    );

    event SessionClosed(
        bytes32 indexed sessionId,
        uint256 sellerAmount,
        uint256 buyerRefund,
        SessionStatus finalStatus
    );

    event SessionDisputed(
        bytes32 indexed sessionId,
        address initiator,
        string reason
    );

    // ============ Errors ============

    error SessionNotFound();
    error SessionNotActive();
    error SessionExpired();
    error InsufficientBalance();
    error UnauthorizedCaller();
    error InvalidSignature();
    error ExceedsMaxUnits();
    error SellerNotRegistered();

    // ============ Seller Registration ============

    /**
     * @notice Register as a seller with an arbiter for disputes
     * @param arbiter Address that can resolve disputes
     */
    function registerSeller(address arbiter) external {
        registeredSellers[msg.sender] = true;
        sellerArbiters[msg.sender] = arbiter;
    }

    // ============ Session Management ============

    /**
     * @notice Create a new payment session
     * @param seller Address of the service provider
     * @param token ERC20 token for payment
     * @param totalDeposit Total amount to deposit
     * @param pricePerUnit Cost per unit of service
     * @param duration Session duration in seconds
     * @return sessionId Unique identifier for this session
     */
    function createSession(
        address seller,
        address token,
        uint256 totalDeposit,
        uint256 pricePerUnit,
        uint256 duration
    ) external returns (bytes32 sessionId) {
        if (!registeredSellers[seller]) revert SellerNotRegistered();

        // Calculate max units from deposit
        uint256 maxUnits = totalDeposit / pricePerUnit;

        // Generate unique session ID
        sessionId = keccak256(abi.encodePacked(
            msg.sender,
            seller,
            block.timestamp,
            sessionCount++
        ));

        // Create session
        sessions[sessionId] = Session({
            buyer: msg.sender,
            seller: seller,
            token: token,
            totalDeposit: totalDeposit,
            consumedAmount: 0,
            pricePerUnit: pricePerUnit,
            maxUnits: maxUnits,
            unitsUsed: 0,
            createdAt: block.timestamp,
            expiresAt: block.timestamp + duration,
            status: SessionStatus.Active
        });

        // Transfer tokens to escrow
        IERC20(token).safeTransferFrom(msg.sender, address(this), totalDeposit);

        emit SessionCreated(
            sessionId,
            msg.sender,
            seller,
            totalDeposit,
            pricePerUnit,
            maxUnits,
            block.timestamp + duration
        );
    }

    /**
     * @notice Consume units from a session (called by seller with buyer signature)
     * @param sessionId Session identifier
     * @param units Number of units to consume
     * @param buyerSignature Buyer's signature authorizing consumption
     * @param nonce Unique nonce to prevent replay
     */
    function consumeUnits(
        bytes32 sessionId,
        uint256 units,
        bytes calldata buyerSignature,
        bytes32 nonce
    ) external {
        Session storage session = sessions[sessionId];

        if (session.buyer == address(0)) revert SessionNotFound();
        if (session.status != SessionStatus.Active) revert SessionNotActive();
        if (block.timestamp > session.expiresAt) revert SessionExpired();
        if (msg.sender != session.seller) revert UnauthorizedCaller();
        if (session.unitsUsed + units > session.maxUnits) revert ExceedsMaxUnits();

        // Verify buyer authorized this consumption
        bytes32 messageHash = keccak256(abi.encodePacked(
            sessionId,
            units,
            nonce,
            block.chainid
        ));
        bytes32 ethSignedHash = messageHash.toEthSignedMessageHash();
        address signer = ethSignedHash.recover(buyerSignature);

        if (signer != session.buyer) revert InvalidSignature();

        // Update consumption
        uint256 amount = units * session.pricePerUnit;
        session.unitsUsed += units;
        session.consumedAmount += amount;

        emit UnitsConsumed(sessionId, units, amount, session.consumedAmount);
    }

    /**
     * @notice Consume units using EIP-712 typed signature (gas-optimized)
     * @param sessionId Session identifier
     * @param units Number of units to consume
     * @param v Signature v
     * @param r Signature r
     * @param s Signature s
     */
    function consumeUnitsWithPermit(
        bytes32 sessionId,
        uint256 units,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external {
        Session storage session = sessions[sessionId];

        if (session.buyer == address(0)) revert SessionNotFound();
        if (session.status != SessionStatus.Active) revert SessionNotActive();
        if (block.timestamp > session.expiresAt) revert SessionExpired();
        if (msg.sender != session.seller) revert UnauthorizedCaller();
        if (session.unitsUsed + units > session.maxUnits) revert ExceedsMaxUnits();

        // EIP-712 signature verification
        bytes32 structHash = keccak256(abi.encode(
            keccak256("ConsumeUnits(bytes32 sessionId,uint256 units,uint256 currentUnitsUsed)"),
            sessionId,
            units,
            session.unitsUsed  // Include current state to prevent double-spend
        ));

        bytes32 digest = keccak256(abi.encodePacked(
            "\x19\x01",
            _domainSeparator(),
            structHash
        ));

        address signer = ecrecover(digest, v, r, s);
        if (signer != session.buyer) revert InvalidSignature();

        // Update consumption
        uint256 amount = units * session.pricePerUnit;
        session.unitsUsed += units;
        session.consumedAmount += amount;

        emit UnitsConsumed(sessionId, units, amount, session.consumedAmount);
    }

    /**
     * @notice Close a session and distribute funds
     * @param sessionId Session identifier
     * @dev Can be called by buyer, seller, or after expiration by anyone
     */
    function closeSession(bytes32 sessionId) external {
        Session storage session = sessions[sessionId];

        if (session.buyer == address(0)) revert SessionNotFound();
        if (session.status != SessionStatus.Active) revert SessionNotActive();

        // Only buyer, seller, or anyone after expiration can close
        bool canClose = (
            msg.sender == session.buyer ||
            msg.sender == session.seller ||
            block.timestamp > session.expiresAt
        );

        if (!canClose) revert UnauthorizedCaller();

        // Mark as completed
        session.status = SessionStatus.Completed;

        // Calculate distribution
        uint256 sellerAmount = session.consumedAmount;
        uint256 buyerRefund = session.totalDeposit - session.consumedAmount;

        // Transfer to seller (consumed amount)
        if (sellerAmount > 0) {
            IERC20(session.token).safeTransfer(session.seller, sellerAmount);
        }

        // Refund to buyer (unused amount)
        if (buyerRefund > 0) {
            IERC20(session.token).safeTransfer(session.buyer, buyerRefund);
        }

        emit SessionClosed(sessionId, sellerAmount, buyerRefund, SessionStatus.Completed);
    }

    /**
     * @notice Initiate a dispute (freezes the session)
     * @param sessionId Session identifier
     * @param reason Description of the dispute
     */
    function initiateDispute(bytes32 sessionId, string calldata reason) external {
        Session storage session = sessions[sessionId];

        if (session.buyer == address(0)) revert SessionNotFound();
        if (session.status != SessionStatus.Active) revert SessionNotActive();
        if (msg.sender != session.buyer && msg.sender != session.seller) {
            revert UnauthorizedCaller();
        }

        session.status = SessionStatus.Disputed;

        emit SessionDisputed(sessionId, msg.sender, reason);
    }

    /**
     * @notice Resolve a dispute (arbiter only)
     * @param sessionId Session identifier
     * @param sellerPercent Percentage (0-100) of remaining funds to give seller
     */
    function resolveDispute(
        bytes32 sessionId,
        uint256 sellerPercent
    ) external {
        Session storage session = sessions[sessionId];

        if (session.buyer == address(0)) revert SessionNotFound();
        if (session.status != SessionStatus.Disputed) revert SessionNotActive();

        address arbiter = sellerArbiters[session.seller];
        if (msg.sender != arbiter) revert UnauthorizedCaller();

        session.status = SessionStatus.Completed;

        // Calculate disputed amount (what hasn't been consumed yet)
        uint256 disputedAmount = session.totalDeposit - session.consumedAmount;
        uint256 sellerExtra = (disputedAmount * sellerPercent) / 100;
        uint256 buyerRefund = disputedAmount - sellerExtra;

        // Already consumed amount always goes to seller
        uint256 totalToSeller = session.consumedAmount + sellerExtra;

        if (totalToSeller > 0) {
            IERC20(session.token).safeTransfer(session.seller, totalToSeller);
        }
        if (buyerRefund > 0) {
            IERC20(session.token).safeTransfer(session.buyer, buyerRefund);
        }

        emit SessionClosed(sessionId, totalToSeller, buyerRefund, SessionStatus.Completed);
    }

    // ============ View Functions ============

    /**
     * @notice Get session details
     */
    function getSession(bytes32 sessionId) external view returns (Session memory) {
        return sessions[sessionId];
    }

    /**
     * @notice Get remaining balance in a session
     */
    function getRemainingBalance(bytes32 sessionId) external view returns (uint256) {
        Session storage session = sessions[sessionId];
        return session.totalDeposit - session.consumedAmount;
    }

    /**
     * @notice Get remaining units in a session
     */
    function getRemainingUnits(bytes32 sessionId) external view returns (uint256) {
        Session storage session = sessions[sessionId];
        return session.maxUnits - session.unitsUsed;
    }

    // ============ Internal ============

    function _domainSeparator() internal view returns (bytes32) {
        return keccak256(abi.encode(
            keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
            keccak256("SessionEscrow"),
            keccak256("1"),
            block.chainid,
            address(this)
        ));
    }
}
```

---

## Integration with Existing x402r

### Option A: Separate Contract

Deploy `SessionEscrow` alongside existing `Escrow`:

```
x402r Contracts
├── Escrow.sol           (existing - full refund/release)
├── SessionEscrow.sol    (NEW - partial refunds)
├── DepositRelay.sol     (existing)
└── DepositRelayFactory.sol (existing)
```

### Option B: Extend Existing Escrow

Add session functions to current `Escrow.sol`:

```solidity
// In existing Escrow.sol, add:

mapping(bytes32 => Session) public sessions;

function createSession(...) external { ... }
function consumeUnits(...) external { ... }
function closeSession(...) external { ... }
```

**Recommendation:** Option A for cleaner separation and easier auditing.

---

## Protocol Extension Format

### New Extension: `session`

```json
{
  "x402Version": 2,
  "paymentPayload": {
    "accepted": {
      "scheme": "exact",
      "network": "eip155:8453",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "amount": "50000000",
      "payTo": "0xSESSION_ESCROW_ADDRESS"
    },
    "extensions": {
      "session": {
        "type": "prepaid",
        "pricePerUnit": "5000000",
        "maxUnits": 10,
        "duration": 3600,
        "seller": "0xEXPERT_ADDRESS",
        "description": "Consulting session - $5 per response"
      }
    }
  }
}
```

### Consume Units Request

```json
{
  "sessionId": "0xabc123...",
  "units": 1,
  "buyerSignature": "0x...",
  "nonce": "0xdef456..."
}
```

### Close Session Request

```json
{
  "sessionId": "0xabc123..."
}
```

---

## Use Case Examples

### 1. AI Consulting Platform

```
Expert charges $5 per response
User deposits $50 (10 responses max)

Flow:
1. User: "How do I optimize my smart contract?"
   Expert responds --> consumeUnits(1)

2. User: "What about gas costs?"
   Expert responds --> consumeUnits(1)

3. User satisfied, closes session
   Expert receives: $10
   User refund: $40
```

### 2. API Credits

```
API charges $0.01 per request
User deposits $10 (1000 requests max)

Flow:
- User makes 347 API calls
- Each call: consumeUnits(1)
- User stops using API
- closeSession()
  API provider: $3.47
  User refund: $6.53
```

### 3. Content Subscription with Pro-rata Refund

```
Monthly subscription: $30
User deposits for 30 days

Flow:
- Day 1-12: User enjoys content
  Daily: consumeUnits(1) @ $1/day

- Day 13: User cancels
  closeSession()
  Provider: $12
  User refund: $18
```

---

## Gas Optimization Considerations

### Batch Consumption

For high-frequency use cases (APIs), support batch consumption:

```solidity
function consumeUnitsBatch(
    bytes32 sessionId,
    uint256 totalUnits,
    bytes calldata aggregateSignature
) external {
    // Consume multiple units in single tx
}
```

### Off-chain Aggregation

For very high frequency:

```
Off-chain:
  Request 1 --> signed receipt
  Request 2 --> signed receipt
  Request 3 --> signed receipt
  ...
  Request 100 --> signed receipt

On-chain (once per hour/day):
  consumeUnits(100, aggregatedProof)
```

---

## Security Considerations

1. **Signature Replay Protection**
   - Use nonces or include `unitsUsed` in signature to prevent replay

2. **Session Expiration**
   - Auto-refund remaining balance after expiration
   - Seller cannot consume units after expiry

3. **Dispute Resolution**
   - Arbiter can only distribute REMAINING funds
   - Already-consumed amount always goes to seller

4. **Front-running Protection**
   - EIP-712 signatures include chain ID
   - Session ID includes timestamp

---

## Deployment Plan

### Phase 1: Testnet (Base Sepolia)

1. Deploy `SessionEscrow` contract
2. Update x402r SDK to support `session` extension
3. Update facilitator to route session requests
4. Test with demo consulting dApp

### Phase 2: Mainnet (Base)

1. Audit `SessionEscrow` contract
2. Deploy to mainnet
3. Gradual rollout with rate limits

### Phase 3: Multi-chain

1. Deploy to other supported networks
2. Add cross-chain session support (future)

---

## Questions for Discussion

1. **Signature scheme:** EIP-712 vs simple ECDSA for unit consumption?

2. **Expiration behavior:** Auto-close and refund, or require explicit close?

3. **Dispute window:** Should there be a grace period after close for disputes?

4. **Integration:** Separate contract or extend existing Escrow?

5. **Fee structure:** Should protocol take a fee on sessions?

---

## Next Steps

1. Review this proposal and provide feedback
2. Agree on contract architecture (Option A vs B)
3. Implement `SessionEscrow.sol`
4. Deploy to Base Sepolia for testing
5. Ultravioleta DAO implements facilitator support
6. Joint testing with demo application
7. Security audit
8. Mainnet deployment

---

## Contact

**Ultravioleta DAO**
- GitHub: https://github.com/UltravioletaDAO
- Facilitator: https://facilitator.ultravioletadao.xyz

**BackTrack (Ali)**
- x402r Contracts: https://github.com/BackTrackCo/x402r-contracts
- x402r Proposal: https://github.com/coinbase/x402/issues/864

---

*This proposal is open for discussion and modification based on feedback from the x402r community.*
