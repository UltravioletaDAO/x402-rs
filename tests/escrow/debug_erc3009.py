#!/usr/bin/env python3
"""Debug ERC-3009 signature by calling USDC directly."""

import json
import secrets
import time

import boto3
from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3

# Network config for Base Mainnet
NETWORK = {
    "chain_id": 8453,
    "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "escrow": "0x320a3c35F131E5D2Fb36af56345726B298936037",
    "rpc_url": "https://mainnet.base.org",
    "usdc_domain_name": "USD Coin",
    "usdc_domain_version": "2",
}

MAX_UINT48 = 281474976710655
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"


def get_private_key():
    """Get private key from AWS Secrets Manager."""
    client = boto3.client("secretsmanager", region_name="us-east-2")
    response = client.get_secret_value(SecretId="lighthouse-buyer-tester")
    return response["SecretString"]


def main():
    print("=== Debug ERC-3009 Signature ===\n")

    private_key = get_private_key()
    account = Account.from_key(private_key)
    payer = account.address

    # Simple test: sign a basic transferWithAuthorization to ourselves
    # with a random nonce

    amount = 10000  # 0.01 USDC
    to_address = account.address  # Send to self
    valid_after = 0
    valid_before = int(time.time()) + 3600
    nonce = "0x" + secrets.token_hex(32)  # Random nonce

    print(f"Payer/From: {payer}")
    print(f"To: {to_address}")
    print(f"Amount: {amount}")
    print(f"ValidBefore: {valid_before}")
    print(f"Nonce: {nonce}")

    # EIP-712 domain
    domain = {
        "name": NETWORK["usdc_domain_name"],
        "version": NETWORK["usdc_domain_version"],
        "chainId": NETWORK["chain_id"],
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
        "from": Web3.to_checksum_address(payer),
        "to": Web3.to_checksum_address(to_address),
        "value": amount,
        "validAfter": valid_after,
        "validBefore": valid_before,
        "nonce": nonce,
    }

    print(f"\n=== EIP-712 Domain ===")
    print(json.dumps(domain, indent=2))

    print(f"\n=== Message ===")
    print(json.dumps({k: str(v) for k, v in message.items()}, indent=2))

    # Sign
    signable = encode_typed_data(
        domain_data=domain,
        message_types=types,
        message_data=message,
    )

    signed = account.sign_message(signable)
    v = signed.v
    r = signed.r
    s = signed.s

    print(f"\n=== Signature ===")
    print(f"v: {v}")
    print(f"r: {hex(r)}")
    print(f"s: {hex(s)}")
    print(f"full sig: 0x{signed.signature.hex()}")

    # Build calldata for transferWithAuthorization
    # function transferWithAuthorization(
    #     address from, address to, uint256 value,
    #     uint256 validAfter, uint256 validBefore, bytes32 nonce,
    #     uint8 v, bytes32 r, bytes32 s
    # )
    selector = Web3.keccak(text="transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,uint8,bytes32,bytes32)")[:4]

    nonce_bytes = bytes.fromhex(nonce[2:])
    r_bytes = r.to_bytes(32, 'big')
    s_bytes = s.to_bytes(32, 'big')

    params = encode(
        ["address", "address", "uint256", "uint256", "uint256", "bytes32", "uint8", "bytes32", "bytes32"],
        [
            Web3.to_checksum_address(payer),
            Web3.to_checksum_address(to_address),
            amount,
            valid_after,
            valid_before,
            nonce_bytes,
            v,
            r_bytes,
            s_bytes,
        ]
    )

    calldata = "0x" + selector.hex() + params.hex()

    print(f"\n=== Calldata ===")
    print(f"Selector: 0x{selector.hex()}")
    print(f"Total length: {len(calldata) // 2} bytes")

    # Simulate call
    print(f"\n=== Simulating transferWithAuthorization ===")
    w3 = Web3(Web3.HTTPProvider(NETWORK["rpc_url"]))

    try:
        result = w3.eth.call({
            "to": Web3.to_checksum_address(NETWORK["usdc"]),
            "data": calldata,
            "from": payer,
        })
        print(f"SUCCESS! Result: {result.hex()}")
    except Exception as e:
        print(f"REVERTED: {e}")

        # Check if this is a nonce issue - try to check authorizationState
        print(f"\n=== Checking if nonce was already used ===")
        try:
            # authorizationState(address authorizer, bytes32 nonce)
            state_selector = Web3.keccak(text="authorizationState(address,bytes32)")[:4]
            state_params = encode(["address", "bytes32"], [payer, nonce_bytes])
            state_calldata = "0x" + state_selector.hex() + state_params.hex()

            state_result = w3.eth.call({
                "to": Web3.to_checksum_address(NETWORK["usdc"]),
                "data": state_calldata,
            })
            print(f"Authorization state: {state_result.hex()}")
            # 0x00 = unused, 0x01 = used
        except Exception as e2:
            print(f"Could not check state: {e2}")

        # Also check USDC balance
        print(f"\n=== Checking USDC balance ===")
        balance_selector = Web3.keccak(text="balanceOf(address)")[:4]
        balance_params = encode(["address"], [payer])
        balance_calldata = "0x" + balance_selector.hex() + balance_params.hex()

        try:
            balance_result = w3.eth.call({
                "to": Web3.to_checksum_address(NETWORK["usdc"]),
                "data": balance_calldata,
            })
            balance = int.from_bytes(balance_result, 'big')
            print(f"USDC balance: {balance / 1e6} USDC")
        except Exception as e3:
            print(f"Could not check balance: {e3}")


if __name__ == "__main__":
    main()
