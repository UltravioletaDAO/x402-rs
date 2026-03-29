# SKALE Docs PR — Ultravioleta DAO Facilitator

Instructions for PR to https://github.com/skalenetwork/docs.skale.space

---

## Change 1: Add to Facilitators List

**File**: The facilitators table at `get-started/agentic-builders/facilitators`

**Add this row to the table:**

```
| [Ultravioleta DAO](https://ultravioletadao.xyz) | https://facilitator.ultravioletadao.xyz | [Using Ultravioleta](/cookbook/facilitators/using-ultravioleta) | x402 v1 + v2 |
```

---

## Change 2: New Cookbook Page

**File**: Create `cookbook/facilitators/using-ultravioleta.md` (or `.mdx` depending on their doc framework)

**Content below** — follows the Corbits cookbook format:

---

```markdown
---
title: Ultravioleta DAO
description: Integrate Ultravioleta DAO facilitator for x402 payment processing on SKALE Base
---

# Ultravioleta DAO

Integrate the Ultravioleta DAO facilitator for gasless x402 payment processing on SKALE Base and 20+ other blockchain networks.

**Facilitator URL**: `https://facilitator.ultravioletadao.xyz`

**Supported Networks**: 33 networks across EVM (Base, Ethereum, Polygon, Arbitrum, Optimism, Avalanche, Celo, SKALE Base, BSC, Monad, HyperEVM, Unichain, Scroll), Solana, Fogo, NEAR, Stellar, Algorand, and Sui.

**Stablecoins**: USDC, EURC, AUSD, PYUSD, USDT (varies by network). SKALE Base uses USDC.e (bridged from Base).

**Protocol**: x402 v1 and v2 with auto-detection.

## Prerequisites

