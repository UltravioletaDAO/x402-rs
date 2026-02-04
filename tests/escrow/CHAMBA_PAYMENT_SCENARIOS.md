# Chamba x Advanced Escrow: Complete Payment Scenarios

## How This Works

The PaymentOperator Advanced Escrow provides 5 on-chain primitives that map to different Chamba payment scenarios. Each scenario tells the story of an AI agent using Chamba to hire a human, and what happens with the payment.

---

## The 5 Payment Primitives

| # | Primitive | Contract Call | What Happens |
|---|-----------|--------------|--------------|
| 1 | **AUTHORIZE** | `operator.authorize() -> escrow.authorize()` | Lock funds in escrow |
| 2 | **RELEASE** | `operator.release() -> escrow.capture()` | Release escrowed funds to worker |
| 3 | **REFUND IN ESCROW** | `operator.refundInEscrow() -> escrow.partialVoid()` | Return escrowed funds to agent |
| 4 | **CHARGE** | `operator.charge() -> escrow.charge()` | Direct instant payment (no escrow) |
| 5 | **REFUND POST ESCROW** | `operator.refundPostEscrow() -> escrow.refund()` | Dispute refund after release |

---

## Scenario 1: AUTHORIZE + RELEASE (Happy Path - Task Completed)

### Story: "Verify if Cafe Velvet is Open"

**Agent**: TravelBot (AI travel assistant planning a Medellin trip for its user)
**Worker**: Maria, a local in El Poblado
**Bounty**: $5 USDC

```
Timeline:
  09:00 AM - Agent publishes task with AUTHORIZE
             -> $5 USDC locked in escrow
             -> preApprovalExpiry: 2 hours (worker must accept by 11 AM)
             -> authorizationExpiry: 6 hours (must complete by 3 PM)
             -> refundExpiry: 24 hours (dispute window)

  09:15 AM - Maria sees task on Chamba dashboard, accepts
  09:45 AM - Maria walks to Cafe Velvet, takes photo with GPS stamp
  09:50 AM - Maria uploads: photo + GPS + text "Open, serving espresso"
  09:51 AM - Auto-verification:
             [OK] GPS matches Cafe Velvet location
             [OK] Photo timestamp is fresh (< 30 min)
             [OK] AI Vision confirms cafe is open
  09:52 AM - Agent approves -> RELEASE executed
             -> $4.60 USDC to Maria (after 8% fee)
             -> $0.40 USDC platform fee
```

**Payment Flow**:
```
Agent wallet --[AUTHORIZE]--> TokenStore (escrow)
                                    |
TokenStore --[RELEASE/capture]--> Maria's wallet ($4.60)
                              --> PaymentOperator ($0.40 fee)
```

**PaymentInfo Configuration**:
```python
payment_info = {
    "operator": PAYMENT_OPERATOR,
    "receiver": maria_wallet,
    "token": USDC_BASE,
    "maxAmount": 5_000_000,          # $5.00
    "preApprovalExpiry": now + 7200,  # 2 hours to accept
    "authorizationExpiry": now + 21600,  # 6 hours to complete
    "refundExpiry": now + 86400,      # 24h dispute window
    "minFeeBps": 0,
    "maxFeeBps": 800,                 # 8% max fee
    "feeReceiver": PAYMENT_OPERATOR,
    "salt": random_salt,
}
```

---

## Scenario 2: AUTHORIZE + REFUND IN ESCROW (Task Cancelled by Agent)

### Story: "Photograph Concert at Parque Lleras"

**Agent**: EventBot (AI event tracker)
**Worker**: Carlos, a photographer
**Bounty**: $20 USDC

```
Timeline:
  Monday 2:00 PM - Agent publishes task for Friday night concert
                    -> $20 USDC locked in escrow
                    -> preApprovalExpiry: 48 hours
                    -> authorizationExpiry: 120 hours (5 days)

  Monday 4:00 PM - Carlos accepts the task

  Wednesday 10:00 AM - Concert promoter announces cancellation (rain)
  Wednesday 10:05 AM - Agent cancels task -> REFUND IN ESCROW
                       -> $20 USDC returned to agent wallet
                       -> Carlos notified: "Task cancelled - event cancelled"
```

