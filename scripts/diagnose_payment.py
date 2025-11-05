#!/usr/bin/env python3
"""
Complete USDC Payment Diagnostic Script
Verifies all prerequisites before attempting payment on Base mainnet
"""

import os
import time
from web3 import Web3
from eth_account import Account
from eth_account.messages import encode_typed_data

print("=" * 80)
print("USDC PAYMENT DIAGNOSTIC - Base Mainnet")
print("=" * 80)

# Configuration
BASE_RPC = "https://mainnet.base.org"
USDC_BASE_ADDRESS = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
BUYER_ADDRESS = "0x6bdc03ae4BBAb31843dDDaAE749149aE675ea011"  # Test buyer from load_test.py
SELLER_ADDRESS = "0x4dFB1Cd42604194e79eDaCff4e0d28A576e40d19"  # Test seller
FACILITATOR_ADDRESS = "0x103040545AC5031A11E8C03dd11324C7333a13C7"  # From successful txs
PRICE_USDC = 10000  # 0.01 USDC (6 decimals)

# Connect to Base
w3 = Web3(Web3.HTTPProvider(BASE_RPC))

print(f"\n1. RPC Connection")
print(f"   Connected: {w3.is_connected()}")
print(f"   Latest Block: {w3.eth.block_number}")
print(f"   Chain ID: {w3.eth.chain_id}")

# USDC Contract ABI (minimal)
USDC_ABI = [
    {
        "constant": True,
        "inputs": [{"name": "account", "type": "address"}],
        "name": "balanceOf",
        "outputs": [{"name": "", "type": "uint256"}],
        "type": "function"
    },
    {
        "constant": True,
        "inputs": [],
        "name": "name",
        "outputs": [{"name": "", "type": "string"}],
        "type": "function"
    },
    {
        "constant": True,
        "inputs": [],
        "name": "version",
        "outputs": [{"name": "", "type": "string"}],
        "type": "function"
    },
    {
        "constant": True,
        "inputs": [],
        "name": "DOMAIN_SEPARATOR",
        "outputs": [{"name": "", "type": "bytes32"}],
        "type": "function"
    },
    {
        "constant": True,
        "inputs": [{"name": "authorizer", "type": "address"}, {"name": "nonce", "type": "bytes32"}],
        "name": "authorizationState",
        "outputs": [{"name": "", "type": "bool"}],
        "type": "function"
    },
    {
        "constant": True,
        "inputs": [{"name": "account", "type": "address"}],
        "name": "isBlacklisted",
        "outputs": [{"name": "", "type": "bool"}],
        "type": "function"
    }
]

usdc = w3.eth.contract(address=Web3.to_checksum_address(USDC_BASE_ADDRESS), abi=USDC_ABI)

print(f"\n2. USDC Contract Info")
print(f"   Address: {USDC_BASE_ADDRESS}")
print(f"   Name: {usdc.functions.name().call()}")
print(f"   Version: {usdc.functions.version().call()}")
domain_separator = usdc.functions.DOMAIN_SEPARATOR().call()
print(f"   Domain Separator: {domain_separator.hex()}")

print(f"\n3. Buyer Account Analysis")
buyer_balance_usdc = usdc.functions.balanceOf(Web3.to_checksum_address(BUYER_ADDRESS)).call()
buyer_balance_eth = w3.eth.get_balance(Web3.to_checksum_address(BUYER_ADDRESS))
buyer_blacklisted = usdc.functions.isBlacklisted(Web3.to_checksum_address(BUYER_ADDRESS)).call()

print(f"   Address: {BUYER_ADDRESS}")
print(f"   ETH Balance: {Web3.from_wei(buyer_balance_eth, 'ether')} ETH")
print(f"   USDC Balance: {buyer_balance_usdc / 1e6} USDC")
print(f"   Blacklisted: {buyer_blacklisted}")

if buyer_balance_usdc < PRICE_USDC:
    print(f"   ❌ INSUFFICIENT USDC! Need {PRICE_USDC / 1e6} USDC, have {buyer_balance_usdc / 1e6} USDC")
    print(f"   Required: {(PRICE_USDC - buyer_balance_usdc) / 1e6} more USDC")
else:
    print(f"   ✅ Sufficient USDC balance")

if buyer_blacklisted:
    print(f"   ❌ BUYER IS BLACKLISTED!")

print(f"\n4. Seller Account Analysis")
seller_balance_usdc = usdc.functions.balanceOf(Web3.to_checksum_address(SELLER_ADDRESS)).call()
seller_blacklisted = usdc.functions.isBlacklisted(Web3.to_checksum_address(SELLER_ADDRESS)).call()

print(f"   Address: {SELLER_ADDRESS}")
print(f"   USDC Balance: {seller_balance_usdc / 1e6} USDC")
print(f"   Blacklisted: {seller_blacklisted}")

if seller_blacklisted:
    print(f"   ❌ SELLER IS BLACKLISTED!")

print(f"\n5. Facilitator Account Analysis")
facilitator_balance_eth = w3.eth.get_balance(Web3.to_checksum_address(FACILITATOR_ADDRESS))
print(f"   Address: {FACILITATOR_ADDRESS}")
print(f"   ETH Balance: {Web3.from_wei(facilitator_balance_eth, 'ether')} ETH")

if facilitator_balance_eth < Web3.to_wei(0.001, 'ether'):
    print(f"   ⚠️ LOW ETH BALANCE! May not have enough gas")
else:
    print(f"   ✅ Sufficient ETH for gas")

print(f"\n6. Test Nonce (Check if already used)")
test_nonce = "0x" + "00" * 32  # Example nonce
nonce_used = usdc.functions.authorizationState(
    Web3.to_checksum_address(BUYER_ADDRESS),
    bytes.fromhex(test_nonce[2:])
).call()
print(f"   Test Nonce: {test_nonce}")
print(f"   Already Used: {nonce_used}")

print(f"\n7. EIP-712 Domain Verification")
# Calculate expected domain separator
from eth_utils import keccak

eip712_domain_typehash = keccak(text="EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
name_hash = keccak(text="USD Coin")
version_hash = keccak(text="2")
chain_id = 8453

calculated_domain_separator = keccak(
    eip712_domain_typehash +
    name_hash +
    version_hash +
    chain_id.to_bytes(32, 'big') +
    bytes.fromhex(USDC_BASE_ADDRESS[2:].rjust(64, '0'))
)

print(f"   On-chain: {domain_separator.hex()}")
print(f"   Calculated: {calculated_domain_separator.hex()}")
print(f"   Match: {domain_separator.hex() == calculated_domain_separator.hex()}")

print(f"\n" + "=" * 80)
print("DIAGNOSIS COMPLETE")
print("=" * 80)

# Summary
issues = []
if buyer_balance_usdc < PRICE_USDC:
    issues.append(f"❌ Buyer needs {(PRICE_USDC - buyer_balance_usdc) / 1e6} more USDC")
if buyer_blacklisted:
    issues.append("❌ Buyer is blacklisted")
if seller_blacklisted:
    issues.append("❌ Seller is blacklisted")
if facilitator_balance_eth < Web3.to_wei(0.001, 'ether'):
    issues.append("⚠️ Facilitator has low ETH balance")

if issues:
    print(f"\n🔴 ISSUES FOUND:")
    for issue in issues:
        print(f"   {issue}")
    print(f"\nFix these issues before attempting payment.")
else:
    print(f"\n✅ ALL PRE-REQUISITES SATISFIED")
    print(f"   Payment should work if signature is correct.")

print(f"\n" + "=" * 80)
