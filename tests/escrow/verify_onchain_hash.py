#!/usr/bin/env python3
"""
Verify nonce computation against on-chain AuthCaptureEscrow.getHash().

This script calls the actual deployed escrow contract's getHash() function
to see if our computed nonce matches the on-chain result.
"""

from eth_abi import encode
from web3 import Web3
import os

# Base Mainnet
CHAIN_ID = 8453
ESCROW_ADDRESS = "0x320a3c35F131E5D2Fb36af56345726B298936037"
RPC_URL = os.environ.get("RPC_URL_BASE_MAINNET", "https://mainnet.base.org")

ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

# Test payment info - matches what we use in tests
PAYMENT_INFO = {
    "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "payer": ZERO_ADDRESS,  # Zero for payer-agnostic hash
    "receiver": "0xD3868E1eD738CED6945A574a7c769433BeD5d474",
    "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "maxAmount": 10000,
    "preApprovalExpiry": 281474976710655,  # MAX_UINT48
    "authorizationExpiry": 281474976710655,
    "refundExpiry": 281474976710655,
    "minFeeBps": 0,
    "maxFeeBps": 100,
    "feeReceiver": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "salt": 0x1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF,
}

# AuthCaptureEscrow.getHash ABI
ESCROW_ABI = [
    {
        "inputs": [
            {
                "components": [
                    {"name": "operator", "type": "address"},
                    {"name": "payer", "type": "address"},
                    {"name": "receiver", "type": "address"},
                    {"name": "token", "type": "address"},
                    {"name": "maxAmount", "type": "uint120"},
                    {"name": "preApprovalExpiry", "type": "uint48"},
                    {"name": "authorizationExpiry", "type": "uint48"},
                    {"name": "refundExpiry", "type": "uint48"},
                    {"name": "minFeeBps", "type": "uint16"},
                    {"name": "maxFeeBps", "type": "uint16"},
                    {"name": "feeReceiver", "type": "address"},
                    {"name": "salt", "type": "uint256"},
                ],
                "name": "paymentInfo",
                "type": "tuple",
            }
        ],
        "name": "getHash",
        "outputs": [{"type": "bytes32"}],
        "stateMutability": "view",
        "type": "function",
    },
    {
        "inputs": [],
        "name": "PAYMENT_INFO_TYPEHASH",
        "outputs": [{"type": "bytes32"}],
        "stateMutability": "view",
        "type": "function",
    },
]


def compute_sdk_nonce(chain_id, escrow_address, payment_info):
    """Compute nonce as the SDK does - direct tuple encoding."""
    payment_info_tuple = (
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
        int(payment_info["salt"]),
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


def compute_contract_style_hash(chain_id, escrow_address, payment_info, typehash):
    """
    Compute hash as the contract does:
    1. paymentInfoHash = keccak256(abi.encode(TYPEHASH, paymentInfo))
    2. return keccak256(abi.encode(chainId, escrow, paymentInfoHash))
    """
    payment_info_tuple = (
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
        int(payment_info["salt"]),
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


def main():
    print("=== On-Chain Hash Verification ===\n")

    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    if not w3.is_connected():
        print(f"ERROR: Cannot connect to {RPC_URL}")
        return

    print(f"Connected to Base Mainnet (chainId: {w3.eth.chain_id})")

    escrow = w3.eth.contract(
        address=Web3.to_checksum_address(ESCROW_ADDRESS), abi=ESCROW_ABI
    )

    # Get the PAYMENT_INFO_TYPEHASH from the contract
    print("\n1. Fetching PAYMENT_INFO_TYPEHASH from contract...")
    typehash = escrow.functions.PAYMENT_INFO_TYPEHASH().call()
    print(f"   TYPEHASH: 0x{typehash.hex()}")

    # Expected typehash from Solidity:
    # keccak256("PaymentInfo(address operator,address payer,address receiver,address token,uint120 maxAmount,uint48 preApprovalExpiry,uint48 authorizationExpiry,uint48 refundExpiry,uint16 minFeeBps,uint16 maxFeeBps,address feeReceiver,uint256 salt)")
    expected_typehash = Web3.keccak(
        text="PaymentInfo(address operator,address payer,address receiver,address token,uint120 maxAmount,uint48 preApprovalExpiry,uint48 authorizationExpiry,uint48 refundExpiry,uint16 minFeeBps,uint16 maxFeeBps,address feeReceiver,uint256 salt)"
    )
    print(f"   Expected: 0x{expected_typehash.hex()}")
    print(f"   Match: {typehash == expected_typehash}")

    # Call on-chain getHash()
    print("\n2. Calling on-chain escrow.getHash()...")
    payment_info_tuple = (
        Web3.to_checksum_address(PAYMENT_INFO["operator"]),
        Web3.to_checksum_address(PAYMENT_INFO["payer"]),
        Web3.to_checksum_address(PAYMENT_INFO["receiver"]),
        Web3.to_checksum_address(PAYMENT_INFO["token"]),
        int(PAYMENT_INFO["maxAmount"]),
        int(PAYMENT_INFO["preApprovalExpiry"]),
        int(PAYMENT_INFO["authorizationExpiry"]),
        int(PAYMENT_INFO["refundExpiry"]),
        int(PAYMENT_INFO["minFeeBps"]),
        int(PAYMENT_INFO["maxFeeBps"]),
        Web3.to_checksum_address(PAYMENT_INFO["feeReceiver"]),
        int(PAYMENT_INFO["salt"]),
    )

    onchain_hash = escrow.functions.getHash(payment_info_tuple).call()
    print(f"   On-chain hash: 0x{onchain_hash.hex()}")

    # Compute SDK-style nonce
    print("\n3. Computing SDK-style nonce (direct tuple encoding)...")
    sdk_nonce = compute_sdk_nonce(CHAIN_ID, ESCROW_ADDRESS, PAYMENT_INFO)
    print(f"   SDK nonce: {sdk_nonce}")

    # Compute contract-style hash
    print("\n4. Computing contract-style hash (with TYPEHASH)...")
    contract_style = compute_contract_style_hash(
        CHAIN_ID, ESCROW_ADDRESS, PAYMENT_INFO, typehash
    )
    print(f"   Contract-style: {contract_style}")

    # Compare
    print("\n=== COMPARISON ===")
    print(f"On-chain hash:     0x{onchain_hash.hex()}")
    print(f"SDK nonce:         {sdk_nonce}")
    print(f"Contract-style:    {contract_style}")
    print()
    print(f"SDK matches on-chain:      {sdk_nonce == '0x' + onchain_hash.hex()}")
    print(f"Contract-style matches:    {contract_style == '0x' + onchain_hash.hex()}")

    if sdk_nonce != "0x" + onchain_hash.hex():
        print("\n*** SDK NONCE DOES NOT MATCH ON-CHAIN! ***")
        print("This explains why PaymentOperator.authorize() fails!")
        print("The signature's nonce doesn't match what the contract computes.")


if __name__ == "__main__":
    main()
