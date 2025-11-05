# x402 /settle Endpoint Troubleshooting Guide

## ✅ SOLVED: How to Fix "Invalid request" Errors

### The Problem

Your `/settle` endpoint was returning:
```
HTTP/1.1 400 Bad Request
{ "error": "Invalid request" }
```

### The Solution

**CRITICAL: `validAfter` and `validBefore` MUST be strings, not numbers!**

❌ **WRONG:**
```json
"authorization": {
  "validAfter": 1761329327,
  "validBefore": 1761829987
}
```

✅ **CORRECT:**
```json
"authorization": {
  "validAfter": "1761329327",
  "validBefore": "1761829987"
}
```

---

## 📋 Complete Working Payload Template

```json
{
  "x402Version": 1,
  "paymentPayload": {
    "x402Version": 1,
    "scheme": "exact",
    "network": "base",
    "payload": {
      "signature": "0x<130 hex chars>",
      "authorization": {
        "from": "0x...",
        "to": "0x...",
        "value": "100000",
        "validAfter": "1761329327",    // ← STRING (quoted)
        "validBefore": "1761829987",   // ← STRING (quoted)
        "nonce": "0x<64 hex chars>"
      }
    }
  },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "base",
    "maxAmountRequired": "100000",
    "resource": "http://localhost:3000/api/x402/resource",
    "description": "Payment description",
    "mimeType": "application/json",
    "payTo": "0x...",
    "maxTimeoutSeconds": 60,
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "extra": {
      "name": "USD Coin",
      "version": "2"
    }
  }
}
```

---

## 🔍 Error Messages and Fixes

### Error: "data did not match any variant of untagged enum ExactPaymentPayload"

**Cause:** The `authorization` object has invalid types (timestamps as numbers instead of strings).

**Fix:** Wrap `validAfter` and `validBefore` in quotes.

---

### Error: "missing field `x402Version`"

**Cause:** The `paymentPayload` object is missing the nested `x402Version` field.

**Fix:** Ensure BOTH top-level and `paymentPayload` have `x402Version: 1`.

```json
{
  "x402Version": 1,           // ← Top level
  "paymentPayload": {
    "x402Version": 1,         // ← Inside paymentPayload
    ...
  }
}
```

---

### Error: "missing field `maxAmountRequired`"

**Cause:** Using simplified `paymentRequirements` structure (with `amount` and `recipient` fields).

**Fix:** Use the FULL `paymentRequirements` structure with all required fields:
- `maxAmountRequired` (not `amount`)
- `payTo` (not `recipient`)
- `resource`
- `description`
- `mimeType`
- `maxTimeoutSeconds`
- `asset`
- `extra` (optional)

---

### Response: `{"isValid":false,"invalidReason":null,"payer":"0x..."}`

**Meaning:** The endpoint accepted your request! The payment is invalid (likely bad signature), but the JSON structure is correct.

**This is SUCCESS** - your payload format is now working!

---

## 🧪 Test Your Payload

### Using cURL:

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H 'Content-Type: application/json' \
  -d @tests/x402/payloads/WORKING_settle_template.json
```

**Expected response (valid structure):**
```json
{
  "isValid": false,
  "invalidReason": null,
  "payer": "0x87228cF28dd82546d76249A8Bb92AdEa9258F404"
}
```

---

## 📊 Quick Reference: Field Types

| Field | Type | Example |
|-------|------|---------|
| `x402Version` | Number | `1` |
| `signature` | String | `"0x1234..."` (130 hex chars) |
| `from` | String | `"0x..."` |
| `to` | String | `"0x..."` |
| `value` | String | `"100000"` |
| `validAfter` | **String** ⚠️ | `"1761329327"` |
| `validBefore` | **String** ⚠️ | `"1761829987"` |
| `nonce` | String | `"0x..."` (64 hex chars) |
| `maxAmountRequired` | String | `"100000"` |
| `maxTimeoutSeconds` | Number | `60` |

---

## 🚀 Load Testing Configuration

### For Artillery:

```yaml
- post:
    url: "/settle"
    json:
      x402Version: 1
      paymentPayload:
        x402Version: 1
        scheme: "exact"
        network: "base"
        payload:
          signature: "0x{{ $randomString(130, 'hex') }}"
          authorization:
            from: "0x87228cF28dd82546d76249A8Bb92AdEa9258F404"
            to: "0x87228cF28dd82546d76249A8Bb92AdEa9258F404"
            value: "100000"
            validAfter: "{{ $timestamp }}"        # ← String template
            validBefore: "{{ $timestamp + 3600 }}" # ← String template
            nonce: "0x{{ $randomString(64, 'hex') }}"
      paymentRequirements:
        scheme: "exact"
        network: "base"
        maxAmountRequired: "100000"
        resource: "http://localhost:3000/test"
        description: "Test"
        mimeType: "application/json"
        payTo: "0x87228cF28dd82546d76249A8Bb92AdEa9258F404"
        maxTimeoutSeconds: 60
        asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        extra:
          name: "USD Coin"
          version: "2"
```

### For k6:

```javascript
const now = Math.floor(Date.now() / 1000);

const payload = {
  x402Version: 1,
  paymentPayload: {
    x402Version: 1,
    scheme: "exact",
    network: "base",
    payload: {
      signature: randomSignature(),
      authorization: {
        from: "0x87228cF28dd82546d76249A8Bb92AdEa9258F404",
        to: "0x87228cF28dd82546d76249A8Bb92AdEa9258F404",
        value: "100000",
        validAfter: String(now),         // ← Convert to string
        validBefore: String(now + 3600), // ← Convert to string
        nonce: randomNonce()
      }
    }
  },
  paymentRequirements: {
    scheme: "exact",
    network: "base",
    maxAmountRequired: "100000",
    resource: "http://localhost:3000/test",
    description: "Test",
    mimeType: "application/json",
    payTo: "0x87228cF28dd82546d76249A8Bb92AdEa9258F404",
    maxTimeoutSeconds: 60,
    asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    extra: {
      name: "USD Coin",
      version: "2"
    }
  }
};
```

---

## ✅ Checklist Before Testing

- [ ] `validAfter` is a **string** (quoted)
- [ ] `validBefore` is a **string** (quoted)
- [ ] `x402Version` appears **twice** (top-level and in `paymentPayload`)
- [ ] `signature` is exactly 130 hex characters (after `0x`)
- [ ] `nonce` is exactly 64 hex characters (after `0x`)
- [ ] Using `maxAmountRequired` (not `amount`)
- [ ] Using `payTo` (not `recipient`)
- [ ] All required `paymentRequirements` fields are present

---

## 📞 Still Having Issues?

1. **Check the response body** - it contains detailed error messages
2. **Validate your JSON** - use https://jsonlint.com/
3. **Test with the working template** - `WORKING_settle_template.json`
4. **Run the Python test suite** - `python tests/x402/python/test_facilitator.py`

---

**Last Updated:** 2025-10-30
**Status:** ✅ RESOLVED
