#!/usr/bin/env python3
"""
Check Facilitator GLUE Version Configuration

This script verifies what EIP-712 version the facilitator expects for GLUE token
by creating test signatures with version "1" and "2" and checking which one works.
"""

import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent.parent))

import requests
import json
from web3 import Web3
from eth_account import Account
from shared.payment_signer import PaymentSigner
import secrets as secrets_module

# Configuration
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"
GLUE_TOKEN = "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743"
CHAIN_ID = 43113

def get_test_wallet():
    """Get test wallet from AWS"""
    try:
        import boto3
        client = boto3.client('secretsmanager', region_name='us-east-2')
        response = client.get_secret_value(SecretId='facilitator-test-buyer')
        config = json.loads(response['SecretString'])
        if 'client-agent' in config:
            return config['client-agent'].get('private_key')
    except:
        pass
    return None

def test_facilitator_verify(version: str, buyer_key: str, seller_addr: str) -> dict:
    """
    Test facilitator /v1/verify endpoint with specific version

    Returns: dict with 'success', 'status_code', 'error' keys
    """

    buyer = Account.from_key(buyer_key)

    # Create payment signature with specified version
    signer = PaymentSigner(
        glue_token_address=GLUE_TOKEN,
        chain_id=CHAIN_ID,
        token_version=version
    )

    amount = 10_000  # 0.01 GLUE
    auth = signer.sign_transfer_authorization(
        from_address=buyer.address,
        to_address=seller_addr,
        value=amount,
        private_key=buyer_key
    )

    # Build verify request payload (Rust uses snake_case for field names)
    payload = {
        "x402Version": 1,  # x402 protocol version (not EIP-712 version)
        "paymentPayload": {
            "x402Version": 1,
            "scheme": "exact",
            "network": "avalanche-fuji",
            "payload": {
                "asset": GLUE_TOKEN,
                "from": auth['from'],
                "to": auth['to'],
                "value": str(auth['value']),
                "validAfter": str(auth['validAfter']),
                "validBefore": str(auth['validBefore']),
                "nonce": auth['nonce'],
                "v": auth['v'],
                "r": auth['r'],
                "s": auth['s']
            }
        },
        "paymentRequirements": {
            "network": "avalanche-fuji",
            "asset": GLUE_TOKEN,
            "receiver": seller_addr,
            "value": str(amount),
            "extra": {
                "name": "Gasless Ultravioleta DAO Extended Token",
                "version": version  # THIS is what we're testing
            }
        }
    }

    try:
        resp = requests.post(
            f"{FACILITATOR_URL}/verify",
            json=payload,
            timeout=15
        )

        result = {
            'success': resp.status_code == 200,
            'status_code': resp.status_code,
            'response': resp.text[:500] if resp.text else None
        }

        if resp.status_code == 200:
            try:
                data = resp.json()
                result['valid'] = data.get('valid', False)
                result['response_json'] = data
            except:
                pass

        return result

    except Exception as e:
        return {
            'success': False,
            'status_code': None,
            'error': str(e)
        }

def main():
    print("\n" + "=" * 70)
    print("FACILITATOR GLUE VERSION CHECK")
    print("=" * 70)
    print("\nThis script tests which EIP-712 version the facilitator expects")
    print("for GLUE token by making verify requests with version 1 and 2.\n")
    print("=" * 70)

    # Load test wallet
    print("\n[1] Load test wallet")
    key = get_test_wallet()
    if not key:
        print("  [FAIL] No wallet found")
        return

    buyer = Account.from_key(key)
    print(f"  Buyer: {buyer.address}")

    # Use karma-hello as test seller
    seller = "0x2C3e071df446B25B821F59425152838ae4931E75"
    print(f"  Seller: {seller}")

    # Test version "1"
    print("\n[2] Test facilitator with version='1'")
    print("  Creating EIP-712 signature with version='1'...")
    result_v1 = test_facilitator_verify("1", key, seller)

    print(f"  HTTP Status: {result_v1.get('status_code', 'ERROR')}")
    if result_v1['success']:
        print(f"  [SUCCESS] Facilitator accepts version='1'")
        if 'valid' in result_v1:
            print(f"  Signature valid: {result_v1['valid']}")
    else:
        print(f"  [FAIL] Facilitator rejects version='1'")
        if 'error' in result_v1:
            print(f"  Error: {result_v1['error']}")
        elif 'response' in result_v1:
            print(f"  Response: {result_v1['response']}")

    # Test version "2"
    print("\n[3] Test facilitator with version='2'")
    print("  Creating EIP-712 signature with version='2'...")
    result_v2 = test_facilitator_verify("2", key, seller)

    print(f"  HTTP Status: {result_v2.get('status_code', 'ERROR')}")
    if result_v2['success']:
        print(f"  [SUCCESS] Facilitator accepts version='2'")
        if 'valid' in result_v2:
            print(f"  Signature valid: {result_v2['valid']}")
    else:
        print(f"  [FAIL] Facilitator rejects version='2'")
        if 'error' in result_v2:
            print(f"  Error: {result_v2['error']}")
        elif 'response' in result_v2:
            print(f"  Response: {result_v2['response']}")

    # Conclusion
    print("\n" + "=" * 70)
    print("CONCLUSION")
    print("=" * 70)

    if result_v1['success'] and not result_v2['success']:
        print("\n  The facilitator expects GLUE version='1'")
        print("  Status: Facilitator correctly configured!")
    elif result_v2['success'] and not result_v1['success']:
        print("\n  The facilitator expects GLUE version='2'")
        print("  Status: Facilitator needs update to version='1'!")
        print("\n  Action required:")
        print("    1. Rebuild facilitator: cd x402-rs && cargo build --release")
        print("    2. Redeploy facilitator service")
    elif result_v1['success'] and result_v2['success']:
        print("\n  WARNING: Facilitator accepts both versions")
        print("  This shouldn't happen - check facilitator logic")
    else:
        print("\n  ERROR: Facilitator rejects both versions")
        print("  Check facilitator logs and configuration")

    print("\n" + "=" * 70)

if __name__ == '__main__':
    main()
