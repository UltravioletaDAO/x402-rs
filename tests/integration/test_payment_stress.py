#!/usr/bin/env python3
"""
Stress test facilitator on Base mainnet with USDC
Tests EIP-3009 transferWithAuthorization flow
"""
import requests
import json
import time
import secrets
from web3 import Web3
from eth_account import Account
from eth_account.messages import encode_typed_data
import sys

# Base Mainnet Configuration
RPC_URL_BASE = "https://mainnet.base.org"
CHAIN_ID_BASE = 8453
USDC_BASE = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"  # Official USDC on Base
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"

# Test seller address (karma-hello mainnet)
TEST_SELLER = "0x2C3e071df446B25B821F59425152838ae4931E75"

def get_buyer_wallet():
    """Get buyer wallet from AWS or env"""
    import os
    key = os.getenv('PRIVATE_KEY')
    if key:
        return Account.from_key(key)

    try:
        import boto3
        client = boto3.client('secretsmanager', region_name='us-east-2')
        response = client.get_secret_value(SecretId='facilitator-test-buyer')
        config = json.loads(response['SecretString'])
        if 'client-agent' in config:
            return Account.from_key(config['client-agent']['private_key'])
    except Exception as e:
        print(f"Error loading buyer wallet: {e}")

    return None

def check_usdc_balance(address):
    """Check USDC balance on Base mainnet"""
    w3 = Web3(Web3.HTTPProvider(RPC_URL_BASE))

    # ERC-20 balanceOf ABI
    abi = [{"constant":True,"inputs":[{"name":"_owner","type":"address"}],"name":"balanceOf","outputs":[{"name":"balance","type":"uint256"}],"type":"function"}]

    usdc = w3.eth.contract(address=Web3.to_checksum_address(USDC_BASE), abi=abi)
    balance = usdc.functions.balanceOf(Web3.to_checksum_address(address)).call()

    # USDC has 6 decimals
    return balance / 10**6

def sign_transfer_authorization(from_address, to_address, value, private_key):
    """Sign EIP-3009 transferWithAuthorization for USDC"""

    now = int(time.time())
    valid_after = now - 60  # Valid from 1 minute ago
    valid_before = now + 3600  # Valid for 1 hour
    nonce = "0x" + secrets.token_hex(32)

    # EIP-712 Domain for USDC on Base
    domain = {
        "name": "USD Coin",
        "version": "2",
        "chainId": CHAIN_ID_BASE,
        "verifyingContract": Web3.to_checksum_address(USDC_BASE)
    }

    # Message
    message = {
        "from": Web3.to_checksum_address(from_address),
        "to": Web3.to_checksum_address(to_address),
        "value": value,
        "validAfter": valid_after,
        "validBefore": valid_before,
        "nonce": nonce
    }

    # Types
    types = {
        "TransferWithAuthorization": [
            {"name": "from", "type": "address"},
            {"name": "to", "type": "address"},
            {"name": "value", "type": "uint256"},
            {"name": "validAfter", "type": "uint256"},
            {"name": "validBefore", "type": "uint256"},
            {"name": "nonce", "type": "bytes32"}
        ]
    }

    # Encode and sign
    typed_data = {
        "types": types,
        "primaryType": "TransferWithAuthorization",
        "domain": domain,
        "message": message
    }

    encoded = encode_typed_data(full_message=typed_data)
    signed = Account.sign_message(encoded, private_key=private_key)

    return {
        "from": from_address,
        "to": to_address,
        "value": str(value),
        "validAfter": str(valid_after),
        "validBefore": str(valid_before),
        "nonce": nonce,
        "v": signed.v,
        "r": hex(signed.r),
        "s": hex(signed.s),
        "signature": signed.signature.hex()
    }

def create_x402_payment(authorization):
    """Create x402 payment payload"""
    return {
        "x402Version": 1,
        "paymentPayload": {
            "x402Version": 1,
            "scheme": "exact",
            "network": "base",
            "payload": {
                "signature": authorization["signature"],
                "authorization": {
                    "from": authorization["from"],
                    "to": authorization["to"],
                    "value": authorization["value"],
                    "validAfter": authorization["validAfter"],
                    "validBefore": authorization["validBefore"],
                    "nonce": authorization["nonce"]
                }
            }
        },
        "paymentRequirements": {
            "scheme": "exact",
            "network": "base",
            "maxAmountRequired": authorization["value"],
            "resource": "https://karma-hello.karmacadabra.ultravioletadao.xyz/test",
            "description": "Base mainnet stress test",
            "mimeType": "application/json",
            "payTo": authorization["to"],
            "maxTimeoutSeconds": 300,
            "asset": USDC_BASE,
            "extra": {
                "name": "USD Coin",
                "version": "2"
            }
        }
    }

def test_facilitator_settle(payment):
    """Test facilitator /settle endpoint"""
    try:
        response = requests.post(
            f"{FACILITATOR_URL}/settle",
            json=payment,
            headers={'Content-Type': 'application/json'},
            timeout=30
        )

        if response.status_code == 200:
            data = response.json()
            return {
                "success": True,
                "tx_hash": data.get("transactionHash") or data.get("transaction_hash"),
                "response": data
            }
        else:
            return {
                "success": False,
                "status": response.status_code,
                "error": response.text[:200]
            }
    except Exception as e:
        return {
            "success": False,
            "error": str(e)
        }

