#!/usr/bin/env python3
"""
Test using tokenCollector as verifyingContract (as x402r-scheme SDK does).

The SDK uses authorization.to (tokenCollector) as verifyingContract, which seems
wrong according to ERC-3009 spec, but let's test it anyway.
"""

import secrets
import time

import boto3
from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3
import requests

NETWORK = {
    "chain_id": 8453,
    "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "escrow": "0x320a3c35F131E5D2Fb36af56345726B298936037",
    "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "token_collector": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
}

MAX_UINT48 = 281474976710655
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"


def get_private_key():
    client = boto3.client("secretsmanager", region_name="us-east-2")
    response = client.get_secret_value(SecretId="lighthouse-buyer-tester")
    return response["SecretString"]


def compute_nonce(chain_id, escrow_address, payment_info):
    salt = payment_info["salt"]
    if isinstance(salt, str):
        salt = int(salt, 16) if salt.startswith("0x") else int(salt)

    payment_info_tuple = (
        Web3.to_checksum_address(payment_info["operator"]),
        ZERO_ADDRESS,
        Web3.to_checksum_address(payment_info["receiver"]),
        Web3.to_checksum_address(payment_info["token"]),
        int(payment_info["maxAmount"]),
        int(payment_info["preApprovalExpiry"]),
        int(payment_info["authorizationExpiry"]),
        int(payment_info["refundExpiry"]),
        int(payment_info["minFeeBps"]),
        int(payment_info["maxFeeBps"]),
        Web3.to_checksum_address(payment_info["feeReceiver"]),
        salt,
    )

    encoded = encode(
        [
            "uint256",
            "address",
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",
        ],
        [chain_id, Web3.to_checksum_address(escrow_address), payment_info_tuple],
    )

    return "0x" + Web3.keccak(encoded).hex()


def sign_with_usdc_domain(private_key, auth, chain_id):
    """Sign using USDC as verifyingContract (correct ERC-3009)"""
    domain = {
        "name": "USD Coin",
        "version": "2",
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(NETWORK["usdc"]),
    }

    types = {
        "TransferWithAuthorization": [
            {"name": "from", "type": "address"},
            {"name": "to", "type": "address"},
            {"name": "value", "type": "uint256"},
            {"name": "validAfter", "type": "uint256"},
            {"name": "validBefore", "type": "uint256"},
            {"name": "nonce", "type": "bytes32"},
        ],
    }

    message = {
        "from": Web3.to_checksum_address(auth["from"]),
        "to": Web3.to_checksum_address(auth["to"]),
        "value": int(auth["value"]),
        "validAfter": int(auth["validAfter"]),
        "validBefore": int(auth["validBefore"]),
        "nonce": auth["nonce"],
    }

    signable = encode_typed_data(domain_data=domain, message_types=types, message_data=message)
    account = Account.from_key(private_key)
    signed = account.sign_message(signable)
    return "0x" + signed.signature.hex()


def sign_with_tokencollector_domain(private_key, auth, chain_id):
    """Sign using tokenCollector as verifyingContract (as SDK does - seems wrong!)"""
    domain = {
        "name": "USD Coin",  # Still use USD Coin name
        "version": "2",
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(NETWORK["token_collector"]),  # TokenCollector!
    }

    types = {
        "TransferWithAuthorization": [
            {"name": "from", "type": "address"},
            {"name": "to", "type": "address"},
            {"name": "value", "type": "uint256"},
            {"name": "validAfter", "type": "uint256"},
            {"name": "validBefore", "type": "uint256"},
            {"name": "nonce", "type": "bytes32"},
        ],
    }

    message = {
        "from": Web3.to_checksum_address(auth["from"]),
        "to": Web3.to_checksum_address(auth["to"]),
        "value": int(auth["value"]),
        "validAfter": int(auth["validAfter"]),
        "validBefore": int(auth["validBefore"]),
        "nonce": auth["nonce"],
    }

    signable = encode_typed_data(domain_data=domain, message_types=types, message_data=message)
    account = Account.from_key(private_key)
    signed = account.sign_message(signable)
    return "0x" + signed.signature.hex()


def build_and_test(private_key, use_tokencollector_domain=False):
    account = Account.from_key(private_key)
    payer = account.address
    receiver = payer

    amount = 10000  # 0.01 USDC
    salt = "0x" + secrets.token_hex(32)
    valid_before = int(time.time()) + 3600

    payment_info = {
        "operator": NETWORK["operator"],
        "receiver": receiver,
        "token": NETWORK["usdc"],
        "maxAmount": str(amount),
        "preApprovalExpiry": MAX_UINT48,
        "authorizationExpiry": MAX_UINT48,
        "refundExpiry": MAX_UINT48,
        "minFeeBps": 0,
        "maxFeeBps": 100,
        "feeReceiver": NETWORK["operator"],
        "salt": salt,
    }

    nonce = compute_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)

    auth = {
        "from": payer,
        "to": NETWORK["token_collector"],
        "value": str(amount),
        "validAfter": "0",
        "validBefore": str(valid_before),
        "nonce": nonce,
    }

    domain_type = "tokenCollector" if use_tokencollector_domain else "USDC"
    print(f"\n=== Testing with {domain_type} as verifyingContract ===")
    print(f"Payer: {payer}")
    print(f"Nonce: {nonce}")

    if use_tokencollector_domain:
        signature = sign_with_tokencollector_domain(private_key, auth, NETWORK["chain_id"])
    else:
        signature = sign_with_usdc_domain(private_key, auth, NETWORK["chain_id"])

    payload = {
        "x402Version": 2,
        "scheme": "escrow",
        "payload": {
            "authorization": auth,
            "signature": signature,
            "paymentInfo": payment_info,
        },
        "paymentRequirements": {
            "scheme": "escrow",
            "network": f"eip155:{NETWORK['chain_id']}",
            "maxAmountRequired": str(amount),
            "asset": NETWORK["usdc"],
            "payTo": receiver,
            "extra": {
                "escrowAddress": NETWORK["escrow"],
                "operatorAddress": NETWORK["operator"],
                "tokenCollector": NETWORK["token_collector"],
            },
        },
    }

    print(f"Sending to facilitator...")
    response = requests.post(
        f"{FACILITATOR_URL}/settle",
        json=payload,
        headers={"Content-Type": "application/json"},
        timeout=60,
    )

    print(f"Status: {response.status_code}")
    result = response.json()

    if result.get("success"):
        print(f"SUCCESS! TX: {result.get('transaction')}")
    else:
        print(f"FAILED: {result.get('errorReason')}")

    return result


def main():
    print("=== Comparing verifyingContract behavior ===")
    private_key = get_private_key()

    # Test 1: Use USDC as verifyingContract (our current approach)
    print("\n" + "=" * 60)
    result1 = build_and_test(private_key, use_tokencollector_domain=False)

    # Test 2: Use tokenCollector as verifyingContract (as SDK does)
    print("\n" + "=" * 60)
    result2 = build_and_test(private_key, use_tokencollector_domain=True)

    print("\n" + "=" * 60)
    print("=== SUMMARY ===")
    print(f"USDC domain: {'SUCCESS' if result1.get('success') else 'FAILED'}")
    print(f"TokenCollector domain: {'SUCCESS' if result2.get('success') else 'FAILED'}")


if __name__ == "__main__":
    main()
