# Handoff: SKALE ERC-8004 Registration Fails

## Problem

`POST /register` with `network: "skale-base"` returns:

```json
{
  "success": false,
  "error": "Failed to send registration transaction: server returned an error response: error code -32602: INVALID_PARAMS: Invalid method parameters (invalid name and/or type) recognised",
  "network": "skale-base"
}
```

Identity READ works fine (`GET /identity/skale-base/1` returns Agent #1 from Relai.fi). Only WRITE operations fail.

## Reproduce

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/register \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "skale-base",
    "agentUri": "https://execution.market/agents/0x52e05c8e45a32eee169639f6d2ca40f8887b5a15",
    "recipient": "0x52e05c8e45a32eee169639f6d2ca40f8887b5a15"
  }'
```

## Root Cause (likely)

SKALE uses a non-standard gas model. The chain has zero gas fees but requires sFUEL (a free gas token) and uses different transaction parameters. The facilitator probably builds the TX with standard EVM gas params (`maxFeePerGas`, `maxPriorityFeePerGas`, or legacy `gasPrice`) that SKALE's RPC rejects as `INVALID_PARAMS`.

## What to check in x402-rs

1. **`src/network.rs`** — How gas params are set for `Network::SkaleBase`. SKALE may need `gasPrice: 0` or omit gas fields entirely.

2. **`src/erc8004/mod.rs`** — The `register` function builds and sends the TX. Check if it uses a generic TX builder that doesn't handle SKALE's gas model.

3. **RPC endpoint** — Verify the SKALE RPC URL is correct: `https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague` (Europa chain) or the chain-specific endpoint for chain ID `1187947933`.

4. **sFUEL balance** — The facilitator's wallet needs sFUEL on SKALE. Check balance:
   ```bash
   cast balance <FACILITATOR_WALLET> --rpc-url https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague
   ```
   sFUEL is free — get it from https://sfuel.skale.network/

## Impact

Execution Market can't register agent identities on SKALE. Tasks on SKALE show Agent #37500 (Base ID) instead of a SKALE-native ID. Payments work fine (different flow), only ERC-8004 identity/reputation writes are broken.

## Also affects

- `POST /feedback` (reputation writes) — likely same `INVALID_PARAMS` error
- `POST /feedback/revoke`
- `POST /feedback/response`
