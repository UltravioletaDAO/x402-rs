#!/usr/bin/env python3
"""Quick GLUE Payment Test - Uses existing PaymentSigner"""

import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent.parent))

import requests
import json
from web3 import Web3
from eth_account import Account
from shared.payment_signer import PaymentSigner

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

def main():
    print("\n" + "=" * 70)
    print("QUICK GLUE PAYMENT TEST (Version 1)")
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
    signer = PaymentSigner(
        glue_token_address=GLUE_TOKEN,
        chain_id=CHAIN_ID,
        token_version="1"  # CRITICAL: Must match contract
    )

    amount = 10_000  # 0.01 GLUE
    auth = signer.sign_transfer_authorization(
        from_address=buyer.address,
        to_address=seller,
        value=amount,
        private_key=key
    )

    print(f"  From: {auth['from'][:10]}...")
    print(f"  To: {auth['to'][:10]}...")
    print(f"  Value: {amount / 1e6} GLUE")
    print(f"  Nonce: {auth['nonce'][:10]}...")
    print(f"  Signature: {auth['signature'][:10]}...")

    # Make x402 request
    print("\n[4] Make x402 payment request")
    # Build x402-v1 Authorization header
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