- Node.js 18+ or Python 3.10+
- SKALE Base RPC endpoint: `https://skale-base.skalenodes.com/v1/base`
- A wallet with USDC.e on SKALE Base
- Familiarity with the [x402 protocol](https://www.x402.org/)

## SKALE Base Configuration

| Property | Value |
|----------|-------|
| Chain ID | `1187947933` |
| CAIP-2 | `eip155:1187947933` |
| x402 Network Name | `skale-base` |
| USDC.e Contract | `0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20` |
| Gas Token | CREDIT (free) |
| EIP-1559 | No (legacy transactions only) |

## Server Setup (TypeScript)

Install the SDK:

```bash
npm install uvd-x402-sdk ethers
```

Create a paywall server:

```typescript
import { Hono } from 'hono';
import { serve } from '@hono/node-server';

const app = new Hono();

const FACILITATOR_URL = 'https://facilitator.ultravioletadao.xyz';
const RECEIVING_ADDRESS = '0xYourWalletAddress';

// Free endpoint
app.get('/api/free', (c) => {
  return c.json({
    message: 'This endpoint is free!',
    timestamp: new Date().toISOString(),
  });
});

// Protected endpoint - returns 402 if no payment
app.get('/api/premium', async (c) => {
  const payment = c.req.header('X-PAYMENT');

  if (!payment) {
    return c.json({
      x402Version: 1,
      paymentRequirements: [{
        scheme: 'exact',
        network: 'skale-base',
        maxAmountRequired: '1000000', // $1.00 USDC (6 decimals)
        resource: '/api/premium',
        description: 'Premium API access',
        mimeType: 'application/json',
        payTo: RECEIVING_ADDRESS,
        asset: '0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20',
        extra: {
          name: 'Bridged USDC (SKALE Bridge)',
          version: '2',
        },
      }],
    }, 402);
  }

  // Verify and settle via facilitator
  const verifyRes = await fetch(`${FACILITATOR_URL}/verify`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      x402Version: 1,
      paymentHeader: payment,
    }),
  });

  if (!verifyRes.ok) {
    return c.json({ error: 'Payment verification failed' }, 400);
  }

  const settleRes = await fetch(`${FACILITATOR_URL}/settle`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      x402Version: 1,
      paymentHeader: payment,
    }),
  });

  const settlement = await settleRes.json();

  return c.json({
    message: 'Premium content delivered!',
    settlement,
    timestamp: new Date().toISOString(),
  });
});

serve({ fetch: app.fetch, port: 3000 });
console.log('Server running on http://localhost:3000');
```

## Server Setup (Python)

Install the SDK:

```bash
pip install uvd-x402-sdk
```

Create a paywall server with FastAPI:

```python
from decimal import Decimal
from fastapi import FastAPI, Request
from uvd_x402_sdk import X402Client, X402Config, create_402_response

app = FastAPI()

config = X402Config(
    recipient_evm="0xYourWalletAddress",
    facilitator_url="https://facilitator.ultravioletadao.xyz",
)
client = X402Client(config=config)

@app.get("/api/free")
async def free_endpoint():
    return {"message": "This endpoint is free!"}

@app.get("/api/premium")
async def premium_endpoint(request: Request):
    payment = request.headers.get("X-PAYMENT")

    if not payment:
        return create_402_response(
            amount_usd=Decimal("1.00"),
            config=config,
            resource="/api/premium",
            description="Premium API access",
            network="skale-base",
        )

    result = client.process_payment(payment, Decimal("1.00"))
    return {
        "message": "Premium content delivered!",
        "payer": result.payer_address,
        "network": result.network,
        "tx": result.transaction_hash,
    }
```

## Client Setup (TypeScript)

```typescript
import { X402Client } from 'uvd-x402-sdk';

const client = new X402Client({ defaultChain: 'skale-base' });
const address = await client.connect('skale-base');

const result = await client.createPayment({
  recipient: '0xMerchantAddress',
  amount: '1.00',
});

const response = await fetch('https://merchant.example.com/api/premium', {
  headers: { 'X-PAYMENT': result.paymentHeader },
});

const data = await response.json();
console.log('Received:', data);
```

## Key Features on SKALE Base

- **Zero gas costs**: The facilitator pays gas using CREDIT tokens (free on SKALE)
- **EIP-3009 support**: USDC.e on SKALE supports `transferWithAuthorization` for gasless payments
- **ERC-8004 reputation**: On-chain agent identity and reputation system
- **Legacy transactions**: SKALE uses type 0 (legacy) transactions — the facilitator handles this automatically

## API Reference

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/verify` | POST | Verify payment authorization |
| `/settle` | POST | Submit payment on-chain |
| `/accepts` | POST | Negotiate payment requirements |
| `/supported` | GET | List supported networks and tokens |
| `/health` | GET | Health check |
| `/version` | GET | Facilitator version |
| `/docs` | GET | Interactive Swagger UI |
| `/api-docs/openapi.json` | GET | OpenAPI 3.0 spec |

Full interactive API docs: [facilitator.ultravioletadao.xyz/docs](https://facilitator.ultravioletadao.xyz/docs)

## EIP-712 Domain (SKALE Base USDC.e)

When constructing EIP-3009 authorizations for SKALE Base, use this domain:

```json
{
  "name": "Bridged USDC (SKALE Bridge)",
  "version": "2",
  "chainId": 1187947933,
  "verifyingContract": "0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20"
}
```

:::caution
The domain name is `"Bridged USDC (SKALE Bridge)"` — not `"USDC"` or `"USD Coin"`. Using the wrong name will cause signature verification to fail.
:::

## Troubleshooting

| Issue | Solution |
|-------|----------|
| "Invalid signature" on SKALE | Verify EIP-712 domain name is `"Bridged USDC (SKALE Bridge)"` with version `"2"` |
| Transaction reverts | SKALE uses legacy transactions (no EIP-1559). Ensure your client does not set `maxFeePerGas` |
| Balance shows 0 | Check USDC.e balance at `0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20`, not native CREDIT |
| RPC timeout | Use public RPC: `https://skale-base.skalenodes.com/v1/base` (no API key needed) |

## Resources

- [Ultravioleta DAO](https://ultravioletadao.xyz)
- [Facilitator API Docs](https://facilitator.ultravioletadao.xyz/docs)
- [OpenAPI Spec](https://facilitator.ultravioletadao.xyz/api-docs/openapi.json)
- [TypeScript SDK](https://www.npmjs.com/package/uvd-x402-sdk)
- [Python SDK](https://pypi.org/project/uvd-x402-sdk/)
- [x402 Protocol](https://www.x402.org/)
- [GitHub](https://github.com/UltravioletaDAO/x402-rs)

:::note Disclaimer
Ultravioleta DAO is a third-party facilitator. Use at your own risk. SKALE does not endorse or guarantee the service.
:::
```

---

## How to Submit the PR

```bash
# 1. Fork the repo
gh repo fork skalenetwork/docs.skale.space --clone
cd docs.skale.space

# 2. Create branch
git checkout -b feat/add-ultravioleta-facilitator

# 3. Add the facilitator to the table (find the facilitators page)
# Edit the table to add the Ultravioleta row

# 4. Create the cookbook page
# Copy the content above into the appropriate file

# 5. Commit and push
git add .
git commit -m "feat: add Ultravioleta DAO facilitator to docs"
git push origin feat/add-ultravioleta-facilitator

# 6. Create PR
gh pr create --title "Add Ultravioleta DAO facilitator" --body "Adds Ultravioleta DAO as a facilitator option with cookbook integration guide. Facilitator supports 33 networks including SKALE Base with USDC.e payments and ERC-8004 reputation."
```