**Payment Flow**:
```
Agent wallet --[AUTHORIZE]--> TokenStore (escrow)
                                    |
TokenStore --[REFUND IN ESCROW/partialVoid]--> Agent wallet ($20 back)
```

**When to use**: Agent needs to cancel before work is done. No cost to anyone.

### Variant: No Workers Accepted

```
  09:00 AM - Agent publishes: "Scan rare book pages at library" - $8
  11:00 AM - preApprovalExpiry reached, no workers accepted
  11:01 AM - Agent calls REFUND IN ESCROW
             -> $8 USDC returned
             -> Agent re-posts with $12 bounty
```

---

## Scenario 3: CHARGE (Direct Instant Payment)

### Story: "Quick Delivery Across Town"

**Agent**: LogisticsBot (AI courier dispatcher)
**Worker**: Juan, a motorcycle courier with 95% reputation
**Bounty**: $3 USDC

```
Timeline:
  2:00 PM - Agent knows Juan is reliable (95% reputation score)
  2:01 PM - Agent uses CHARGE for instant payment
            -> $3 USDC goes directly from agent to Juan
            -> No escrow hold, no waiting period
            -> Juan sees payment immediately in his wallet

  2:05 PM - Juan picks up package
  2:30 PM - Juan delivers, uploads photo confirmation
```

**Payment Flow**:
```
Agent wallet --[CHARGE]--> Juan's wallet ($2.76)
                       --> PaymentOperator ($0.24 fee)
```

**When to use**:
- Micro-tasks under $5 where escrow overhead isn't worth it
- Trusted workers with high reputation (>90%)
- Repeat workers the agent has used before
- Time-critical tasks where any delay is costly
- Prepayment before work starts (trust-based)

**PaymentInfo for CHARGE**:
```python
# Same PaymentInfo struct but CHARGE is called instead of AUTHORIZE
# Funds go directly to receiver, no capture step needed
payment_info = {
    "operator": PAYMENT_OPERATOR,
    "receiver": juan_wallet,
    "token": USDC_BASE,
    "maxAmount": 3_000_000,
    "preApprovalExpiry": now + 3600,
    "authorizationExpiry": now + 86400,
    "refundExpiry": now + 604800,  # 7 day refund window (for disputes)
    "minFeeBps": 0,
    "maxFeeBps": 800,
    "feeReceiver": PAYMENT_OPERATOR,
    "salt": random_salt,
}
```

---

## Scenario 4: AUTHORIZE + PARTIAL RELEASE + REFUND IN ESCROW (Proof of Attempt)

### Story: "Buy Specific Wine at La Cava del Barrio"

**Agent**: PersonalShopperBot (AI concierge)
**Worker**: Ana, local shopper
**Bounty**: $30 USDC (for the service, not the wine itself)

```
Timeline:
  10:00 AM - Agent publishes task: "Buy 2019 Chateau wine at La Cava"
             -> $30 USDC locked in escrow

  10:30 AM - Ana accepts, goes to wine shop
  11:00 AM - Wine is sold out (legitimate obstacle)
  11:05 AM - Ana uploads "Proof of Attempt":
             - Photo of shelf where wine should be (empty)
             - Photo of conversation with employee confirming sold out
             - GPS confirms location is La Cava del Barrio

  11:10 AM - Agent verifies proof is legitimate
  11:11 AM - Two transactions:
             1. RELEASE $4.50 to Ana (15% for genuine attempt)
             2. REFUND IN ESCROW $25.50 back to agent

  Result:
  - Ana got $4.50 for her time and effort
  - Agent got $25.50 back to try a different store
  - Everyone is fairly compensated
```

**Payment Flow** (two-step):
```
Agent wallet --[AUTHORIZE]--> TokenStore ($30 in escrow)
                                    |
            RELEASE (partial) ----> Ana's wallet ($4.50)
            REFUND IN ESCROW -----> Agent wallet ($25.50)
```

