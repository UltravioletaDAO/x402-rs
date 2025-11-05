#!/usr/bin/env python3
"""
Check Facilitator Configuration and Wallet Balances
Verifies that production facilitator is using correct mainnet wallet
"""

import boto3
import json
from web3 import Web3

print("=" * 80)
print("FACILITATOR CONFIGURATION CHECK")
print("=" * 80)

# Expected mainnet wallet
EXPECTED_MAINNET_WALLET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"

# 1. Check AWS Secrets
print("\n[1] AWS SECRETS MANAGER")
print("-" * 80)

client = boto3.client('secretsmanager', region_name='us-east-2')

try:
    response = client.get_secret_value(SecretId='facilitator-evm-private-key')
    config = json.loads(response['SecretString'])
    facilitator_address = config['address']
    print(f"Facilitator Address: {facilitator_address}")
    print(f"Expected Address:    {EXPECTED_MAINNET_WALLET}")

    if facilitator_address.lower() == EXPECTED_MAINNET_WALLET.lower():
        print("STATUS: [OK] MATCH - Using correct mainnet wallet")
    else:
        print("STATUS: [ERROR] MISMATCH - Wrong wallet configured!")
except Exception as e:
    print(f"ERROR: {e}")
    facilitator_address = None

# 2. Check Wallet Balances on Mainnets
if facilitator_address:
    print("\n[2] FACILITATOR WALLET BALANCES (MAINNETS)")
    print("-" * 80)

    networks = {
        "Base": {
            "rpc": "https://mainnet.base.org",
            "currency": "ETH",
            "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        },
        "Avalanche": {
            "rpc": "https://avalanche-c-chain-rpc.publicnode.com",
            "currency": "AVAX",
            "usdc": "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E"
        },
        "Polygon": {
            "rpc": "https://polygon-rpc.com",
            "currency": "MATIC",
            "usdc": "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"
        },
        "Optimism": {
            "rpc": "https://mainnet.optimism.io",
            "currency": "ETH",
            "usdc": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85"
        }
    }

    usdc_abi = [{
        'inputs': [{'type': 'address'}],
        'name': 'balanceOf',
        'outputs': [{'type': 'uint256'}],
        'stateMutability': 'view',
        'type': 'function'
    }]

    for name, config in networks.items():
        try:
            w3 = Web3(Web3.HTTPProvider(config['rpc']))

            # Native balance
            native_balance = w3.eth.get_balance(Web3.to_checksum_address(facilitator_address))
            native_balance_formatted = w3.from_wei(native_balance, 'ether')

            # USDC balance
            usdc_contract = w3.eth.contract(
                address=Web3.to_checksum_address(config['usdc']),
                abi=usdc_abi
            )
            usdc_balance = usdc_contract.functions.balanceOf(
                Web3.to_checksum_address(facilitator_address)
            ).call()
            usdc_balance_formatted = usdc_balance / 1000000  # 6 decimals

            print(f"\n{name}:")
            print(f"  Native: {float(native_balance_formatted):.6f} {config['currency']}")
            print(f"  USDC:   {usdc_balance_formatted:.6f} USDC")

            # Warnings
            if float(native_balance_formatted) < 0.01:
                print(f"  [WARNING] Low {config['currency']} balance - may not have enough gas!")

        except Exception as e:
            print(f"\n{name}:")
            print(f"  ERROR: {e}")

# 3. Check Test Wallets
print("\n[3] TEST WALLET ADDRESSES")
print("-" * 80)

try:
    response = client.get_secret_value(SecretId='facilitator-test-buyer')
    config = json.loads(response['SecretString'])
    buyer_address = config['address']
    print(f"Test Buyer:  {buyer_address}")
except Exception as e:
    print(f"Test Buyer:  ERROR - {e}")
    buyer_address = None

try:
    response = client.get_secret_value(SecretId='facilitator-test-seller')
    config = json.loads(response['SecretString'])
    seller_address = config['address']
    print(f"Test Seller: {seller_address}")
except Exception as e:
    print(f"Test Seller: ERROR - {e}")
    seller_address = None

# 4. Check Test Wallet Balances on Base
if buyer_address and seller_address:
    print("\n[4] TEST WALLET BALANCES (BASE MAINNET)")
    print("-" * 80)

    w3 = Web3(Web3.HTTPProvider("https://mainnet.base.org"))
    usdc_address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
    usdc_contract = w3.eth.contract(address=Web3.to_checksum_address(usdc_address), abi=usdc_abi)

    for label, address in [("Buyer", buyer_address), ("Seller", seller_address)]:
        try:
            eth_balance = w3.eth.get_balance(Web3.to_checksum_address(address))
            usdc_balance = usdc_contract.functions.balanceOf(Web3.to_checksum_address(address)).call()

            print(f"\n{label} ({address}):")
            print(f"  ETH:  {w3.from_wei(eth_balance, 'ether'):.6f} ETH")
            print(f"  USDC: {usdc_balance / 1000000:.6f} USDC")

            if usdc_balance < 10000:
                print(f"  [WARNING] Insufficient USDC for 0.01 USDC test payment!")
        except Exception as e:
            print(f"\n{label}: ERROR - {e}")

print("\n" + "=" * 80)
print("CHECK COMPLETE")
print("=" * 80)
