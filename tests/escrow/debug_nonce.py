#!/usr/bin/env python3
"""Debug nonce computation to verify ABI encoding."""

import secrets
from eth_abi import encode
from web3 import Web3

NETWORK = {
    "chain_id": 8453,
    "escrow": "0x320a3c35F131E5D2Fb36af56345726B298936037",
    "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
}

MAX_UINT48 = 281474976710655
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"


def compute_nonce_and_show_encoding(chain_id, escrow_address, payment_info):
    """Compute nonce and show the encoding for debugging."""

    salt = payment_info["salt"]
    if isinstance(salt, str):
        salt = int(salt, 16) if salt.startswith("0x") else int(salt)

    # PaymentInfo tuple with payer = 0x0
    payment_info_tuple = (
        Web3.to_checksum_address(payment_info["operator"]),
        ZERO_ADDRESS,  # payer = 0x0 for nonce computation
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

    print("=== PaymentInfo Tuple (with zeroPayer) ===")
    fields = ["operator", "payer", "receiver", "token", "maxAmount",
              "preApprovalExpiry", "authorizationExpiry", "refundExpiry",
              "minFeeBps", "maxFeeBps", "feeReceiver", "salt"]
    for i, (name, val) in enumerate(zip(fields, payment_info_tuple)):
        print(f"  {i}: {name} = {val}")

    # ABI encode
    encoded = encode(
        [
            "uint256",  # chainId
            "address",  # escrow
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",  # PaymentInfo
        ],
        [chain_id, Web3.to_checksum_address(escrow_address), payment_info_tuple],
    )

    print(f"\n=== Encoded Data ({len(encoded)} bytes) ===")
    print(f"0x{encoded.hex()}")

    # Show breakdown
    print(f"\n=== Encoding Breakdown ===")
    pos = 0

    # uint256 chainId (32 bytes)
    print(f"chainId (uint256): {encoded[pos:pos+32].hex()}")
    pos += 32

    # address escrow (32 bytes, right-padded)
    print(f"escrow (address): {encoded[pos:pos+32].hex()}")
    pos += 32

    # Now the tuple - for a static tuple, fields are encoded inline
    print(f"\n--- PaymentInfo tuple ---")
    print(f"operator (address): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"payer (address): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"receiver (address): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"token (address): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"maxAmount (uint120): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"preApprovalExpiry (uint48): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"authorizationExpiry (uint48): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"refundExpiry (uint48): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"minFeeBps (uint16): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"maxFeeBps (uint16): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"feeReceiver (address): {encoded[pos:pos+32].hex()}")
    pos += 32
    print(f"salt (uint256): {encoded[pos:pos+32].hex()}")
    pos += 32

    # Compute hash
    nonce = Web3.keccak(encoded)
    print(f"\n=== Final Nonce ===")
    print(f"keccak256: 0x{nonce.hex()}")

    return "0x" + nonce.hex()


def main():
    print("=== Debug Nonce Computation ===\n")

    # Use a fixed salt for reproducibility
    salt = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"

    payment_info = {
        "operator": NETWORK["operator"],
        "receiver": "0xD3868E1eD738CED6945A574a7c769433BeD5d474",  # Some receiver
        "token": NETWORK["usdc"],
        "maxAmount": "10000",
        "preApprovalExpiry": MAX_UINT48,
        "authorizationExpiry": MAX_UINT48,
        "refundExpiry": MAX_UINT48,
        "minFeeBps": 0,
        "maxFeeBps": 100,
        "feeReceiver": NETWORK["operator"],
        "salt": salt,
    }

    nonce = compute_nonce_and_show_encoding(
        NETWORK["chain_id"],
        NETWORK["escrow"],
        payment_info
    )

    print(f"\n=== Summary ===")
    print(f"ChainId: {NETWORK['chain_id']}")
    print(f"Escrow: {NETWORK['escrow']}")
    print(f"Salt: {salt}")
    print(f"Nonce: {nonce}")


if __name__ == "__main__":
    main()