**This requires two separate PaymentInfo transactions** or partial amounts:
```python
# Option 1: Use partial release amount in release()
operator.release(payment_info, 4_500_000)  # $4.50 to worker
operator.refundInEscrow(payment_info, 25_500_000)  # $25.50 to agent
```

---

## Scenario 5: AUTHORIZE + RELEASE + REFUND POST ESCROW (Quality Dispute)

### Story: "Professional Product Photos for E-commerce"

**Agent**: ShopifyBot (AI store manager)
**Worker**: Diego, freelance photographer
**Bounty**: $25 USDC

```
Timeline:
  Monday 9:00 AM - Agent publishes task
                    -> $25 USDC locked in escrow
                    -> authorizationExpiry: 48 hours
                    -> refundExpiry: 7 days (dispute window)

  Monday 2:00 PM - Diego accepts, sets up photo studio
  Tuesday 10:00 AM - Diego uploads 20 product photos
  Tuesday 10:05 AM - Auto-check passes (photos present, metadata OK)
  Tuesday 10:06 AM - Agent auto-approves -> RELEASE
                     -> $23.00 to Diego
                     -> $2.00 platform fee

  Wednesday 3:00 PM - Agent reviews photos in detail
                      -> Discovers: 8 photos are blurry, unusable
                      -> Agent initiates dispute: REFUND POST ESCROW

  Dispute Process:
  1. Agent submits evidence:
     - Original requirements ("crisp, e-commerce quality")
     - Quality analysis showing 8/20 are blurry
  2. Diego submits evidence:
     - Equipment specs (professional camera)
     - Lighting setup photos
     - Raw files showing it's a camera issue, not editing
  3. Arbitration panel (3 reviewers) votes:
     - 2 of 3 rule for partial refund (requirements were met for 12/20)

  Resolution:
  - Diego returns $10.00 (for 8 unusable photos)
  - Diego keeps $13.00 (for 12 good photos)
  - Agent recovers $10.00
  - Both reputations preserved (partial fault)
```

**Payment Flow**:
```
Agent wallet --[AUTHORIZE]--> TokenStore ($25)
TokenStore --[RELEASE]--> Diego's wallet ($23)

                  ... 3 days later ...

Diego's wallet --[REFUND POST ESCROW]--> Agent wallet ($10)
(via RefundRequest contract + tokenCollector)
```

---

## Scenario 6: CHARGE + REFUND POST ESCROW (Direct Payment Dispute)

### Story: "Rush Delivery Gone Wrong"

**Agent**: UrgentBot (time-critical task agent)
**Worker**: Pedro, courier
**Bounty**: $15 USDC

```
Timeline:
  3:00 PM - Agent uses CHARGE for instant payment (trust Pedro, 92% rep)
            -> $15 USDC directly to Pedro

  3:15 PM - Pedro picks up package
  4:00 PM - Pedro reports "delivered" but photo shows wrong address
  4:05 PM - Agent initiates REFUND POST ESCROW dispute

  Resolution:
  - GPS data shows Pedro went to wrong building
  - Pedro must return $15 (minus gas costs)
  - Pedro's reputation: -5 points
```

---

## Complete Chamba Payment Menu for Agents

When an AI agent publishes a task on Chamba, it can choose from these payment strategies:

### Payment Strategy 1: "Escrow with Capture" (Recommended)

```
AUTHORIZE -> [work happens] -> RELEASE
```

- **Best for**: Standard tasks ($5-$200)
- **Protection**: Full escrow protection for both parties
- **Fee timing**: Fees deducted at capture
- **Worker motivation**: Funds are visible in escrow, guaranteed payment

### Payment Strategy 2: "Escrow with Cancellation Option"

```
AUTHORIZE -> [conditions change] -> REFUND IN ESCROW
```

- **Best for**: Tasks dependent on external factors (weather, events, availability)
- **Protection**: Agent can recover funds if task becomes impossible
- **Worker impact**: Worker notified, no penalty

### Payment Strategy 3: "Direct Instant Payment"

```
CHARGE (single step)
```

- **Best for**: Micro-tasks (<$5), trusted workers, repeat engagements
- **Protection**: Minimal (refundPostEscrow available if needed)
- **Speed**: Instant payment, no escrow delay

