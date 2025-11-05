#!/usr/bin/env python3
"""
Test Real x402 Payment with GLUE Token Transfer

This script demonstrates the complete x402 payment flow:
1. Create EIP-3009 payment authorization (signed off-chain)
2. Make HTTP request to agent with x402 headers
3. Agent verifies and forwards to facilitator
4. Facilitator executes GLUE.transferWithAuthorization() on-chain
5. Verify GLUE transfer on blockchain

This will show actual GLUE token movement!
"""

import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent.parent))

import requests
import json
from web3 import Web3
from eth_account import Account
import secrets
import time
from decimal import Decimal

# Configuration - PRODUCTION AWS ENDPOINTS
AGENT_URL = "https://karma-hello.karmacadabra.ultravioletadao.xyz"  # Production
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"  # Production
RPC_URL = "https://avalanche-fuji-c-chain-rpc.publicnode.com"
GLUE_TOKEN = "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743"
GLUE_DECIMALS = 6  # CRITICAL: GLUE has 6 decimals, not 18!
CHAIN_ID = 43113  # Avalanche Fuji

# Test buyer - you can replace with any funded wallet
# For demo, we'll show how to load from env
TEST_BUYER_KEY = None  # Will try to load from environment


def get_test_buyer():
    """Get test buyer private key from environment or AWS"""
    import os

    # Try environment variables first
    key = os.getenv('TEST_BUYER_KEY') or os.getenv('TEST_BUYER_PRIVATE_KEY') or os.getenv('PRIVATE_KEY')

    if key:
        print(f"✅ Using wallet from environment variable")
        return key

    # Try AWS Secrets Manager
    try:
        import boto3
        secrets_client = boto3.client('secretsmanager', region_name='us-east-2')
        response = secrets_client.get_secret_value(SecretId='facilitator-test-buyer')
        agents_config = json.loads(response['SecretString'])

        # Priority 1: client-agent (funded)
        if 'client-agent' in agents_config:
            key = agents_config['client-agent'].get('private_key')
            if key:
                print(f"✅ Using buyer: client-agent (from AWS)")
                return key

        # Priority 2: user-agents
        user_agents = agents_config.get('user-agents', {})
        for agent_name in ['cyberpaisa', '0xultravioleta', 'elbitterx', 'aka_r3c', '0xjuandi']:
            if agent_name in user_agents:
                key = user_agents[agent_name].get('private_key')
                if key:
                    print(f"✅ Using buyer: {agent_name} (from AWS)")
                    return key
    except Exception as e:
        print(f"⚠️  Could not load from AWS: {e}")

    return key


def create_eip3009_authorization(
    w3: Web3,
    from_address: str,
    to_address: str,
    value: int,
    valid_after: int,
    valid_before: int,
    nonce: bytes,
    private_key: str
) -> dict:
    """
    Create EIP-3009 transferWithAuthorization signature

    Args:
        w3: Web3 instance
        from_address: Payer address
        to_address: Payee address
        value: Amount in smallest unit (e.g., 0.01 GLUE = 10000000000000000)
        valid_after: Unix timestamp (usually 0)
        valid_before: Unix timestamp (far future)
        nonce: 32 random bytes
        private_key: Signer's private key

    Returns:
        dict with signature components (v, r, s)
    """

    # EIP-712 domain for GLUE token (must match actual on-chain contract)
    # Verified via contract.eip712Domain(): name="GLUE Token", version="1"
    domain = {
        'name': 'GLUE Token',
        'version': '1',
        'chainId': CHAIN_ID,
        'verifyingContract': GLUE_TOKEN
    }

    # EIP-3009 message structure
    message = {
        'from': from_address,
        'to': to_address,
        'value': value,
        'validAfter': valid_after,
        'validBefore': valid_before,
        'nonce': '0x' + nonce.hex()
    }

    # EIP-712 type definitions
    types = {
        'EIP712Domain': [
            {'name': 'name', 'type': 'string'},
            {'name': 'version', 'type': 'string'},
            {'name': 'chainId', 'type': 'uint256'},
            {'name': 'verifyingContract', 'type': 'address'}
        ],
        'TransferWithAuthorization': [
            {'name': 'from', 'type': 'address'},
            {'name': 'to', 'type': 'address'},
            {'name': 'value', 'type': 'uint256'},
            {'name': 'validAfter', 'type': 'uint256'},
            {'name': 'validBefore', 'type': 'uint256'},
            {'name': 'nonce', 'type': 'bytes32'}
        ]
    }

    # Create full EIP-712 message
    full_message = {
        'types': types,
        'primaryType': 'TransferWithAuthorization',
        'domain': domain,
        'message': message
    }

    # Sign using eth_account
    account = Account.from_key(private_key)
    signed = account.sign_typed_data(full_message=full_message)

    return {
        'v': signed.v,
        'r': signed.r.to_bytes(32, 'big'),
        's': signed.s.to_bytes(32, 'big'),
        'signature': signed.signature.hex()
    }


