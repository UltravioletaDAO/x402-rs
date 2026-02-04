#!/usr/bin/env python3
"""
Test escrow scheme with CORRECT nonce computation.

The x402r SDK has a bug - it computes nonce differently from the on-chain contract.
This script uses the correct nonce computation that matches AuthCaptureEscrow.getHash().

Contract getHash() does:
    1. paymentInfoHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfo))
    2. return keccak256(abi.encode(block.chainid, address(this), paymentInfoHash))

SDK incorrectly does:
    keccak256(abi.encode(chainId, escrow, paymentInfo))  // Missing TYPEHASH!
"""

import secrets
import time

import boto3
from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3
import requests

# Base Mainnet configuration
NETWORK = {
    "chain_id": 8453,
    "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "escrow": "0x320a3c35F131E5D2Fb36af56345726B298936037",
    "operator": "0xa06958D93135BEd7e43893897C0d9fA931EF051C",  # Our deployed PaymentOperator
    "token_collector": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
}

MAX_UINT48 = 281474976710655
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"

# PAYMENT_INFO_TYPEHASH from the contract
PAYMENT_INFO_TYPEHASH = bytes.fromhex(
    "ae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591"
)


def get_private_key():
    """Get test wallet private key from AWS Secrets Manager."""
    client = boto3.client("secretsmanager", region_name="us-east-2")
    response = client.get_secret_value(SecretId="lighthouse-buyer-tester")
    return response["SecretString"]


def compute_correct_nonce(chain_id, escrow_address, payment_info, typehash=PAYMENT_INFO_TYPEHASH):
    """
    Compute nonce CORRECTLY as the contract does:
    1. paymentInfoHash = keccak256(abi.encode(TYPEHASH, paymentInfo))
    2. return keccak256(abi.encode(chainId, escrow, paymentInfoHash))
    """
    salt = payment_info["salt"]
    if isinstance(salt, str):
        salt = int(salt, 16) if salt.startswith("0x") else int(salt)

    payment_info_tuple = (
        Web3.to_checksum_address(payment_info["operator"]),
        ZERO_ADDRESS,  # payer = 0 for payer-agnostic hash
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

    # Step 1: keccak256(abi.encode(TYPEHASH, paymentInfo))
    encoded_with_typehash = encode(
        [
            "bytes32",
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",
        ],
        [typehash, payment_info_tuple],
    )
    payment_info_hash = Web3.keccak(encoded_with_typehash)

    # Step 2: keccak256(abi.encode(chainId, escrow, paymentInfoHash))
    final_encoded = encode(
        ["uint256", "address", "bytes32"],
        [chain_id, Web3.to_checksum_address(escrow_address), payment_info_hash],
    )

    return "0x" + Web3.keccak(final_encoded).hex()


def sign_erc3009(private_key, auth, chain_id):
    """
    Sign ReceiveWithAuthorization using USDC as verifyingContract.

    Note: The 'to' address should be the tokenCollector, as that's where
    the USDC.receiveWithAuthorization is called from.
    The TokenCollector calls USDC.receiveWithAuthorization() so we need
    to sign the "ReceiveWithAuthorization" type, NOT "TransferWithAuthorization".
    """
    domain = {
        "name": "USD Coin",
        "version": "2",
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(NETWORK["usdc"]),
    }

    # IMPORTANT: Must use "ReceiveWithAuthorization" type because
    # TokenCollector calls USDC.receiveWithAuthorization(), not transferWithAuthorization()
    types = {
        "ReceiveWithAuthorization": [
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


def test_with_correct_nonce(private_key):
    """Test escrow scheme with correct nonce computation."""
    account = Account.from_key(private_key)
    payer = account.address
    receiver = payer  # Self-payment for testing

    amount = 10000  # 0.01 USDC
    salt = "0x" + secrets.token_hex(32)

    # Use a reasonable preApprovalExpiry (1 hour from now) instead of MAX_UINT48
    # The contract validates: currentTime < preApprovalExpiry
    pre_approval_expiry = int(time.time()) + 3600  # 1 hour from now

    payment_info = {
        "operator": NETWORK["operator"],
        "receiver": receiver,
        "token": NETWORK["usdc"],
        "maxAmount": str(amount),
        "preApprovalExpiry": pre_approval_expiry,
        "authorizationExpiry": MAX_UINT48,
        "refundExpiry": MAX_UINT48,
        "minFeeBps": 0,
        "maxFeeBps": 100,
        "feeReceiver": NETWORK["operator"],  # CRITICAL: Must be PaymentOperator address!
        "salt": salt,
    }

    # Compute the CORRECT nonce (with TYPEHASH)
    nonce = compute_correct_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)

    print("\n=== Testing with CORRECT nonce computation ===")
    print(f"Payer: {payer}")
    print(f"Salt: {salt}")
    print(f"preApprovalExpiry: {pre_approval_expiry}")
    print(f"Nonce (correct): {nonce}")

    # Build authorization - MUST match what ERC3009PaymentCollector uses:
    # IERC3009(token).receiveWithAuthorization({
    #     from: payer,
    #     to: address(this),  // TokenCollector!
    #     value: maxAmount,   // Uses maxAmount, not requested amount!
    #     validAfter: 0,
    #     validBefore: paymentInfo.preApprovalExpiry,  // Uses preApprovalExpiry!
    #     nonce: _getHashPayerAgnostic(paymentInfo),
    #     signature: collectorData
    # });
    auth = {
        "from": payer,
        "to": NETWORK["token_collector"],  # TokenCollector receives the USDC
        "value": str(amount),  # maxAmount from paymentInfo
        "validAfter": "0",
        "validBefore": str(pre_approval_expiry),  # MUST match preApprovalExpiry!
        "nonce": nonce,
    }

    signature = sign_erc3009(private_key, auth, NETWORK["chain_id"])
    print(f"Signature: {signature[:20]}...")

    # Build escrow scheme payload
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

    print(f"\nSending to facilitator...")
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
        if "raw_error" in result:
            print(f"Raw error: {result.get('raw_error')}")

    return result


def main():
    print("=== Escrow Scheme Test with CORRECT Nonce ===")
    print("\nThis test uses the correct nonce computation that matches")
    print("AuthCaptureEscrow.getHash() (with PAYMENT_INFO_TYPEHASH).\n")

    private_key = get_private_key()
    result = test_with_correct_nonce(private_key)

    print("\n=== Summary ===")
    if result.get("success"):
        print("Payment settled successfully with correct nonce!")
    else:
        print(f"Payment failed: {result.get('errorReason')}")


if __name__ == "__main__":
    main()
