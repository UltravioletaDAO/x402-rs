#!/usr/bin/env python3
"""Debug script to simulate authorize call and get detailed error info."""

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
    "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "token_collector": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
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


def compute_nonce(chain_id, escrow_address, payment_info):
    """Compute ERC-3009 nonce."""
    salt = payment_info["salt"]
    if isinstance(salt, str):
        salt = int(salt, 16) if salt.startswith("0x") else int(salt)

    payment_info_tuple = (
        Web3.to_checksum_address(payment_info["operator"]),
        ZERO_ADDRESS,  # payer = 0x0 for nonce
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


def sign_authorization(private_key, auth, chain_id, usdc_address, domain_name, domain_version):
    """Sign ERC-3009 TransferWithAuthorization."""
    domain = {
        "name": domain_name,
        "version": domain_version,
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(usdc_address),
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

    print(f"\n=== EIP-712 Signing ===")
    print(f"Domain: {json.dumps(domain, indent=2)}")
    print(f"Message: {json.dumps({k: str(v) for k, v in message.items()}, indent=2)}")

    signable = encode_typed_data(
        domain_data=domain,
        message_types=types,
        message_data=message,
    )

    account = Account.from_key(private_key)
    signed = account.sign_message(signable)

    sig_hex = signed.signature.hex()
    print(f"Signature: 0x{sig_hex}")
    print(f"v: {signed.v}, r: {hex(signed.r)}, s: {hex(signed.s)}")

    return "0x" + sig_hex


def build_authorize_calldata(payment_info, amount, token_collector, signature):
    """Build the authorize() calldata."""
    from eth_abi import encode

    # PaymentInfo struct
    pi = (
        Web3.to_checksum_address(payment_info["operator"]),
        Web3.to_checksum_address(payment_info["payer"]),
        Web3.to_checksum_address(payment_info["receiver"]),
        Web3.to_checksum_address(payment_info["token"]),
        int(payment_info["maxAmount"]),
        int(payment_info["preApprovalExpiry"]),
        int(payment_info["authorizationExpiry"]),
        int(payment_info["refundExpiry"]),
        int(payment_info["minFeeBps"]),
        int(payment_info["maxFeeBps"]),
        Web3.to_checksum_address(payment_info["feeReceiver"]),
        int(payment_info["salt"], 16) if isinstance(payment_info["salt"], str) else payment_info["salt"],
    )

    # collectorData = abi.encode(signature, "0x")
    sig_bytes = bytes.fromhex(signature[2:] if signature.startswith("0x") else signature)
    collector_data = encode(["bytes", "bytes"], [sig_bytes, b""])

    # authorize(PaymentInfo, uint256, address, bytes)
    params = encode(
        [
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",
            "uint256",
            "address",
            "bytes",
        ],
        [pi, amount, Web3.to_checksum_address(token_collector), collector_data],
    )

    # authorize selector
    selector = Web3.keccak(text="authorize((address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256),uint256,address,bytes)")[:4]

    return selector.hex() + params.hex()


def main():
    print("=== Debug Authorize Call ===\n")

    private_key = get_private_key()
    account = Account.from_key(private_key)
    payer = account.address
    receiver = payer  # Self-payment

    print(f"Payer: {payer}")
    print(f"Receiver: {receiver}")

    # Amount: 0.01 USDC
    amount = 10000

    # Generate salt
    salt = "0x" + secrets.token_hex(32)

    # Build payment info
    payment_info = {
        "operator": NETWORK["operator"],
        "payer": payer,  # Actual payer for contract call
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

    print(f"\n=== Payment Info ===")
    for k, v in payment_info.items():
        print(f"  {k}: {v}")

    # Compute nonce (uses payer=0x0)
    nonce = compute_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)
    print(f"\n=== Nonce ===")
    print(f"Nonce: {nonce}")

    # Build authorization
    valid_before = int(time.time()) + 3600
    auth = {
        "from": payer,
        "to": NETWORK["token_collector"],
        "value": str(amount),
        "validAfter": "0",
        "validBefore": str(valid_before),
        "nonce": nonce,
    }

    print(f"\n=== Authorization ===")
    for k, v in auth.items():
        print(f"  {k}: {v}")

    # Sign
    signature = sign_authorization(
        private_key,
        auth,
        NETWORK["chain_id"],
        NETWORK["usdc"],
        NETWORK["usdc_domain_name"],
        NETWORK["usdc_domain_version"],
    )

    # Build calldata
    calldata = build_authorize_calldata(payment_info, amount, NETWORK["token_collector"], signature)
    print(f"\n=== Calldata ===")
    print(f"Total length: {len(calldata) // 2} bytes")
    print(f"First 100 chars: {calldata[:100]}...")

    # Try eth_call to simulate
    print(f"\n=== Simulating Call ===")
    w3 = Web3(Web3.HTTPProvider(NETWORK["rpc_url"]))

    try:
        result = w3.eth.call({
            "to": Web3.to_checksum_address(NETWORK["operator"]),
            "data": "0x" + calldata,
            "from": account.address,
        })
        print(f"SUCCESS! Result: {result.hex()}")
    except Exception as e:
        print(f"REVERTED: {e}")

        # Try to decode revert reason
        error_str = str(e)
        if "0x" in error_str:
            # Extract hex data from error
            import re
            match = re.search(r'0x[a-fA-F0-9]+', error_str)
            if match:
                hex_data = match.group(0)
                print(f"Revert data: {hex_data}")

                # Try to decode as string
                if len(hex_data) > 10:
                    try:
                        decoded = bytes.fromhex(hex_data[2:]).decode('utf-8', errors='ignore')
                        print(f"Decoded (if string): {decoded}")
                    except:
                        pass


if __name__ == "__main__":
    main()