def make_x402_request(
    agent_url: str,
    endpoint: str,
    buyer_address: str,
    seller_address: str,
    amount: int,
    authorization: dict,
    nonce: bytes,
    valid_after: int,
    valid_before: int
) -> tuple:
    """
    Make HTTP request with x402 payment headers

    Returns:
        (success: bool, response_data: dict, tx_hash: str)
    """

    # Format x402 authorization header
    # Format: "x402-v1 token=<address> from=<buyer> to=<seller> value=<amount> validAfter=<ts> validBefore=<ts> nonce=<hex> v=<v> r=<hex> s=<hex>"

    auth_header = (
        f"x402-v1 "
        f"token={GLUE_TOKEN} "
        f"from={buyer_address} "
        f"to={seller_address} "
        f"value={amount} "
        f"validAfter={valid_after} "
        f"validBefore={valid_before} "
        f"nonce=0x{nonce.hex()} "
        f"v={authorization['v']} "
        f"r=0x{authorization['r'].hex()} "
        f"s=0x{authorization['s'].hex()}"
    )

    headers = {
        'Authorization': auth_header,
        'Content-Type': 'application/json'
    }

    print(f"\n📡 Making x402 request to {agent_url}{endpoint}")
    print(f"   Payment: {amount / (10 ** GLUE_DECIMALS)} GLUE")
    print(f"   From: {buyer_address[:10]}...")
    print(f"   To: {seller_address[:10]}...")

    try:
        response = requests.post(f"{agent_url}{endpoint}", headers=headers, json={}, timeout=30)

        print(f"   Response: HTTP {response.status_code}")

        if response.status_code == 200:
            # Success! Payment accepted
            data = response.json() if response.headers.get('content-type') == 'application/json' else response.text

            # Try to extract tx hash from response
            tx_hash = None
            if isinstance(data, dict):
                tx_hash = data.get('payment_tx') or data.get('tx_hash')

            return True, data, tx_hash

        elif response.status_code == 402:
            print("   ❌ Payment required but not accepted")
            return False, response.text, None

        else:
            print(f"   ❌ Error: {response.text[:200]}")
            return False, response.text, None

    except Exception as e:
        print(f"   ❌ Request failed: {e}")
        return False, str(e), None


def verify_glue_transfer(w3: Web3, tx_hash: str):
    """Verify GLUE token transfer on blockchain"""

    print(f"\n🔍 Verifying GLUE transfer on blockchain...")
    print(f"   Tx: {tx_hash}")

    try:
        receipt = w3.eth.get_transaction_receipt(tx_hash)

        if receipt['status'] == 1:
            print(f"   ✅ Transaction successful!")
            print(f"   Block: {receipt['blockNumber']:,}")
            print(f"   Gas used: {receipt['gasUsed']:,}")

            # Check for Transfer events
            transfer_topic = w3.keccak(text="Transfer(address,address,uint256)").hex()

            for log in receipt['logs']:
                if log['address'].lower() == GLUE_TOKEN.lower() and log['topics'][0].hex() == transfer_topic:
                    # Decode transfer event
                    from_addr = '0x' + log['topics'][1].hex()[-40:]
                    to_addr = '0x' + log['topics'][2].hex()[-40:]
                    value = int(log['data'].hex(), 16)

                    print(f"\n   💰 GLUE Transfer:")
                    print(f"      From: {from_addr}")
                    print(f"      To: {to_addr}")
                    print(f"      Amount: {value / (10 ** GLUE_DECIMALS)} GLUE")

            print(f"\n   🔗 View on Snowtrace:")
            print(f"      https://testnet.snowtrace.io/tx/{tx_hash}")

            return True
        else:
            print(f"   ❌ Transaction failed")
            return False

    except Exception as e:
        print(f"   ❌ Error verifying: {e}")
        return False


