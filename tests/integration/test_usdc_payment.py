#!/usr/bin/env python3
"""
Phase 3: Test USDC Payment on Base with Production Facilitator
Sends a real payment request and analyzes the response
"""

import os
import time
import requests
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3

print("=" * 80)
print("USDC PAYMENT TEST - Base Mainnet")
print("=" * 80)

# Configuration
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"
USDC_BASE_ADDRESS = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
SELLER_ADDRESS = "0x4dFB1Cd42604194e79eDaCff4e0d28A576e40d19"  # Test seller
PRICE_USDC = 10000  # 0.01 USDC (6 decimals)

# Test buyer with small USDC balance (TESTNET ONLY)
BUYER_PRIVATE_KEY = "0x" + "01" * 32  # TESTNET KEY - DO NOT USE IN PRODUCTION
buyer_account = Account.from_key(BUYER_PRIVATE_KEY)

print(f"\nBuyer Address: {buyer_account.address}")
print(f"Seller Address: {SELLER_ADDRESS}")
print(f"USDC Contract: {USDC_BASE_ADDRESS}")
print(f"Payment Amount: {PRICE_USDC} (0.01 USDC)")

# EIP-712 Domain for USDC on Base (from network.rs lines 139-150)
domain = {
    "name": "USD Coin",
    "version": "2",
    "chainId": 8453,  # Base mainnet
    "verifyingContract": USDC_BASE_ADDRESS
}

# Generate random nonce
nonce = "0x" + os.urandom(32).hex()

# Timestamps (EIP-3009 spec)
valid_after = int(time.time()) - 60  # Valid from 60s ago
valid_before = int(time.time()) + 600  # Valid for 10 minutes

# EIP-3009 TransferWithAuthorization message
message = {
    "from": buyer_account.address,
    "to": SELLER_ADDRESS,
    "value": PRICE_USDC,
    "validAfter": valid_after,
    "validBefore": valid_before,
    "nonce": nonce
}

# EIP-712 type definitions (from EIP-3009 spec)
types = {
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
}

# Create structured data
structured_data = {
    "types": types,
    "primaryType": "TransferWithAuthorization",
    "domain": domain,
    "message": message
}

print("\n" + "=" * 80)
print("SIGNING PAYMENT AUTHORIZATION")
print("=" * 80)

# Sign the message
encoded = encode_typed_data(full_message=structured_data)
signed = buyer_account.sign_message(encoded)

print(f"\nSignature Details:")
print(f"  v: {signed.v}")
print(f"  r: 0x{signed.r.to_bytes(32, 'big').hex()}")
print(f"  s: 0x{signed.s.to_bytes(32, 'big').hex()}")
print(f"  Full signature (hex): 0x{signed.signature.hex()}")
print(f"  Signature length: {len(signed.signature)} bytes")

# Build x402 payment payload (facilitator 0.9.0 format)
authorization = {
    "from": buyer_account.address,
    "to": SELLER_ADDRESS,
    "value": PRICE_USDC,
    "validAfter": str(valid_after),
    "validBefore": str(valid_before),
    "nonce": nonce
}

payload = {
    "x402Version": 1,  # Root level (VerifyRequest)
    "paymentPayload": {
        "x402Version": 1,  # Inside paymentPayload (PaymentPayload)
        "scheme": "exact",
        "network": "base",
        "payload": {
            "signature": signed.signature.hex(),  # WITHOUT 0x prefix
            "authorization": authorization
        }
    },
    "paymentRequirements": {
        "scheme": "exact",
        "network": "base",
        "maxAmountRequired": PRICE_USDC,
        "asset": USDC_BASE_ADDRESS,
        "extra": {
            "name": "USD Coin",
            "version": "2"
        }
    }
}

print("\n" + "=" * 80)
print("SENDING PAYMENT TO FACILITATOR")
print("=" * 80)

try:
    response = requests.post(
        f"{FACILITATOR_URL}/settle",
        json=payload,
        headers={"Content-Type": "application/json"},
        timeout=30
    )

    print(f"\nStatus Code: {response.status_code}")
    print(f"Response Body:")
    print(response.text)

    if response.status_code == 200:
        print("\n✅ PAYMENT SUCCEEDED!")
        response_data = response.json()
        if "txHash" in response_data:
            tx_hash = response_data["txHash"]
            print(f"\nTransaction: https://basescan.org/tx/{tx_hash}")
    else:
        print(f"\n❌ PAYMENT FAILED")
        print("\nDebugging Information:")
        print(f"  Domain: {domain}")
        print(f"  Message: from={message['from']}, to={message['to']}, value={message['value']}")
        print(f"  Nonce: {nonce}")
        print(f"  ValidAfter: {valid_after}")
        print(f"  ValidBefore: {valid_before}")

except Exception as e:
    print(f"\n❌ REQUEST FAILED: {e}")

print("\n" + "=" * 80)