def main():
    print("="*70)
    print("BASE MAINNET STRESS TEST - USDC")
    print("="*70)

    # 1. Load buyer wallet
    buyer = get_buyer_wallet()
    if not buyer:
        print("❌ Failed to load buyer wallet")
        return

    print(f"\n[1] Buyer wallet loaded")
    print(f"    Address: {buyer.address}")

    # 2. Check USDC balance
    print(f"\n[2] Checking USDC balance on Base mainnet...")
    try:
        balance = check_usdc_balance(buyer.address)
        print(f"    Balance: ${balance:.2f} USDC")

        if balance < 1:
            print(f"\n[WARNING] Low USDC balance (${balance:.6f})")
            print(f"    Need at least $1 USDC for stress testing")
            print(f"    Base USDC: {USDC_BASE}")

            response = input("\n    Continue anyway? (y/n): ")
            if response.lower() != 'y':
                print("    Aborted by user")
                return
    except Exception as e:
        print(f"    [WARNING] Could not check balance: {e}")
        print(f"    Continuing anyway...")

    # 3. Test parameters
    print(f"\n[3] Test configuration")
    print(f"    Seller: {TEST_SELLER}")
    print(f"    Facilitator: {FACILITATOR_URL}")
    print(f"    Network: Base Mainnet (Chain {CHAIN_ID_BASE})")
    print(f"    Token: USDC ({USDC_BASE})")

    num_tests = int(input("\n    How many test transactions? (1-100): ") or "10")
    amount_cents = int(input("    Amount per transaction (cents): ") or "1")

    amount_usdc = amount_cents * 10**4  # Convert cents to USDC units (6 decimals)

    print(f"\n[4] Running {num_tests} tests at ${amount_cents/100:.2f} each")
    print(f"    Total: ${(num_tests * amount_cents)/100:.2f} USDC")
    print(f"    " + "-"*60)

    # 4. Run stress tests
    results = {
        "success": 0,
        "failed": 0,
        "errors": [],
        "tx_hashes": [],
        "total_time": 0
    }

    for i in range(num_tests):
        print(f"\n    Test {i+1}/{num_tests}:", end=" ")
        start = time.time()

        try:
            # Sign authorization
            auth = sign_transfer_authorization(
                from_address=buyer.address,
                to_address=TEST_SELLER,
                value=amount_usdc,
                private_key=buyer.key.hex()
            )

            # Create payment
            payment = create_x402_payment(auth)

            # Submit to facilitator
            result = test_facilitator_settle(payment)

            elapsed = time.time() - start
            results["total_time"] += elapsed

            if result["success"]:
                results["success"] += 1
                results["tx_hashes"].append(result["tx_hash"])
                print(f"✅ SUCCESS ({elapsed:.2f}s) - TX: {result['tx_hash'][:16]}...")
            else:
                results["failed"] += 1
                error_msg = result.get("error", "Unknown error")[:50]
                results["errors"].append(error_msg)
                print(f"❌ FAILED ({elapsed:.2f}s) - {error_msg}")

        except Exception as e:
            elapsed = time.time() - start
            results["failed"] += 1
            results["errors"].append(str(e)[:50])
            print(f"❌ ERROR ({elapsed:.2f}s) - {str(e)[:50]}")

        # Brief delay between requests
        if i < num_tests - 1:
            time.sleep(0.5)

    # 5. Summary
    print(f"\n" + "="*70)
    print(f"STRESS TEST RESULTS")
    print(f"="*70)
    print(f"Success: {results['success']}/{num_tests} ({results['success']/num_tests*100:.1f}%)")
    print(f"Failed:  {results['failed']}/{num_tests} ({results['failed']/num_tests*100:.1f}%)")
    print(f"Avg time: {results['total_time']/num_tests:.2f}s per transaction")
    print(f"Total time: {results['total_time']:.2f}s")

    if results['tx_hashes']:
        print(f"\n✅ Successful transactions:")
        for tx in results['tx_hashes'][:5]:  # Show first 5
            print(f"   https://basescan.org/tx/{tx}")
        if len(results['tx_hashes']) > 5:
            print(f"   ... and {len(results['tx_hashes'])-5} more")

    if results['errors']:
        print(f"\n❌ Errors encountered:")
        unique_errors = list(set(results['errors']))
        for error in unique_errors[:5]:  # Show first 5 unique
            print(f"   - {error}")

    print(f"\n" + "="*70)

    # Final balance check
    if results['success'] > 0:
        print(f"\n[5] Final balance check...")
        try:
            new_balance = check_usdc_balance(buyer.address)
            spent = (balance - new_balance) if balance else 0
            print(f"    New balance: ${new_balance:.2f} USDC")
            if spent > 0:
                print(f"    Spent: ${spent:.2f} USDC")
        except:
            pass

if __name__ == "__main__":
    main()