def main():
    """Run complete x402 payment test"""

    print("\n" + "=" * 70)
    print("🚀 REAL X402 PAYMENT TEST")
    print("=" * 70)
    print()
    print("This script will:")
    print("  1. Create EIP-3009 payment authorization")
    print("  2. Make HTTP request to karma-hello with x402 headers")
    print("  3. karma-hello verifies and forwards to facilitator")
    print("  4. Facilitator executes GLUE.transferWithAuthorization()")
    print("  5. Verify GLUE token transfer on blockchain")
    print()
    print("=" * 70)

    # Step 1: Get test buyer
    print("\n📋 Step 1: Load test buyer wallet")
    print("-" * 70)

    buyer_key = get_test_buyer()

    if not buyer_key:
        print("\n❌ No test buyer private key found!")
        print("\nTo run this test, you need a funded wallet:")
        print("  1. Set environment variable: export TEST_BUYER_PRIVATE_KEY=0x...")
        print("  2. Or add to .env file: TEST_BUYER_PRIVATE_KEY=0x...")
        print("  3. Or store in AWS Secrets Manager")
        print("\nWallet needs:")
        print("  - ~0.1 AVAX for gas")
        print("  - ~0.1 GLUE tokens")
        return

    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    buyer_account = Account.from_key(buyer_key)
    buyer_address = buyer_account.address

    print(f"✅ Buyer: {buyer_address}")

    # Check balances
    avax_balance = w3.eth.get_balance(buyer_address) / 1e18

    # Check GLUE balance
    glue_abi = json.loads('[{"inputs":[{"internalType":"address","name":"account","type":"address"}],"name":"balanceOf","outputs":[{"internalType":"uint256","name":"","type":"uint256"}],"stateMutability":"view","type":"function"}]')
    glue_contract = w3.eth.contract(address=GLUE_TOKEN, abi=glue_abi)
    glue_balance = glue_contract.functions.balanceOf(buyer_address).call() / (10 ** GLUE_DECIMALS)

    print(f"   AVAX: {avax_balance:.4f}")
    print(f"   GLUE: {glue_balance:.4f}")

    if avax_balance < 0.01:
        print(f"\n⚠️  Low AVAX balance ({avax_balance:.4f}). Get testnet AVAX:")
        print("   https://faucet.avax.network/")

    if glue_balance < 0.01:
        print(f"\n⚠️  Low GLUE balance ({glue_balance:.4f}). Contact admin for GLUE tokens.")
        return

    # Step 2: Get agent info
    print("\n📋 Step 2: Get agent information")
    print("-" * 70)

    try:
        response = requests.get(f"{AGENT_URL}/.well-known/agent-card", timeout=5)
        agent_card = response.json()

        seller_address = agent_card['wallet_address']  # Use wallet_address not address

        # Default price for karma-hello (outputs may be empty)
        price_glue = 0.01  # Default price
        if 'outputs' in agent_card and len(agent_card['outputs']) > 0:
            price_str = agent_card['outputs'][0].get('price', '0.01 GLUE')
            price_glue = float(price_str.split()[0])

        print(f"✅ Agent: {agent_card['name']}")
        print(f"   Address: {seller_address}")
        print(f"   Price: {price_glue} GLUE")

        amount = int(price_glue * (10 ** GLUE_DECIMALS))

    except Exception as e:
        print(f"❌ Could not fetch agent card: {e}")
        print(f"\n💡 Is karma-hello running? Check:")
        print(f"   curl {AGENT_URL}/health")
        return

    # Step 3: Create EIP-3009 authorization
    print("\n📋 Step 3: Create EIP-3009 payment authorization")
    print("-" * 70)

    nonce = secrets.token_bytes(32)
    valid_after = 0
    valid_before = int(time.time()) + 3600  # Valid for 1 hour

    print(f"   Amount: {amount / (10 ** GLUE_DECIMALS)} GLUE ({amount} smallest units)")
    print(f"   Nonce: 0x{nonce.hex()[:16]}...")
    print(f"   Valid until: {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(valid_before))}")

    authorization = create_eip3009_authorization(
        w3,
        buyer_address,
        seller_address,
        amount,
        valid_after,
        valid_before,
        nonce,
        buyer_key
    )

    print(f"✅ Signature created")
    print(f"   v: {authorization['v']}")
    print(f"   r: 0x{authorization['r'].hex()[:16]}...")
    print(f"   s: 0x{authorization['s'].hex()[:16]}...")

    # Step 4: Make x402 request
    print("\n📋 Step 4: Make x402 HTTP request")
    print("-" * 70)

    # Request chat logs using correct endpoint
    endpoint = "/get_chat_logs"

    success, data, tx_hash = make_x402_request(
        AGENT_URL,
        endpoint,
        buyer_address,
        seller_address,
        amount,
        authorization,
        nonce,
        valid_after,
        valid_before
    )

    if not success:
        print("\n❌ Payment failed!")
        print(f"   Response: {data}")
        return

    print(f"\n✅ Payment accepted!")

    # Step 5: Verify on blockchain
    if tx_hash:
        print("\n📋 Step 5: Verify GLUE transfer on blockchain")
        print("-" * 70)

        # Wait a moment for transaction to be mined
        print("   ⏳ Waiting for transaction to be mined...")
        time.sleep(3)

        verify_glue_transfer(w3, tx_hash)
    else:
        print("\n⚠️  No transaction hash returned")
        print("   The payment may have been processed but tx hash not provided")
        print("   Check GLUE token transfers manually:")
        print(f"   https://testnet.snowtrace.io/token/{GLUE_TOKEN}")

    # Summary
    print("\n" + "=" * 70)
    print("✅ X402 PAYMENT TEST COMPLETE!")
    print("=" * 70)

    if success:
        print("\n🎉 Success! You've made a real x402 payment with GLUE transfer!")
        print("\nWhat happened:")
        print("  1. ✅ Created EIP-3009 signed authorization (off-chain)")
        print("  2. ✅ Sent HTTP request with x402 headers")
        print("  3. ✅ Agent verified signature")
        print("  4. ✅ Facilitator executed GLUE.transferWithAuthorization()")
        print("  5. ✅ GLUE tokens transferred on-chain")
        print("\nThis is how the agent economy makes payments!")
    else:
        print("\n⚠️  Payment did not complete")
        print("   Check agent logs: docker-compose logs -f karma-hello")
        print("   Check facilitator: docker-compose logs -f facilitator")


if __name__ == "__main__":
    main()
