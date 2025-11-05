#!/usr/bin/env python3
"""Standalone GLUE Payment Test - No shared imports"""

import requests
import json
import time
from web3 import Web3
from eth_account import Account
from eth_account.messages import encode_typed_data

# Configuration
AGENT_URL = "https://karma-hello.karmacadabra.ultravioletadao.xyz"
RPC_URL = "https://avalanche-fuji-c-chain-rpc.publicnode.com"
GLUE_TOKEN = "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743"
CHAIN_ID = 43113

def get_buyer():
    """Get buyer from AWS"""
    import os
    key = os.getenv('PRIVATE_KEY')
    if key:
        return key

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

def sign_transfer_authorization(from_address, to_address, value, private_key):
    """Sign EIP-3009 transferWithAuthorization"""
    import secrets

    now = int(time.time())
    valid_after = now - 3600
    valid_before = now + 3600
    nonce = "0x" + secrets.token_hex(32)

    # EIP-712 Domain
    domain = {
        "name": "GLUE Token",
        "version": "1",  # CRITICAL: Match contract version
        "chainId": CHAIN_ID,
        "verifyingContract": Web3.to_checksum_address(GLUE_TOKEN)
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

    encoded_message = encode_typed_data(full_message=typed_data)
    account = Account.from_key(private_key)
    signature = account.sign_message(encoded_message)

    return {
        "from": message["from"],
        "to": message["to"],
        "value": value,
        "validAfter": valid_after,
        "validBefore": valid_before,
        "nonce": nonce,
        "v": signature.v,
        "r": hex(signature.r),
        "s": hex(signature.s),
        "signature": signature.signature.hex()
    }

def main():
    print("\n" + "=" * 70)
    print("STANDALONE GLUE PAYMENT TEST (EIP-712 Version 1)")
    print("=" * 70)

    # Load buyer
    print("\n[1] Load buyer wallet")
    key = get_buyer()
    if not key:
        print("[FAIL] No wallet found")
        return

    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    buyer = Account.from_key(key)
    print(f"  Buyer: {buyer.address}")

    # Check balance
    glue_abi = [{"inputs": [{"internalType": "address", "name": "account", "type": "address"}], "name": "balanceOf", "outputs": [{"internalType": "uint256", "name": "", "type": "uint256"}], "stateMutability": "view", "type": "function"}]
    glue = w3.eth.contract(address=Web3.to_checksum_address(GLUE_TOKEN), abi=glue_abi)
    balance = glue.functions.balanceOf(buyer.address).call()
    print(f"  GLUE Balance: {balance / 1e6:.6f} GLUE")

    if balance < 10_000:
        print("[FAIL] Need at least 0.01 GLUE")
        return

    # Get seller
    print("\n[2] Get seller address")
    try:
        resp = requests.get(f"{AGENT_URL}/.well-known/agent-card", timeout=10)
        seller = resp.json().get('wallet_address')
        print(f"  Seller: {seller}")
    except Exception as e:
        print(f"[FAIL] {e}")
        return

    # Sign payment
    print("\n[3] Sign EIP-3009 authorization (version '1')")
    amount = 10_000  # 0.01 GLUE
    auth = sign_transfer_authorization(
        from_address=buyer.address,
        to_address=seller,
        value=amount,
        private_key=key
    )

    print(f"  From: {auth['from'][:10]}...")
    print(f"  To: {auth['to'][:10]}...")
    print(f"  Value: {amount / 1e6} GLUE")
    print(f"  Nonce: {auth['nonce'][:10]}...")
    print(f"  Signature: v={auth['v']}, r={auth['r'][:10]}..., s={auth['s'][:10]}...")

    # Make x402 request
    print("\n[4] Make x402 payment request")
    auth_header = (
        f"x402-v1 token={GLUE_TOKEN} "
        f"from={auth['from']} "
        f"to={auth['to']} "
        f"value={auth['value']} "
        f"validAfter={auth['validAfter']} "
        f"validBefore={auth['validBefore']} "
        f"nonce={auth['nonce']} "
        f"v={auth['v']} "
        f"r={auth['r']} "
        f"s={auth['s']}"
    )

    headers = {
        'Authorization': auth_header,
        'Content-Type': 'application/json'
    }

    try:
        resp = requests.post(
            f"{AGENT_URL}/get_chat_logs",
            headers=headers,
            json={"username": "test", "date": "2024-10-01"},
            timeout=30
        )

        print(f"  HTTP Status: {resp.status_code}")

        if resp.status_code == 200:
            data = resp.json() if 'application/json' in resp.headers.get('content-type', '') else resp.text
            tx_hash = None
            if isinstance(data, dict):
                tx_hash = data.get('payment_tx') or data.get('tx_hash')

            if tx_hash:
                print(f"  [SUCCESS] Payment accepted!")
                print(f"\n[5] Transaction details")
                print(f"  Tx Hash: {tx_hash}")
                print(f"  Snowtrace: https://testnet.snowtrace.io/tx/{tx_hash}")

                try:
                    receipt = w3.eth.get_transaction_receipt(tx_hash)
                    if receipt['status'] == 1:
                        print(f"  Block: {receipt['blockNumber']:,}")
                        print(f"  Gas: {receipt['gasUsed']:,}")
                        print(f"  [CONFIRMED] Transaction successful on-chain!")
                    else:
                        print(f"  [FAIL] Transaction reverted on-chain")
                except Exception as e:
                    print(f"  [WARN] Could not verify: {e}")
            else:
                print(f"  [SUCCESS] Payment accepted (no tx hash in response)")
                print(f"  Response: {str(data)[:200]}")
        else:
            print(f"  [FAIL] Payment rejected")
            print(f"  Response: {resp.text[:300]}")

    except Exception as e:
        print(f"  [FAIL] Request failed: {e}")

    print("\n" + "=" * 70)
    print("Test complete")
    print("=" * 70)

if __name__ == '__main__':
    main()
