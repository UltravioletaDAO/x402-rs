#!/usr/bin/env python3
"""
Compare EIP-712 Domain Separator: Python vs On-chain
This will help identify if there's a mismatch causing signature validation failures
"""

from web3 import Web3
from eth_account.messages import encode_typed_data
from eth_utils import keccak

print("=" * 80)
print("EIP-712 DOMAIN SEPARATOR COMPARISON")
print("=" * 80)

# Configuration
BASE_RPC = "https://mainnet.base.org"
USDC_BASE_ADDRESS = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"

# Connect to Base
w3 = Web3(Web3.HTTPProvider(BASE_RPC))

# USDC Contract ABI (minimal)
USDC_ABI = [
    {
        "constant": True,
        "inputs": [],
        "name": "DOMAIN_SEPARATOR",
        "outputs": [{"name": "", "type": "bytes32"}],
        "type": "function"
    }
]

usdc = w3.eth.contract(address=Web3.to_checksum_address(USDC_BASE_ADDRESS), abi=USDC_ABI)

print("\n1. ON-CHAIN DOMAIN SEPARATOR")
print("-" * 80)
onchain_domain_separator = usdc.functions.DOMAIN_SEPARATOR().call()
print(f"Domain Separator: 0x{onchain_domain_separator.hex()}")

print("\n2. PYTHON CALCULATION (Method 1: Manual)")
print("-" * 80)

# EIP-712 Domain from network.rs lines 139-150
domain_data = {
    "name": "USD Coin",
    "version": "2",
    "chainId": 8453,  # Base mainnet
    "verifyingContract": USDC_BASE_ADDRESS
}

print(f"Domain: {domain_data}")

# Manual calculation
eip712_domain_typehash = keccak(text="EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
name_hash = keccak(text="USD Coin")
version_hash = keccak(text="2")
chain_id = 8453

manual_domain_separator = keccak(
    eip712_domain_typehash +
    name_hash +
    version_hash +
    chain_id.to_bytes(32, 'big') +
    bytes.fromhex(USDC_BASE_ADDRESS[2:].zfill(64))
)

print(f"Manual calculation: 0x{manual_domain_separator.hex()}")
print(f"Matches on-chain: {onchain_domain_separator.hex() == manual_domain_separator.hex()}")

print("\n3. PYTHON CALCULATION (Method 2: eth_account library)")
print("-" * 80)

# Test message (doesn't matter what it is, we just need the domain hash)
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

test_message = {
    "from": "0x0000000000000000000000000000000000000000",
    "to": "0x0000000000000000000000000000000000000000",
    "value": 0,
    "validAfter": 0,
    "validBefore": 0,
    "nonce": "0x" + "00" * 32
}

structured_data = {
    "types": types,
    "primaryType": "TransferWithAuthorization",
    "domain": domain_data,
    "message": test_message
}

encoded = encode_typed_data(full_message=structured_data)

# The encoded message contains the domain separator in its calculation
# Let's extract it by looking at the encoded data
print(f"Encoded message type: {type(encoded)}")
print(f"Encoded domain: {encoded.domain}")

# Calculate what eth_account would use
from eth_account._utils.structured_data.hashing import hash_domain
ethaccount_domain_hash = hash_domain(structured_data)
print(f"eth_account domain hash: 0x{ethaccount_domain_hash.hex()}")
print(f"Matches on-chain: {onchain_domain_separator.hex() == ethaccount_domain_hash.hex()}")

print("\n4. NETWORK.RS CONFIGURATION (from x402-rs)")
print("-" * 80)
print("""
From x402-rs/src/network.rs lines 139-150:

static USDC_BASE: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").into(),
            network: Network::Base,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),  // <-- THIS
            version: "2".into(),       // <-- AND THIS
        }),
    })
});
""")

print("\n5. LOAD_TEST.PY CONFIGURATION")
print("-" * 80)
print("""
From test-seller/load_test.py:

domain = {
    "name": "USD Coin",
    "version": "2",
    "chainId": 8453,
    "verifyingContract": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
}
""")

print("\n" + "=" * 80)
print("ANALYSIS")
print("=" * 80)

if onchain_domain_separator.hex() == manual_domain_separator.hex() == ethaccount_domain_hash.hex():
    print("\nOK - ALL DOMAIN SEPARATORS MATCH!")
    print("   On-chain:    0x" + onchain_domain_separator.hex())
    print("   Manual calc: 0x" + manual_domain_separator.hex())
    print("   eth_account: 0x" + ethaccount_domain_hash.hex())
    print("\n   Domain separator is NOT the issue.")
    print("   The problem must be elsewhere (signature encoding, nonce, timing, etc.)")
else:
    print("\nERROR - DOMAIN SEPARATOR MISMATCH!")
    print(f"   On-chain:    0x{onchain_domain_separator.hex()}")
    print(f"   Manual calc: 0x{manual_domain_separator.hex()}")
    print(f"   eth_account: 0x{ethaccount_domain_hash.hex()}")
    print("\n   THIS IS THE BUG! The facilitator is using wrong domain parameters.")

print("\n" + "=" * 80)
