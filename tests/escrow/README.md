# Escrow Scheme Test Suite

Test scripts for the x402r escrow scheme integration.

## Setup

```bash
cd tests/escrow
pip install -r requirements.txt
```

## Usage

### Run all tests on Base Sepolia (recommended first)

```bash
python test_escrow_scheme.py \
    --private-key YOUR_PRIVATE_KEY \
    --network sepolia
```

### Run validation tests only (no real payments)

```bash
python test_escrow_scheme.py \
    --private-key YOUR_PRIVATE_KEY \
    --network sepolia \
    --skip-real-payments
```

### Run on Base Mainnet (production)

```bash
python test_escrow_scheme.py \
    --private-key YOUR_PRIVATE_KEY \
    --network mainnet
```

### Specify custom receiver

```bash
python test_escrow_scheme.py \
    --private-key YOUR_PRIVATE_KEY \
    --network sepolia \
    --receiver 0xRECEIVER_ADDRESS
```

## Test Cases

| Test | Description | Costs USDC? |
|------|-------------|-------------|
| `health_check` | Verify facilitator is running | No |
| `balance_check` | Check payer's USDC balance | No |
| `wrong_scheme` | Verify wrong scheme is rejected | No |
| `expired_authorization` | Verify expired auth is rejected | No |
| `basic_authorize` | Full authorize flow | Yes (0.01 USDC) |

## Requirements

- Python 3.10+
- A wallet with USDC on Base (Sepolia or Mainnet)
- The wallet's private key

## What Happens

When you run `basic_authorize`:

1. Script creates an EscrowPayload with ERC-3009 signature
2. Sends to facilitator's `/settle` endpoint
3. Facilitator calls `PaymentOperator.authorize()`
4. USDC moves from your wallet to escrow contract
5. Payment is recorded, can be captured/refunded later

## Documentation

See `/docs/ESCROW_SCHEME.md` for complete documentation of the escrow scheme.
