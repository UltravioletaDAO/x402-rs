# Chamba + x402r Advanced Escrow Integration Guide

## Overview

This guide explains how to use the x402r PaymentOperator escrow system for Chamba's task marketplace. The advanced escrow provides 5 distinct flows that map perfectly to Chamba's use cases.

## Escrow Lifecycle Diagram

```
                    ┌─────────────────────────────────────────────────────────┐
                    │                  ESCROW LIFECYCLE                        │
                    └─────────────────────────────────────────────────────────┘

    ┌──────────────┐
    │   AGENT      │ ──── Posts task with bounty
    │   (Payer)    │
    └──────┬───────┘
           │
           ▼
    ╔══════════════╗
    ║  AUTHORIZE   ║ ──── Funds move from Agent → TokenStore (escrow)
    ╚══════╤═══════╝
           │
           │ Worker accepts task
           │
           ├────────────────────────────────────────────────────┐
           │                                                    │
           ▼                                                    ▼
    ┌──────────────┐                                    ┌──────────────┐
    │ Worker       │                                    │ Agent        │
    │ COMPLETES    │                                    │ CANCELS      │
    │ task         │                                    │ task         │
    └──────┬───────┘                                    └──────┬───────┘
           │                                                   │
           ▼                                                   ▼
    ╔══════════════╗                                    ╔══════════════╗
    ║    CHARGE    ║ ──── Funds: Escrow → Worker       ║   RELEASE    ║ ──── Funds: Escrow → Agent
    ╚══════╤═══════╝                                    ╚══════════════╝
           │
           │ Quality issue discovered
           │
           ▼
    ╔══════════════════╗
    ║ REFUND POST      ║ ──── Dispute resolution, funds may return
    ║ ESCROW           ║
    ╚══════════════════╝

    ALTERNATIVE: Worker encounters obstacle BEFORE charge

    ╔══════════════╗        ╔══════════════════╗
    ║  AUTHORIZE   ║ ────── ║ REFUND IN        ║ ──── Funds: Escrow → Agent
    ╚══════════════╝        ║ ESCROW           ║      (Proof of Attempt)
                            ╚══════════════════╝
```

---

## The 5 Escrow Flows with Chamba Examples

### 1. AUTHORIZE - Agent Posts Task

**What happens**: Agent's funds move from their wallet to TokenStore (escrow contract).

**Chamba scenario**:
```
Agent: "I need someone to verify if Cafe Velvet in El Poblado is open"
Bounty: $3 USDC
Bond: $0.45 USDC (15%)
Total locked: $3.45 USDC

Timeline:
  - preApprovalExpiry: 2 hours (worker must accept within this time)
  - authorizationExpiry: 24 hours (worker must complete + agent approve)
  - refundExpiry: 7 days (dispute window)
```

**Code**:
```python
payment_info = {
    "operator": PAYMENT_OPERATOR,
    "receiver": worker_address,  # Will receive payment
    "token": USDC,
    "maxAmount": 3_000_000,  # $3.00 (6 decimals)
    "preApprovalExpiry": now + 7200,     # 2 hours
    "authorizationExpiry": now + 86400,  # 24 hours
    "refundExpiry": now + 604800,        # 7 days
    "minFeeBps": 0,
    "maxFeeBps": 400,  # 4% max platform fee
    "feeReceiver": PAYMENT_OPERATOR,
    "salt": random_salt,
}
```

---

### 2. CHARGE - Task Completed Successfully

**What happens**: Agent approves work, funds move from TokenStore to Worker.

**Chamba scenario**:
```
1. Worker accepts task
2. Worker goes to Cafe Velvet, takes photo with timestamp + GPS
3. Worker uploads evidence to Chamba
4. Chamba auto-verification:
   - GPS matches location: ✓
   - Timestamp is recent: ✓
   - Photo shows cafe open: ✓ (AI Vision)
5. Agent's AI confirms: "Photo shows cafe is open at 3:47 PM"
6. CHARGE executed: Worker receives $3.00 USDC instantly
```

