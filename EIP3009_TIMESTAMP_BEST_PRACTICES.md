# EIP-3009 Timestamp Best Practices

**Critical Infrastructure Guide for x402-rs Facilitator**

This document explains the `validAfter`/`validBefore` mechanism in EIP-3009, documents common pitfalls, and provides best practices for implementing payment authorization flows.

---

## Table of Contents

1. [Understanding EIP-3009 Timestamps](#understanding-eip-3009-timestamps)
2. [The Authorization Lifecycle](#the-authorization-lifecycle)
3. [Common Pitfalls](#common-pitfalls)
4. [Best Practices](#best-practices)
5. [Code Examples](#code-examples)
6. [Debugging Guide](#debugging-guide)
7. [Facilitator Implementation](#facilitator-implementation)
8. [References](#references)

---

## Understanding EIP-3009 Timestamps

### What is EIP-3009?

EIP-3009 (Transfer With Authorization) is an extension to ERC-20 that enables **gasless token transfers**. Instead of requiring the sender to have ETH/AVAX for gas, they sign an authorization off-chain, and a third party (the facilitator) executes the transfer on-chain.

### The Core Function

```solidity
function transferWithAuthorization(
    address from,           // Token sender (signs the authorization)
    address to,             // Token recipient
    uint256 value,          // Amount to transfer
    uint256 validAfter,     // Timestamp AFTER which authorization is valid (inclusive)
    uint256 validBefore,    // Timestamp BEFORE which authorization must be used (exclusive)
    bytes32 nonce,          // Unique 32-byte identifier (prevents replay)
    uint8 v, bytes32 r, bytes32 s  // EIP-712 signature components
) external;
```

### Timestamp Fields Explained

#### `validAfter` (uint256)

- **Type**: Unix timestamp (seconds since epoch)
- **Semantics**: Authorization becomes valid AFTER this timestamp (inclusive: `block.timestamp >= validAfter`)
- **Purpose**: Prevents authorization from being used too early
- **Typical value**: `now - 60` (1 minute ago, for clock skew tolerance)

#### `validBefore` (uint256)

- **Type**: Unix timestamp (seconds since epoch)
- **Semantics**: Authorization must be used BEFORE this timestamp (exclusive: `block.timestamp < validBefore`)
- **Purpose**: Sets expiration time for authorization
- **Typical value**: `now + 3600` (1 hour from now)

### Why Two Timestamps?

The dual-timestamp design provides:

1. **Clock skew tolerance**: `validAfter = now - 60` allows for minor time differences between client and blockchain
2. **Expiration control**: `validBefore` prevents old authorizations from being replayed indefinitely
3. **Scheduled payments**: Can create authorizations that become valid at a future time

---

## The Authorization Lifecycle

### Phase 1: Creation (Off-Chain)

**Client generates authorization**:

```python
import time
import secrets

now = int(time.time())
valid_after = now - 60      # Valid from 1 minute ago
valid_before = now + 3600   # Expires in 1 hour
nonce = "0x" + secrets.token_hex(32)  # Random 32-byte nonce

# Sign EIP-712 message with these parameters
authorization = sign_eip712(from, to, value, valid_after, valid_before, nonce)
```

**Key point**: Each authorization MUST have:
- **Unique nonce**: Random UUID, not sequential
- **Fresh timestamps**: Generated at signing time

### Phase 2: Transmission (HTTP)

**Client sends to seller**:

```http
POST /api/service HTTP/1.1
Host: seller.example.com
X-Payment: <base64-encoded-payment-payload>
```

### Phase 3: Verification (Facilitator)

**Facilitator checks**:

1. **Signature validity**: Recover signer from EIP-712 signature
2. **Temporal validity**: `validAfter <= block.timestamp < validBefore`
3. **Nonce uniqueness**: Check nonce not already used on-chain
4. **Sufficient balance**: Sender has enough tokens

```rust
// x402-rs/src/chain/evm.rs line 738-757
fn assert_time(
    payer: MixedAddress,
    valid_after: UnixTimestamp,
    valid_before: UnixTimestamp,
) -> Result<(), FacilitatorLocalError> {
    let now = UnixTimestamp::try_now()?;

    // Add 6-second grace buffer for latency
    if valid_before < now + 6 {
        return Err(FacilitatorLocalError::InvalidTiming(
            payer,
            format!("Expired: now {} > valid_before {}", now + 6, valid_before),
        ));
    }

    if valid_after > now {
        return Err(FacilitatorLocalError::InvalidTiming(
            payer,
            format!("Not active yet: valid_after {valid_after} > now {now}"),
        ));
    }

    Ok(())
}
```

**6-second grace buffer**: The facilitator adds 6 seconds to `now` when checking expiration to account for:
- Network latency (1-2 seconds)
- Blockchain block time variations (2-12 seconds depending on chain)
- Client-server clock differences

### Phase 4: Settlement (On-Chain)

**Smart contract validates**:

```solidity
// USDC contract checks
require(block.timestamp >= validAfter, "FiatTokenV2: authorization is not yet valid");
require(block.timestamp < validBefore, "FiatTokenV2: authorization is expired");
require(!authorizationStates[from][nonce], "FiatTokenV2: authorization is used");

// Mark nonce as used (prevents replay)
authorizationStates[from][nonce] = true;

// Execute transfer
_transfer(from, to, value);
```

---

## Common Pitfalls

### Pitfall #1: Reusing Authorizations

**WRONG PATTERN**:

```python
# Generate authorization ONCE
authorization = sign_transfer_authorization(from, to, value, private_key)

# Try to use it MULTIPLE times
for i in range(5):
    response = requests.post(seller_url, json=authorization)
    # First request: SUCCESS
    # Subsequent requests: "FiatTokenV2: authorization is used" or "authorization is expired"
```

**Why it fails**:

1. **Nonce reuse**: Once the first transaction settles, the nonce is marked as used on-chain
2. **Timestamp expiration**: If you wait too long, `block.timestamp >= validBefore`

**What actually happened in our test**:

```
Transaction 1: validBefore=1761942163, settled at block.timestamp=1761941550 → SUCCESS
Transaction 2: validBefore=1761942163, attempted at block.timestamp=1761942170 → EXPIRED
                (same authorization reused, but now 1761942170 >= 1761942163)
```

### Pitfall #2: Short Time Windows

**WRONG PATTERN**:

```python
valid_after = int(time.time())
valid_before = int(time.time()) + 660  # Only 11 minutes!
```

**Why it's risky**:

- Network delays: 10-30 seconds
- Blockchain confirmation: 2-12 seconds per block
- Retries: If first attempt fails, window may have expired
- 11 minutes sounds long, but with async systems it's not

**Example failure timeline**:

```
00:00 - Authorization created (validBefore = 00:11)
00:05 - First transaction sent
00:06 - Transaction mined, nonce used
00:07 - Client retries (doesn't know first succeeded)
00:07 - FAIL: "authorization is used"
00:12 - Third attempt
00:12 - FAIL: "authorization is expired" (now > validBefore)
```

### Pitfall #3: Clock Skew Ignorance

**WRONG PATTERN**:

```python
valid_after = int(time.time())  # Exact current time
```

**Why it fails**:

- Client clock: `1234567890`
- Blockchain node clock: `1234567885` (5 seconds behind)
- Result: `block.timestamp < validAfter` → "authorization is not yet valid"

**Correct pattern**:

```python
valid_after = int(time.time()) - 60  # Allow 60-second clock skew
```

### Pitfall #4: Timestamp as Strings vs Integers

**WRONG PATTERN**:

```python
# Python client sends timestamps as integers
authorization = {
    "validAfter": 1234567890,      # int
    "validBefore": 1234571490      # int
}

# JSON serialization converts to string incorrectly
json.dumps(authorization)  # May lose precision or format wrong
```

**Correct pattern** (x402-rs expects stringified integers):

```python
authorization = {
    "validAfter": str(1234567890),    # "1234567890"
    "validBefore": str(1234571490)    # "1234571490"
}
```

See `x402-rs/src/timestamp.rs`:

```rust
impl Serialize for UnixTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())  // Stringified u64
    }
}
```

---

## Best Practices

### ✅ Best Practice #1: Generate Fresh Authorization Per Transaction

**CORRECT PATTERN**:

```python
def make_payment_request(seller_url: str, seller_address: str, amount: int):
    """Each call generates NEW authorization with fresh nonce and timestamps"""

    now = int(time.time())

    # Fresh timestamps for THIS transaction
    valid_after = now - 60      # 1 minute tolerance
    valid_before = now + 3600   # 1 hour expiry

    # Fresh nonce for THIS transaction
    nonce = "0x" + secrets.token_hex(32)

    # Sign with fresh data
    authorization = sign_transfer_authorization(
        from_address=buyer_address,
        to_address=seller_address,
        value=amount,
        valid_after=valid_after,
        valid_before=valid_before,
        nonce=nonce,
        private_key=private_key
    )

    # Use authorization immediately
    response = requests.post(seller_url, json=authorization)
    return response
```

**Why this works**:

- Each authorization has unique nonce → no "authorization is used" errors
- Each authorization has fresh timestamps → no "authorization is expired" errors
- Aligns with EIP-3009 spec: authorizations are single-use

### ✅ Best Practice #2: Use 1-Hour Expiry Window

**RECOMMENDED**:

```python
valid_after = int(time.time()) - 60      # 1 minute ago
valid_before = int(time.time()) + 3600   # 1 hour from now
```

**Rationale**:

- **60-second clock skew tolerance**: Handles client/server time differences
- **3600-second (1 hour) expiry**: Long enough for retries, short enough to limit replay attack window
- Matches production patterns in `shared/payment_signer.py` and `test-seller/load_test.py`

### ✅ Best Practice #3: Handle Facilitator 6-Second Grace Buffer

**The facilitator adds 6 seconds to current time when checking expiration** (see `x402-rs/src/chain/evm.rs:744`):

```rust
if valid_before < now + 6 {
    return Err(/* ... */);
}
```

**Implication**: Your authorization should have `validBefore >= now + 6` to pass verification.

**Safe pattern**:

```python
valid_before = int(time.time()) + 3600  # 1 hour is >> 6 seconds, safe
```

**Dangerous pattern**:

```python
valid_before = int(time.time()) + 5  # Only 5 seconds!
# Facilitator checks: now + 6 > validBefore → FAIL
```

### ✅ Best Practice #4: Use Random Nonces (Not Sequential)

**CORRECT**:

```python
import secrets
nonce = "0x" + secrets.token_hex(32)  # Cryptographically random 32 bytes
```

**WRONG**:

```python
nonce_counter = 0
nonce = Web3.keccak(text=str(nonce_counter))  # Sequential, predictable
nonce_counter += 1
```

**Why random is better**:

- No need to track nonce state (stateless clients)
- No collision risk across multiple clients
- Collision probability: `1 / 2^256` (astronomically low)
- EIP-3009 spec recommends UUIDs or random bytes

### ✅ Best Practice #5: Retry Logic with New Authorizations

**CORRECT**:

```python
def buy_with_retry(seller_url, seller_address, amount, max_retries=3):
    """Retry with FRESH authorization each time"""

    for attempt in range(max_retries):
        try:
            # Generate NEW authorization for THIS attempt
            authorization = create_fresh_authorization(seller_address, amount)

            response = requests.post(seller_url, json=authorization, timeout=30)

            if response.status_code == 200:
                return response  # Success

        except Exception as e:
            if attempt < max_retries - 1:
                time.sleep(2 ** attempt)  # Exponential backoff
                continue
            else:
                raise

    raise Exception(f"All {max_retries} attempts failed")
```

**WRONG**:

```python
# Generate authorization ONCE
authorization = create_fresh_authorization(seller_address, amount)

# Retry with SAME authorization
for attempt in range(3):
    response = requests.post(seller_url, json=authorization)
    # If first attempt succeeded, subsequent ones will fail with "authorization is used"
```

---

## Code Examples

### Example 1: Correct Payment Flow (Python)

```python
#!/usr/bin/env python3
"""
Correct EIP-3009 payment implementation
Each transaction gets fresh authorization
"""
import time
import secrets
from web3 import Web3
from eth_account import Account
from eth_account.messages import encode_typed_data

def sign_transfer_authorization(
    from_address: str,
    to_address: str,
    value: int,
    private_key: str,
    token_address: str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",  # USDC Base
    chain_id: int = 8453
) -> dict:
    """
    Create fresh EIP-3009 authorization

    Returns:
        dict with signature and authorization fields
    """
    # STEP 1: Generate fresh temporal parameters
    now = int(time.time())
    valid_after = now - 60      # 1 minute tolerance
    valid_before = now + 3600   # 1 hour expiry

    # STEP 2: Generate fresh nonce
    nonce = "0x" + secrets.token_hex(32)

    # STEP 3: Build EIP-712 domain
    domain = {
        "name": "USD Coin",
        "version": "2",
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(token_address)
    }

    # STEP 4: Build message
    message = {
        "from": Web3.to_checksum_address(from_address),
        "to": Web3.to_checksum_address(to_address),
        "value": value,
        "validAfter": valid_after,
        "validBefore": valid_before,
        "nonce": nonce
    }

    # STEP 5: Build typed data
    typed_data = {
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"}
            ],
            "TransferWithAuthorization": [
                {"name": "from", "type": "address"},
                {"name": "to", "type": "address"},
                {"name": "value", "type": "uint256"},
                {"name": "validAfter", "type": "uint256"},
                {"name": "validBefore", "type": "uint256"},
                {"name": "nonce", "type": "bytes32"}
            ]
        },
        "primaryType": "TransferWithAuthorization",
        "domain": domain,
        "message": message
    }

    # STEP 6: Sign
    encoded = encode_typed_data(full_message=typed_data)
    account = Account.from_key(private_key)
    signed = account.sign_message(encoded)

    # STEP 7: Return authorization (timestamps as STRINGS for x402-rs)
    return {
        "signature": signed.signature.hex(),
        "authorization": {
            "from": from_address,
            "to": to_address,
            "value": str(value),           # String!
            "validAfter": str(valid_after),  # String!
            "validBefore": str(valid_before), # String!
            "nonce": nonce
        }
    }


def make_multiple_payments(seller_url: str, seller_address: str, num_payments: int):
    """
    Correct pattern: Generate fresh authorization per payment
    """
    from_address = "0xYourBuyerAddress"
    private_key = "0xYourPrivateKey"
    amount_usdc = 10000  # $0.01 USDC (6 decimals)

    for i in range(num_payments):
        print(f"\n[Payment {i+1}/{num_payments}]")

        # Generate FRESH authorization for THIS payment
        auth = sign_transfer_authorization(
            from_address=from_address,
            to_address=seller_address,
            value=amount_usdc,
            private_key=private_key
        )

        print(f"  Nonce: {auth['authorization']['nonce'][:18]}...")
        print(f"  Valid: {auth['authorization']['validAfter']} to {auth['authorization']['validBefore']}")

        # Create x402 payment payload
        payment = {
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": "base",
                "payload": {
                    "signature": auth["signature"],
                    "authorization": auth["authorization"]
                }
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "base",
                "maxAmountRequired": str(amount_usdc),
                "resource": seller_url,
                "description": "Service payment",
                "mimeType": "application/json",
                "payTo": seller_address,
                "maxTimeoutSeconds": 300,
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "extra": {
                    "name": "USD Coin",
                    "version": "2"
                }
            }
        }

        # Send to seller
        response = requests.post(seller_url, json=payment, timeout=30)

        if response.status_code == 200:
            print(f"  ✅ SUCCESS")
        else:
            print(f"  ❌ FAILED: {response.status_code} - {response.text[:100]}")

        # Small delay between requests
        time.sleep(2)


if __name__ == "__main__":
    make_multiple_payments(
        seller_url="https://test-seller.example.com/hello",
        seller_address="0xSellerAddress",
        num_payments=5
    )
```

### Example 2: Load Testing Pattern

See `test-seller/load_test.py` (lines 123-178) for production implementation:

```python
class TestSellerLoadTest:
    def sign_transfer_authorization(self) -> tuple[str, Dict[str, Any]]:
        """Sign EIP-712 TransferWithAuthorization"""

        # FRESH nonce per call
        nonce = "0x" + os.urandom(32).hex()

        # FRESH timestamps per call
        valid_after = int(time.time()) - 60   # 1 minute ago
        valid_before = int(time.time()) + 600  # 10 minutes from now

        # ... sign and return ...

    def make_paid_request(self, request_id: int) -> bool:
        """Make a single paid request"""

        # Generate FRESH authorization for THIS request
        signature, authorization = self.sign_transfer_authorization()

        # Use authorization immediately
        response = requests.post(seller_url, json=payload)
        return response.status_code == 200
```

---

## Debugging Guide

### Symptom 1: "FiatTokenV2: authorization is expired"

**Error message** (from blockchain or facilitator):

```
FacilitatorLocalError::InvalidTiming(
    payer,
    "Expired: now 1761942170 > valid_before 1761942163"
)
```

**Root causes**:

1. **Authorization too old**: Signed minutes/hours ago, `block.timestamp >= validBefore`
2. **Short time window**: `validBefore - validAfter < 600` (less than 10 minutes)
3. **Reused authorization**: First use was slow, second use exceeded window

**How to diagnose**:

```python
# Add debug logging
import time

print(f"[DEBUG] Current time: {int(time.time())}")
print(f"[DEBUG] validAfter:   {authorization['validAfter']}")
print(f"[DEBUG] validBefore:  {authorization['validBefore']}")
print(f"[DEBUG] Window size:  {int(authorization['validBefore']) - int(authorization['validAfter'])} seconds")
print(f"[DEBUG] Time until expiry: {int(authorization['validBefore']) - int(time.time())} seconds")
```

**Expected output** (healthy):

```
[DEBUG] Current time: 1761941500
[DEBUG] validAfter:   1761941440  (60 seconds ago)
[DEBUG] validBefore:  1761945100  (3600 seconds from now)
[DEBUG] Window size:  3660 seconds (1 hour + 1 minute tolerance)
[DEBUG] Time until expiry: 3600 seconds
```

**Problematic output**:

```
[DEBUG] Current time: 1761942170
[DEBUG] validAfter:   1761941503
[DEBUG] validBefore:  1761942163  (7 seconds ago!)
[DEBUG] Window size:  660 seconds (only 11 minutes)
[DEBUG] Time until expiry: -7 seconds (EXPIRED!)
```

**Solution**:

```python
# Change this:
valid_before = int(time.time()) + 600   # 10 minutes (too short)

# To this:
valid_before = int(time.time()) + 3600  # 1 hour (recommended)
```

### Symptom 2: "FiatTokenV2: authorization is used"

**Error message**:

```
Error: Nonce already used on-chain
```

**Root cause**:

- Same authorization (same nonce) submitted multiple times
- First submission succeeded, marked nonce as used
- Subsequent submissions fail

**How to diagnose**:

```python
# Log nonce per request
print(f"[Request {i}] Nonce: {authorization['nonce']}")

# If you see SAME nonce repeated:
[Request 1] Nonce: 0x1234abcd...
[Request 2] Nonce: 0x1234abcd...  ← PROBLEM! Should be different
```

**Solution**:

Move authorization generation INSIDE the request loop:

```python
# WRONG
authorization = sign_transfer_authorization(...)  # Once
for i in range(5):
    requests.post(url, json=authorization)  # Reused

# CORRECT
for i in range(5):
    authorization = sign_transfer_authorization(...)  # Fresh each time
    requests.post(url, json=authorization)
```

### Symptom 3: "FiatTokenV2: authorization is not yet valid"

**Error message**:

```
FacilitatorLocalError::InvalidTiming(
    payer,
    "Not active yet: valid_after 1234567900 > now 1234567890"
)
```

**Root cause**:

- `validAfter` is in the future
- Usually caused by clock skew (client ahead of blockchain node)

**How to diagnose**:

```python
import time

now = int(time.time())
print(f"[DEBUG] Client time:     {now}")
print(f"[DEBUG] validAfter:      {authorization['validAfter']}")
print(f"[DEBUG] Difference:      {int(authorization['validAfter']) - now} seconds")
```

**If difference is positive**: Client clock is behind blockchain

**Solution**:

```python
# Change this:
valid_after = int(time.time())  # Exact time (risky)

# To this:
valid_after = int(time.time()) - 60  # 1 minute tolerance
```

### Symptom 4: Facilitator Rejects Valid Authorization

**Error from facilitator** `/verify` or `/settle`:

```json
{
  "valid": false,
  "reason": "InvalidTiming"
}
```

**But timestamps look correct?**

Check the **6-second grace buffer**:

```python
now = int(time.time())
valid_before = now + 10  # Only 10 seconds

# Facilitator checks: now + 6 < valid_before
# 1234567890 + 6 = 1234567896
# 1234567896 < 1234567900 → OK

# But if network delay is 5 seconds:
# now becomes 1234567895
# 1234567895 + 6 = 1234567901
# 1234567901 < 1234567900 → FAIL!
```

**Solution**: Use at least 1-hour expiry (`+ 3600`)

### Debugging Checklist

When payment fails, check:

- [ ] Is `validBefore` at least 1 hour in the future?
- [ ] Is `validAfter` at least 60 seconds in the past?
- [ ] Is the nonce unique (not reused)?
- [ ] Are timestamps stringified integers (`"1234567890"` not `1234567890`)?
- [ ] Is the signature fresh (not from a previous transaction)?
- [ ] Check facilitator logs: `aws logs tail /ecs/karmacadabra-prod-facilitator`
- [ ] Check blockchain explorer (BaseScan/Snowtrace) for transaction revert reasons

---

## Facilitator Implementation

### Timestamp Validation Code

**Location**: `x402-rs/src/chain/evm.rs` lines 738-757

```rust
/// Validates that the current time is within the `validAfter` and `validBefore` bounds.
///
/// Adds a 6-second grace buffer when checking expiration to account for latency.
#[instrument(skip_all, err)]
fn assert_time(
    payer: MixedAddress,
    valid_after: UnixTimestamp,
    valid_before: UnixTimestamp,
) -> Result<(), FacilitatorLocalError> {
    let now = UnixTimestamp::try_now()
        .map_err(FacilitatorLocalError::ClockError)?;

    // Check expiration with 6-second grace buffer
    if valid_before < now + 6 {
        return Err(FacilitatorLocalError::InvalidTiming(
            payer,
            format!("Expired: now {} > valid_before {}", now + 6, valid_before),
        ));
    }

    // Check not yet valid
    if valid_after > now {
        return Err(FacilitatorLocalError::InvalidTiming(
            payer,
            format!("Not active yet: valid_after {valid_after} > now {now}"),
        ));
    }

    Ok(())
}
```

**Key implementation details**:

1. **Grace buffer**: `now + 6` accounts for network/blockchain latency
2. **Inclusive lower bound**: `valid_after <= now` (authorization valid from this timestamp)
3. **Exclusive upper bound**: `now + 6 < valid_before` (authorization invalid at/after this timestamp)
4. **System clock source**: `UnixTimestamp::try_now()` uses `SystemTime::now().duration_since(UNIX_EPOCH)`

### Timestamp Serialization

**Location**: `x402-rs/src/timestamp.rs` lines 18-35

```rust
/// A Unix timestamp represented as a `u64`, used in payment authorization windows.
///
/// Serialized as a stringified integer to avoid loss of precision in JSON.
/// For example, `1699999999` becomes `"1699999999"` in the wire format.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Ord, Eq)]
pub struct UnixTimestamp(pub u64);

impl Serialize for UnixTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())  // "1234567890"
    }
}

impl<'de> Deserialize<'de> for UnixTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let ts = s
            .parse::<u64>()
            .map_err(|_| serde::de::Error::custom("timestamp must be a non-negative integer"))?;
        Ok(UnixTimestamp(ts))
    }
}
```

**Implication**: Python clients must send timestamps as **strings**, not integers:

```python
# CORRECT
authorization = {
    "validAfter": "1234567890",   # String
    "validBefore": "1234571490"   # String
}

# WRONG (will fail deserialization)
authorization = {
    "validAfter": 1234567890,     # Integer (not accepted by x402-rs)
    "validBefore": 1234571490
}
```

### Integration with Payment Verification

**Location**: `x402-rs/src/chain/evm.rs` lines 223-279

```rust
async fn assert_valid_payment(
    &self,
    payload: &PaymentPayload,
    requirements: &PaymentRequirements,
) -> Result<(USDC::USDCInstance<&InnerProvider>, ExactEvmPayment, Eip712Domain), FacilitatorLocalError> {
    // ... network/scheme/receiver validation ...

    let valid_after = payment_payload.authorization.valid_after;
    let valid_before = payment_payload.authorization.valid_before;

    // Validate timestamps
    assert_time(payer.into(), valid_after, valid_before)?;

    // ... balance/signature validation ...
}
```

**Flow**:

1. Extract `validAfter` and `validBefore` from payment payload
2. Call `assert_time()` to validate temporal bounds
3. If validation fails, return `FacilitatorLocalError::InvalidTiming`
4. If valid, proceed to signature and balance checks

---

## References

### Official Specifications

- **EIP-3009**: [https://eips.ethereum.org/EIPS/eip-3009](https://eips.ethereum.org/EIPS/eip-3009)
- **EIP-712**: [https://eips.ethereum.org/EIPS/eip-712](https://eips.ethereum.org/EIPS/eip-712) (Typed structured data signing)
- **USDC Implementation**: [Centre Consortium USDC contracts](https://github.com/centrehq/centre-tokens)

### Karmacadabra Codebase

**Facilitator (Rust)**:
- `x402-rs/src/chain/evm.rs` - EIP-3009 verification and settlement
- `x402-rs/src/timestamp.rs` - Timestamp type definitions
- `x402-rs/src/handlers.rs` - HTTP handlers for `/verify` and `/settle`

**Python Clients**:
- `shared/payment_signer.py` - EIP-712 signing utilities
- `shared/x402_client.py` - HTTP client for x402 protocol
- `test-seller/load_test.py` - Production load testing implementation
- `scripts/test_base_usdc_stress.py` - Mainnet stress testing

### Related Documentation

- `docs/ARCHITECTURE.md` - System architecture overview
- `x402-rs/README.md` - Facilitator setup and deployment
- `CLAUDE.md` - Development guidelines (includes EIP-3009 best practices)

---

## Quick Reference Card

### Recommended Timestamp Pattern

```python
import time
import secrets

# For EACH transaction:
now = int(time.time())
valid_after = now - 60      # 1 minute tolerance
valid_before = now + 3600   # 1 hour expiry
nonce = "0x" + secrets.token_hex(32)  # Fresh random nonce

authorization = {
    "from": buyer_address,
    "to": seller_address,
    "value": str(amount),              # String!
    "validAfter": str(valid_after),    # String!
    "validBefore": str(valid_before),  # String!
    "nonce": nonce
}

# Sign with EIP-712, send immediately
```

### Validation Checklist

Before submitting payment authorization:

- [ ] `validAfter = now - 60` (1 minute tolerance)
- [ ] `validBefore = now + 3600` (1 hour expiry)
- [ ] `nonce` is random 32 bytes (not sequential)
- [ ] All timestamps are stringified (`str(timestamp)`)
- [ ] Authorization is used immediately (not stored for later)
- [ ] Each transaction gets fresh authorization (not reused)

### Error Code Quick Reference

| Error | Cause | Solution |
|-------|-------|----------|
| `authorization is expired` | `block.timestamp >= validBefore` | Generate fresh authorization with `+ 3600` |
| `authorization is not yet valid` | `block.timestamp < validAfter` | Use `now - 60` instead of `now` |
| `authorization is used` | Nonce already used on-chain | Generate fresh nonce per transaction |
| `InvalidTiming` (facilitator) | Outside `validAfter`/`validBefore` window | Check timestamps, use 1-hour window |

---

**Document Version**: 1.0
**Last Updated**: 2025-10-31
**Author**: Future-Architect (Claude)
**Related Incident**: Base USDC deployment timestamp debugging (Oct 2025)