### Payment Strategy 4: "Escrow with Partial Payment"

```
AUTHORIZE -> RELEASE (partial) -> REFUND IN ESCROW (remainder)
```

- **Best for**: Tasks with "Proof of Attempt" scenarios
- **Protection**: Worker gets compensated for effort even if task fails
- **Fairness**: Proportional payment based on actual work done

### Payment Strategy 5: "Full Lifecycle with Dispute Resolution"

```
AUTHORIZE -> RELEASE -> [dispute] -> REFUND POST ESCROW
```

- **Best for**: High-value tasks ($50+) requiring quality assurance
- **Protection**: Maximum protection with arbitration
- **Complexity**: Most complex, requires RefundRequest contract

---

## Time Configuration Guide for Agents

| Task Tier | Accept Window | Complete Window | Dispute Window | Example |
|-----------|--------------|-----------------|----------------|---------|
| **Micro** ($0.50-$5) | 1 hour | 2 hours | 24 hours | "Is the store open?" |
| **Standard** ($5-$50) | 2 hours | 24 hours | 7 days | "Take photos of location" |
| **Premium** ($50-$200) | 4 hours | 48 hours | 14 days | "Notarize document" |
| **Enterprise** ($200+) | 24 hours | 7 days | 30 days | "Complete inspection report" |

**PaymentInfo mapping**:
```python
# Micro task
{"preApprovalExpiry": now + 3600, "authorizationExpiry": now + 7200, "refundExpiry": now + 86400}

# Standard task
{"preApprovalExpiry": now + 7200, "authorizationExpiry": now + 86400, "refundExpiry": now + 604800}

# Premium task
{"preApprovalExpiry": now + 14400, "authorizationExpiry": now + 172800, "refundExpiry": now + 1209600}

# Enterprise task
{"preApprovalExpiry": now + 86400, "authorizationExpiry": now + 604800, "refundExpiry": now + 2592000}
```

---

## Fee Configuration

```python
# Platform fee: 8% for standard tasks
payment_info = {
    "minFeeBps": 0,       # Minimum: 0% (agent can choose)
    "maxFeeBps": 800,     # Maximum: 8% (Chamba's take)
    "feeReceiver": PAYMENT_OPERATOR,  # MUST be PaymentOperator address
}

# Fee tiers by category:
# - Micro tasks (<$5):     Flat $0.25 equivalent
# - Standard ($5-$50):     8%
# - Premium ($50-$200):    6%
# - Enterprise ($200+):    4% (negotiated)
```

---

## Decision Tree for Agents

```
Agent wants to publish a task on Chamba
    |
    +-- Is the worker trusted (>90% rep)?
    |       |
    |       +-- YES: Is the task under $5?
    |       |       |
    |       |       +-- YES: Use CHARGE (instant, no escrow)
    |       |       +-- NO:  Use AUTHORIZE + RELEASE (escrow for safety)
    |       |
    |       +-- NO: Use AUTHORIZE + RELEASE (always escrow for new workers)
    |
    +-- Is the task dependent on external factors?
    |       |
    |       +-- YES: Use AUTHORIZE (with plan to REFUND IN ESCROW if needed)
    |       +-- NO:  Use AUTHORIZE + RELEASE (standard flow)
    |
    +-- Is the task high-value ($50+)?
    |       |
    |       +-- YES: Use AUTHORIZE + RELEASE + full dispute window
    |       +-- NO:  Use standard flow with shorter dispute window
    |
    +-- Is this a "Proof of Attempt" scenario?
            |
            +-- YES: Use AUTHORIZE + partial RELEASE + REFUND IN ESCROW
            +-- NO:  Use standard AUTHORIZE + RELEASE
```

---

## Contract Addresses (Base Mainnet)

| Contract | Address | Purpose |
|----------|---------|---------|
| PaymentOperator | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` | Our operator |
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` | Escrow engine |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` | ERC-3009 receiver |
| TokenStore | `0x29BfE2143379Ca2E93721E42901610297f0AB463` | Holds escrowed funds |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` | Dispute resolution |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | Payment token |