**Partial payout option** (Chamba feature):
```
On submission: Worker gets 40% immediately ($1.20)
On approval: Worker gets remaining 60% ($1.80)
Total: $3.00
```

---

### 3. RELEASE - Agent Cancels Task

**What happens**: Agent decides not to proceed, funds return from TokenStore to Agent.

**Chamba scenarios**:

**Scenario A: No workers accepted**
```
Agent: "Video of sunset from Cerro Nutibara" - $15
24 hours pass, no workers accept
Agent: RELEASE → $15 returned
Agent re-posts with higher bounty
```

**Scenario B: External conditions changed**
```
Agent: "Take photo of open-air concert tonight" - $20
Weather forecast: 100% chance of rain
Concert cancelled
Agent: RELEASE before any worker accepts → $20 returned
```

**Scenario C: Agent made mistake**
```
Agent: "Buy item at store X" - $25
Agent realizes: wrong store address in description
Agent: RELEASE → $25 returned
Agent re-posts with corrected address
```

---

### 4. REFUND IN ESCROW - Proof of Attempt

**What happens**: Worker encountered legitimate obstacle, funds return to Agent (with optional partial payment).

**Chamba scenario**:
```
Agent: "Get notarized copy of document at Notary Office #42" - $50
Worker accepts, goes to notary
Problem: Office is permanently closed (out of business)

Worker submits "Proof of Attempt":
  - Photo of closed sign with GPS
  - Screenshot of Google Maps showing "Permanently closed"
  - Timestamp proving visit

Chamba verification:
  - GPS matches notary location: ✓
  - Evidence shows legitimate obstacle: ✓

Resolution:
  - Worker receives: $7.50 (15% for genuine attempt)
  - Agent receives: $42.50 refund
  - Task status: "Cancelled - Location unavailable"
```

**Valid obstacles in Chamba**:
- Location closed/doesn't exist
- Item out of stock
- Access denied (private property, security)
- Weather emergency
- Physical impossibility

---

### 5. REFUND POST ESCROW - Quality Dispute

**What happens**: After payment, Agent discovers quality issue. Requires arbitration.

**Chamba scenario**:
```
Agent: "Professional product photos for e-commerce" - $25
Worker: Takes photos, uploads, agent auto-approves
        CHARGE: Worker receives $25

3 days later:
Agent: Discovers photos are blurry, unusable for e-commerce

Dispute Process:
1. Agent initiates dispute via RefundRequest
2. Evidence submitted:
   - Agent: Original requirements, photo quality analysis
   - Worker: Equipment used, lighting conditions, original files
3. Arbitration panel (3 people) reviews
4. Ruling: 2-of-3 consensus

Outcomes:
  A) Agent wins (photos don't meet requirements):
     - Worker must return $25 (or future earnings garnished)
     - Agent recovers $25
     - Worker reputation: -1

  B) Worker wins (photos meet stated requirements):
     - Worker keeps $25
     - Agent loses $3.75 bond (goes to worker)
     - Worker receives total: $28.75
     - Agent reputation: -1 (unfair dispute)

  C) Partial fault (requirements were ambiguous):
     - Worker returns $12.50
     - Agent recovers $12.50
     - Worker keeps $12.50 + gets $1.875 from bond
     - No reputation penalty
```

---

## Time Configuration by Task Type

| Task Type | Accept Window | Complete Window | Dispute Window | Example |
|-----------|---------------|-----------------|----------------|---------|
| **Micro** ($0.50-$5) | 1 hour | 2 hours | 24 hours | "Verify store is open" |
| **Standard** ($5-$50) | 2 hours | 24 hours | 7 days | "Take photos of location" |
| **Premium** ($50-$200) | 4 hours | 48 hours | 14 days | "Notarize document" |
| **Enterprise** ($200+) | 24 hours | 7 days | 30 days | "Complete inspection report" |

