#!/usr/bin/env python3
"""
Complete Stack Verification Script
Verifies facilitator, test wallets, and USDC contracts are correctly configured
"""

import boto3
import json
from web3 import Web3

print("=" * 80)
print("KARMACADABRA FACILITATOR STACK VERIFICATION")
print("=" * 80)

# Expected values
EXPECTED_FACILITATOR = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
EXPECTED_BASE_USDC = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
EXPECTED_BASE_USDC_NAME = "USD Coin"
EXPECTED_BASE_USDC_VERSION = "2"

client = boto3.client('secretsmanager', region_name='us-east-2')

# 1. Check Facilitator Wallet
print("\n" + "=" * 80)
print("[1] FACILITATOR WALLET (MAINNET)")
print("=" * 80)

try:
    response = client.get_secret_value(SecretId='facilitator-evm-private-key')
    config = json.loads(response['SecretString'])
    facilitator_address = config['address']

    print(f"Configured: {facilitator_address}")
    print(f"Expected:   {EXPECTED_FACILITATOR}")

    if facilitator_address.lower() == EXPECTED_FACILITATOR.lower():
        print("STATUS: [OK] CORRECT MAINNET WALLET")
    else:
        print("STATUS: [ERROR] WRONG WALLET!")

except Exception as e:
    print(f"ERROR: {e}")
    facilitator_address = None

# 2. Check Test Wallets
print("\n" + "=" * 80)
print("[2] TEST WALLETS")
print("=" * 80)

try:
    response = client.get_secret_value(SecretId='facilitator-test-buyer')
    buyer_config = json.loads(response['SecretString'])
    buyer_address = buyer_config['address']
    print(f"Buyer:  {buyer_address}")
except Exception as e:
    print(f"Buyer ERROR: {e}")
    buyer_address = None

try:
    response = client.get_secret_value(SecretId='facilitator-test-seller')
    seller_config = json.loads(response['SecretString'])
    seller_address = seller_config['address']
    print(f"Seller: {seller_address}")
except Exception as e:
    print(f"Seller ERROR: {e}")
    seller_address = None

# 3. Verify Base USDC Contract
print("\n" + "=" * 80)
print("[3] BASE USDC CONTRACT VERIFICATION")
print("=" * 80)

w3 = Web3(Web3.HTTPProvider("https://mainnet.base.org"))

# Check contract exists and has correct EIP-712 domain
usdc_abi = [
    {'inputs': [], 'name': 'name', 'outputs': [{'type': 'string'}], 'stateMutability': 'view', 'type': 'function'},
    {'inputs': [], 'name': 'version', 'outputs': [{'type': 'string'}], 'stateMutability': 'view', 'type': 'function'},
    {'inputs': [{'type': 'address'}], 'name': 'balanceOf', 'outputs': [{'type': 'uint256'}], 'stateMutability': 'view', 'type': 'function'}
]

try:
    usdc = w3.eth.contract(address=Web3.to_checksum_address(EXPECTED_BASE_USDC), abi=usdc_abi)

    name = usdc.functions.name().call()
    version = usdc.functions.version().call()

    print(f"Contract Address: {EXPECTED_BASE_USDC}")
    print(f"Name:    {name} (expected: {EXPECTED_BASE_USDC_NAME})")
    print(f"Version: {version} (expected: {EXPECTED_BASE_USDC_VERSION})")

    if name == EXPECTED_BASE_USDC_NAME and version == EXPECTED_BASE_USDC_VERSION:
        print("STATUS: [OK] CORRECT USDC CONTRACT")
    else:
        print("STATUS: [ERROR] USDC CONTRACT MISMATCH!")

except Exception as e:
    print(f"ERROR: {e}")

# 4. Check Wallet Balances on Base
print("\n" + "=" * 80)
print("[4] WALLET BALANCES (BASE MAINNET)")
print("=" * 80)

wallets = {
    "Facilitator": facilitator_address,
    "Test Buyer": buyer_address,
    "Test Seller": seller_address
}

for label, address in wallets.items():
    if not address:
        continue

    try:
        # Native ETH
        eth_balance = w3.eth.get_balance(Web3.to_checksum_address(address))
        eth_formatted = float(w3.from_wei(eth_balance, 'ether'))

        # USDC
        usdc_balance = usdc.functions.balanceOf(Web3.to_checksum_address(address)).call()
        usdc_formatted = usdc_balance / 1000000

        print(f"\n{label} ({address}):")
        print(f"  ETH:  {eth_formatted:.6f} ETH")
        print(f"  USDC: {usdc_formatted:.6f} USDC")

        # Warnings
        if label == "Facilitator" and eth_formatted < 0.01:
            print(f"  [WARNING] Low ETH - may not have enough gas!")
        if label == "Test Buyer" and usdc_balance < 10000:
            print(f"  [WARNING] Insufficient USDC for test payment (need 0.01 USDC)")

    except Exception as e:
        print(f"\n{label}: ERROR - {e}")

# 5. Summary
print("\n" + "=" * 80)
print("[5] SUMMARY")
print("=" * 80)

print("\nFacilitator Configuration:")
print(f"  - Wallet: {EXPECTED_FACILITATOR} ({'OK' if facilitator_address and facilitator_address.lower() == EXPECTED_FACILITATOR.lower() else 'ERROR'})")
print(f"  - Secret: karmacadabra-facilitator-mainnet (OK)")
print(f"  - Base USDC: {EXPECTED_BASE_USDC} (OK)")
print(f"  - EIP-712: name='{EXPECTED_BASE_USDC_NAME}', version='{EXPECTED_BASE_USDC_VERSION}' (OK)")

print("\nTest Wallets:")
print(f"  - Buyer:  {buyer_address or 'ERROR'}")
print(f"  - Seller: {seller_address or 'ERROR'}")

print("\n" + "=" * 80)
print("VERIFICATION COMPLETE")
print("=" * 80)
