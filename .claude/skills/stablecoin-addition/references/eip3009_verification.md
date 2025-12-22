# EIP-3009 Verification Reference

This document provides detailed guidance on verifying EIP-3009 support for stablecoins.

## Understanding EIP-3009 vs EIP-2612

### EIP-3009: Transfer With Authorization

EIP-3009 defines `transferWithAuthorization` which enables **gasless transfers** in a single transaction. The token holder signs an authorization off-chain, and anyone can submit it.

```solidity
function transferWithAuthorization(
    address from,
    address to,
    uint256 value,
    uint256 validAfter,
    uint256 validBefore,
    bytes32 nonce,
    bytes signature  // or (uint8 v, bytes32 r, bytes32 s)
) external;
```

**Key properties:**
- Single-step transfer
- Payer doesn't need gas
- Includes time validity window
- Includes unique nonce to prevent replay

### EIP-2612: Permit (NOT Sufficient for x402)

EIP-2612 defines `permit` which only authorizes an **approval**, not a transfer. A second transaction is required to actually move tokens.

```solidity
function permit(
    address owner,
    address spender,
    uint256 value,
    uint256 deadline,
    uint8 v, bytes32 r, bytes32 s
) external;
```

**Why permit is insufficient:**
1. Requires TWO transactions (permit + transferFrom)
2. Spender must have gas for the second transaction
3. Not atomic - state can change between transactions
4. More complex protocol implementation

### Quick Reference Table

| Feature | EIP-3009 | EIP-2612 |
|---------|----------|----------|
| Function | `transferWithAuthorization` | `permit` |
| Transactions | 1 | 2 |
| Gas needed by payer | No | Yes (for transferFrom) |
| x402 Compatible | YES | NO |
| Common in | USDC, EURC, USDT0 | DAI, GHO, most DeFi tokens |

---

## Verification Methods

### Method 1: Call with Dummy Parameters

The most reliable verification method. Call `transferWithAuthorization` with dummy parameters and check the error message.

```bash
cast call <CONTRACT> \
  "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)" \
  0x0000000000000000000000000000000000000001 \
  0x0000000000000000000000000000000000000002 \
  1000000 \
  0 \
  9999999999 \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  0x0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 \
  --rpc-url <RPC_URL>
```

**Interpretation:**

| Error Message | EIP-3009 Support |
|---------------|------------------|
| `ECRecover: invalid signature` | YES - Function exists |
| `FiatToken: invalid signature` | YES - USDC variant |
| `SignatureChecker: invalid signature length` | YES - Different validation |
| `execution reverted` (generic) | NO - Function doesn't exist |
| `function selector was not recognized` | NO - Not implemented |

### Method 2: Check Function Selector

Query for the function selector directly:

```bash
# Get the selector for transferWithAuthorization
cast sig "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)"
# Output: 0xe3ee160e

# Try calling with minimal data to see if function exists
cast call <CONTRACT> "0xe3ee160e" --rpc-url <RPC_URL>
```

If the function doesn't exist, you'll get `function selector not found` or similar.

### Method 3: Check Contract Source Code

1. Navigate to block explorer (Etherscan, Arbiscan, etc.)
2. Go to Contract > Read Contract
3. Look for functions:
   - `transferWithAuthorization` - REQUIRED
   - `receiveWithAuthorization` - Often present with EIP-3009
   - `DOMAIN_SEPARATOR` - Required for signature verification
   - `authorizationState` or similar - Nonce tracking

### Method 4: ABI Analysis

Download and analyze the contract ABI:

```bash
# Get ABI from block explorer API
curl "https://api.arbiscan.io/api?module=contract&action=getabi&address=<CONTRACT>"

# Search for transferWithAuthorization
jq '.result | fromjson | .[] | select(.name == "transferWithAuthorization")' abi.json
```

---

## EIP-712 Domain Verification

### Required Domain Parameters

Every EIP-3009 implementation uses EIP-712 for signature verification:

```solidity
struct EIP712Domain {
    string name,
    string version,
    uint256 chainId,
    address verifyingContract
}
```

### Getting Domain Parameters

```bash
# Get token name (usually the EIP-712 name)
cast call <CONTRACT> "name()" --rpc-url <RPC> | cast --to-ascii

# Get version (if exposed)
cast call <CONTRACT> "version()" --rpc-url <RPC> | cast --to-ascii

# Get DOMAIN_SEPARATOR for verification
cast call <CONTRACT> "DOMAIN_SEPARATOR()" --rpc-url <RPC>
```

### Common Name Variations