**PaymentInfo mapping**:
```python
# Standard task ($5-$50)
payment_info = {
    "preApprovalExpiry": now + 7200,      # 2 hours to accept
    "authorizationExpiry": now + 86400,   # 24 hours to complete
    "refundExpiry": now + 604800,         # 7 days dispute window
}
```

---

## Fee Configuration

```python
payment_info = {
    "minFeeBps": 0,      # Minimum: 0% (can be free)
    "maxFeeBps": 800,    # Maximum: 8% (Chamba's take)
    "feeReceiver": PAYMENT_OPERATOR,  # MUST be PaymentOperator address
}

# Chamba fee tiers:
# - Micro tasks: Flat $0.25 (set maxFeeBps to cover this)
# - Standard: 8%
# - Premium: 6%
# - Enterprise: 4% (negotiated)
```

---

## Contract Addresses (Base Mainnet)

| Contract | Address | Purpose |
|----------|---------|---------|
| PaymentOperator | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` | Our permissionless operator |
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` | Escrow state machine |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` | Receives ERC-3009 transfers |
| TokenStore | `0x29BfE2143379Ca2E93721E42901610297f0AB463` | Holds escrowed funds |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` | Dispute resolution |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | Payment token |

---

## Running the Tests

```bash
# Run all 5 lifecycle tests
cd tests/escrow
python run_all_tests.py

# Run individual tests
python test_escrow_with_correct_nonce.py  # 1. AUTHORIZE
python test_2_charge.py                    # 2. CHARGE
python test_3_release.py                   # 3. RELEASE
python test_4_refund_in_escrow.py          # 4. REFUND IN ESCROW
python test_5_refund_post_escrow.py        # 5. REFUND POST ESCROW
```

**Requirements**:
- Python 3.10+
- `pip install web3 eth-account eth-abi boto3 requests`
- AWS credentials for test wallet (Secrets Manager)
- ~0.05 USDC + ~0.01 ETH for gas

---

## Integration with Chamba MCP Server

```python
# In mcp_server/tools/publish_task.py

async def create_escrow_payment(task: Task, agent_wallet: str):
    """Create escrow payment for Chamba task."""

    # Calculate amounts
    bounty = task.bounty_usdc
    agent_bond = bounty * 0.15  # 15% bond
    platform_fee = calculate_fee(bounty)
    total = bounty + agent_bond + platform_fee

    # Build PaymentInfo
    payment_info = {
        "operator": CHAMBA_PAYMENT_OPERATOR,
        "receiver": "0x0",  # Filled when worker accepts
        "token": USDC_BASE,
        "maxAmount": int(total * 1e6),
        "preApprovalExpiry": get_accept_window(task.tier),
        "authorizationExpiry": get_complete_window(task.tier),
        "refundExpiry": get_dispute_window(task.tier),
        "minFeeBps": 0,
        "maxFeeBps": get_max_fee_bps(task.tier),
        "feeReceiver": CHAMBA_PAYMENT_OPERATOR,
        "salt": generate_salt(task.id),
    }

    # Sign ERC-3009 authorization
    nonce = compute_escrow_nonce(CHAIN_ID_BASE, ESCROW_ADDRESS, payment_info)
    signature = sign_receive_authorization(
        agent_wallet,
        TOKEN_COLLECTOR,
        total,
        payment_info["preApprovalExpiry"],
        nonce
    )

    return {
        "payment_info": payment_info,
        "signature": signature,
        "nonce": nonce,
    }
```

---

## Key Learnings

1. **feeReceiver MUST be PaymentOperator address** - Not the platform wallet
2. **Nonce computation includes PAYMENT_INFO_TYPEHASH** - SDK bug was missing this
3. **ERC-3009 type is ReceiveWithAuthorization** - Not TransferWithAuthorization
4. **Timing is in Unix SECONDS** - Not milliseconds
5. **RefundPostEscrow requires RefundRequest approval** - Can't unilaterally refund after charge
