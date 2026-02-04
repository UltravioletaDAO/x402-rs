#!/usr/bin/env python3
"""Debug script to verify payment info encoding matches contract."""

import sys
import time
import secrets
from eth_account import Account
from web3 import Web3

sys.path.insert(0, ".")
from test_escrow_with_correct_nonce import (
    NETWORK, compute_correct_nonce, get_private_key
)

# AuthCaptureEscrow ABI with getHash
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
        "outputs": [{"name": "", "type": "bytes32"}],
        "stateMutability": "view",
        "type": "function",
    },
]


def main():
    """Compare local nonce computation with contract getHash."""
    private_key = get_private_key()
    account = Account.from_key(private_key)
    payer = account.address
    receiver = payer

    w3 = Web3(Web3.HTTPProvider("https://mainnet.base.org"))

    amount = 10000
    salt = "0x" + secrets.token_hex(32)

    now = int(time.time())
    pre_approval_expiry = now + 3600
    authorization_expiry = now + 86400
    refund_expiry = now + 604800

    # Local payment_info (for nonce computation)
    payment_info = {
        "operator": NETWORK["operator"],
        "receiver": receiver,
        "token": NETWORK["usdc"],
        "maxAmount": str(amount),
        "preApprovalExpiry": pre_approval_expiry,
        "authorizationExpiry": authorization_expiry,
        "refundExpiry": refund_expiry,
        "minFeeBps": 0,
        "maxFeeBps": 100,
        "feeReceiver": NETWORK["operator"],
        "salt": salt,
    }

    # Compute nonce locally
    local_nonce = compute_correct_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)
    print(f"Local nonce: {local_nonce}")

    # Build tuple for contract call (WITH payer)
    payment_info_tuple = (
        Web3.to_checksum_address(NETWORK["operator"]),
        Web3.to_checksum_address(payer),
        Web3.to_checksum_address(receiver),
        Web3.to_checksum_address(NETWORK["usdc"]),
        amount,
        pre_approval_expiry,
        authorization_expiry,
        refund_expiry,
        0,
        100,
        Web3.to_checksum_address(NETWORK["operator"]),
        int(salt, 16),
    )

    print(f"\nPayment Info Tuple:")
    print(f"  operator: {payment_info_tuple[0]}")
    print(f"  payer: {payment_info_tuple[1]}")
    print(f"  receiver: {payment_info_tuple[2]}")
    print(f"  token: {payment_info_tuple[3]}")
    print(f"  maxAmount: {payment_info_tuple[4]}")
    print(f"  preApprovalExpiry: {payment_info_tuple[5]}")
    print(f"  authorizationExpiry: {payment_info_tuple[6]}")
    print(f"  refundExpiry: {payment_info_tuple[7]}")
    print(f"  minFeeBps: {payment_info_tuple[8]}")
    print(f"  maxFeeBps: {payment_info_tuple[9]}")
    print(f"  feeReceiver: {payment_info_tuple[10]}")
    print(f"  salt: {hex(payment_info_tuple[11])}")

    # Call contract getHash
    escrow_contract = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["escrow"]),
        abi=ESCROW_ABI
    )

    try:
        contract_hash = escrow_contract.functions.getHash(payment_info_tuple).call()
        print(f"\nContract getHash: 0x{contract_hash.hex()}")

        # The nonce is computed with payer=0, so let's also try that
        payment_info_tuple_payer_zero = (
            Web3.to_checksum_address(NETWORK["operator"]),
            "0x0000000000000000000000000000000000000000",  # payer = 0
            Web3.to_checksum_address(receiver),
            Web3.to_checksum_address(NETWORK["usdc"]),
            amount,
            pre_approval_expiry,
            authorization_expiry,
            refund_expiry,
            0,
            100,
            Web3.to_checksum_address(NETWORK["operator"]),
            int(salt, 16),
        )

        contract_hash_payer_zero = escrow_contract.functions.getHash(payment_info_tuple_payer_zero).call()
        print(f"Contract getHash (payer=0): 0x{contract_hash_payer_zero.hex()}")

        print(f"\nLocal nonce (should match payer=0): {local_nonce}")

        if f"0x{contract_hash_payer_zero.hex()}" == local_nonce:
            print("\n[OK] Nonce computation matches contract!")
        else:
            print("\n[MISMATCH] Nonce computation doesn't match contract!")

    except Exception as e:
        print(f"\nError calling contract: {e}")


if __name__ == "__main__":
    main()