| Token | Possible EIP-712 Names |
|-------|----------------------|
| USDC | "USD Coin", "USDC" |
| EURC | "Euro Coin", "EURC" |
| USDT0 | "USD₮0", "USDT0", "Tether USD" |
| PYUSD | "PayPal USD" |
| AUSD | "AUSD", "Agora Dollar" |

**WARNING:** The EIP-712 name must match EXACTLY what the contract uses. Case sensitivity matters. Unicode characters matter.

### Decoding DOMAIN_SEPARATOR

The DOMAIN_SEPARATOR is a hash of the typed data domain. To verify:

```python
from eth_abi import encode
from web3 import Web3

# EIP-712 domain type hash
DOMAIN_TYPEHASH = Web3.keccak(text="EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")

# Compute expected separator
name_hash = Web3.keccak(text="USD Coin")
version_hash = Web3.keccak(text="2")
chain_id = 42161  # Arbitrum
contract = "0x..."

domain = encode(
    ['bytes32', 'bytes32', 'bytes32', 'uint256', 'address'],
    [DOMAIN_TYPEHASH, name_hash, version_hash, chain_id, contract]
)
separator = Web3.keccak(domain)
```

Compare with the on-chain `DOMAIN_SEPARATOR()` value.

---

## Signature Format Variations

### Standard Compact Signature (65 bytes)

Most EIP-3009 implementations accept a single `bytes` signature:

```
signature = r (32 bytes) || s (32 bytes) || v (1 byte)
```

This is what USDC, EURC, USDT0 use.

### Split v,r,s Format

Some implementations require separate v, r, s parameters:

```solidity
function transferWithAuthorization(
    address from,
    address to,
    uint256 value,
    uint256 validAfter,
    uint256 validBefore,
    bytes32 nonce,
    uint8 v,      // Separate v
    bytes32 r,    // Separate r
    bytes32 s     // Separate s
) external;
```

**Tokens using v,r,s format:**
- PYUSD (PayPal USD)

To handle this in the facilitator, add the token to `needs_split_signature()` in `src/chain/evm.rs`.

---

## Known Token EIP-3009 Status

### Tokens WITH EIP-3009 Support

| Token | Networks | Notes |
|-------|----------|-------|
| USDC | All EVM | Circle standard |
| EURC | Ethereum, Base, Avalanche, Optimism, Polygon | Circle EUR |
| USDT0 | Arbitrum, Celo, Optimism | New LayerZero version |
| PYUSD | Ethereum | Uses v,r,s format |
| AUSD | Ethereum, Arbitrum, Avalanche, Polygon | Agora |

### Tokens WITHOUT EIP-3009 (Only EIP-2612)

| Token | Issue |
|-------|-------|
| USDT (Legacy) | Original contract, not upgradeable |
| DAI | Uses permit(), not transferWithAuthorization |
| GHO | Aave stablecoin, permit only |
| crvUSD | Curve stablecoin, permit only |
| FRAX | Permit only |
| LUSD | Permit only |
| cUSD/cCOP (Mento) | Celo stablecoins, permit only |

---

## Troubleshooting

### Error: "execution reverted"

**Cause:** The `transferWithAuthorization` function doesn't exist on this contract.

**Solution:** The token doesn't support EIP-3009. Cannot be integrated.

### Error: "invalid signature" or "ECRecover: invalid signature"

**Good sign!** This means the function exists. The error is because dummy parameters were used.

### Error: "authorization is used or canceled"

**Cause:** The nonce has been used before.

**Solution:** Use a unique nonce for each authorization.

### Error: "authorization is not yet valid" or "authorization is expired"

**Cause:** The `validAfter`/`validBefore` timestamps don't match current block time.

**Solution:** Check timestamp handling (seconds vs milliseconds).

### Signatures fail on-chain but verify locally

**Possible causes:**
1. EIP-712 name doesn't match exactly (check Unicode, case)
2. EIP-712 version is wrong
3. Chain ID mismatch
4. Wrong contract address in domain
5. Signature format mismatch (compact vs v,r,s)

---

## Verification Checklist

Before implementing a new stablecoin:

- [ ] Confirmed `transferWithAuthorization` exists (Method 1 or 2)
- [ ] Got exact EIP-712 name from contract
- [ ] Got exact EIP-712 version from contract
- [ ] Verified decimals (usually 6)
- [ ] Documented contract address for each network
- [ ] Identified signature format (compact vs v,r,s)
- [ ] Created test authorization locally
- [ ] Verified DOMAIN_SEPARATOR matches computed value
