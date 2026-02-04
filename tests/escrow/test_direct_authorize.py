#!/usr/bin/env python3
"""
Test escrow authorize() directly on-chain (bypassing facilitator).

This script:
1. Creates a PaymentInfo with our deployed PaymentOperator
2. Computes the correct nonce
3. Signs an ERC-3009 authorization
4. Calls PaymentOperator.authorize() directly
"""

import secrets
import time

import boto3
from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3

# Base Mainnet configuration
NETWORK = {
    "chain_id": 8453,
    "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "escrow": "0x320a3c35F131E5D2Fb36af56345726B298936037",
    "operator": "0xa06958D93135BEd7e43893897C0d9fA931EF051C",  # Our deployed PaymentOperator
    "token_collector": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
}

RPC_URL = "https://mainnet.base.org"
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"
MAX_UINT48 = 281474976710655

PAYMENT_INFO_TYPEHASH = bytes.fromhex(
    "ae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591"
)

# PaymentOperator ABI (authorize function)
OPERATOR_ABI = [
    {
        "type": "function",
        "name": "authorize",
        "inputs": [
            {
                "name": "paymentInfo",
                "type": "tuple",
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
            },
            {"name": "amount", "type": "uint256"},
            {"name": "tokenCollector", "type": "address"},
            {"name": "collectorData", "type": "bytes"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
]


def get_private_key():
    """Get test wallet private key from AWS Secrets Manager."""
    client = boto3.client("secretsmanager", region_name="us-east-2")
    response = client.get_secret_value(SecretId="lighthouse-buyer-tester")
    return response["SecretString"]


def compute_nonce(chain_id, escrow_address, payment_info):
    """Compute nonce as on-chain getHash() does (with TYPEHASH)."""
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
    encoded = encode(
        [
            "bytes32",
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",
        ],
        [PAYMENT_INFO_TYPEHASH, payment_info_tuple],
    )
    payment_info_hash = Web3.keccak(encoded)

    # Step 2: keccak256(abi.encode(chainId, escrow, paymentInfoHash))
    final = encode(
        ["uint256", "address", "bytes32"],
        [chain_id, Web3.to_checksum_address(escrow_address), payment_info_hash],
    )
    return "0x" + Web3.keccak(final).hex()


def sign_erc3009_receive(private_key, payer, to, value, valid_before, nonce, chain_id):
    """
    Sign ReceiveWithAuthorization (ERC-3009).
    The contract uses receiveWithAuthorization, NOT transferWithAuthorization.
    """
    domain = {
        "name": "USD Coin",
        "version": "2",
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(NETWORK["usdc"]),
    }

    # ReceiveWithAuthorization type
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
        "from": Web3.to_checksum_address(payer),
        "to": Web3.to_checksum_address(to),
        "value": int(value),
        "validAfter": 0,
        "validBefore": int(valid_before),
        "nonce": nonce,
    }

    signable = encode_typed_data(
        domain_data=domain, message_types=types, message_data=message
    )
    account = Account.from_key(private_key)
    signed = account.sign_message(signable)
    return signed.signature


def main():
    print("=== Direct Escrow Authorize Test ===\n")

    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    print(f"Connected to Base Mainnet (chainId: {w3.eth.chain_id})")

    # Get test wallet
    private_key = get_private_key()
    account = Account.from_key(private_key)
    payer = account.address

    print(f"\nPayer/Caller: {payer}")

    # Check USDC balance
    usdc = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["usdc"]),
        abi=[
            {
                "type": "function",
                "name": "balanceOf",
                "inputs": [{"name": "account", "type": "address"}],
                "outputs": [{"name": "", "type": "uint256"}],
                "stateMutability": "view",
            }
        ],
    )
    balance = usdc.functions.balanceOf(payer).call()
    print(f"USDC Balance: {balance / 1e6} USDC")

    if balance < 10000:
        print("ERROR: Need at least 0.01 USDC for test")
        return

    # Build PaymentInfo
    amount = 10000  # 0.01 USDC
    salt = "0x" + secrets.token_hex(32)
    pre_approval_expiry = int(time.time()) + 3600  # 1 hour from now

    # PaymentInfo.feeReceiver MUST be the PaymentOperator address itself!
    # This is validated by the operator contract
    fee_receiver = NETWORK["operator"]  # PaymentOperator address

    payment_info = {
        "operator": NETWORK["operator"],
        "payer": payer,
        "receiver": payer,  # Self-payment for testing
        "token": NETWORK["usdc"],
        "maxAmount": amount,
        "preApprovalExpiry": pre_approval_expiry,
        "authorizationExpiry": MAX_UINT48,
        "refundExpiry": MAX_UINT48,
        "minFeeBps": 0,
        "maxFeeBps": 100,
        "feeReceiver": fee_receiver,  # Must match operator's feeRecipient
        "salt": salt,
    }

    print(f"\nPaymentInfo:")
    print(f"  operator: {payment_info['operator']}")
    print(f"  payer: {payment_info['payer']}")
    print(f"  receiver: {payment_info['receiver']}")
    print(f"  maxAmount: {payment_info['maxAmount']} ({amount/1e6} USDC)")
    print(f"  preApprovalExpiry: {payment_info['preApprovalExpiry']}")
    print(f"  salt: {salt}")

    # Compute nonce
    nonce = compute_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)
    print(f"\nComputed nonce: {nonce}")

    # Sign ERC-3009 ReceiveWithAuthorization
    # to = tokenCollector (ERC3009PaymentCollector receives the USDC)
    signature = sign_erc3009_receive(
        private_key,
        payer,
        NETWORK["token_collector"],
        amount,
        pre_approval_expiry,
        nonce,
        NETWORK["chain_id"],
    )
    print(f"Signature: 0x{signature.hex()[:40]}...")

    # Build PaymentOperator.authorize() transaction
    operator = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["operator"]), abi=OPERATOR_ABI
    )

    salt_int = int(salt, 16)
    payment_info_tuple = (
        Web3.to_checksum_address(NETWORK["operator"]),
        Web3.to_checksum_address(payer),
        Web3.to_checksum_address(payer),
        Web3.to_checksum_address(NETWORK["usdc"]),
        amount,
        pre_approval_expiry,
        MAX_UINT48,
        MAX_UINT48,
        0,
        100,
        Web3.to_checksum_address(fee_receiver),  # Must match operator's feeRecipient
        salt_int,
    )

    # Build transaction
    print("\n=== Calling PaymentOperator.authorize() ===")

    try:
        # First, estimate gas
        estimated_gas = operator.functions.authorize(
            payment_info_tuple,
            amount,
            Web3.to_checksum_address(NETWORK["token_collector"]),
            signature,
        ).estimate_gas({"from": payer})
        print(f"Estimated gas: {estimated_gas:,}")
    except Exception as e:
        print(f"Gas estimation failed: {e}")
        print("\nThis might indicate:")
        print("- Signature validation failed")
        print("- Insufficient USDC allowance")
        print("- PaymentInfo validation failed")
        return

    nonce_count = w3.eth.get_transaction_count(payer)
    gas_price = w3.eth.gas_price

    tx = operator.functions.authorize(
        payment_info_tuple,
        amount,
        Web3.to_checksum_address(NETWORK["token_collector"]),
        signature,
    ).build_transaction(
        {
            "from": payer,
            "nonce": nonce_count,
            "gas": int(estimated_gas * 1.3),  # 30% buffer
            "maxFeePerGas": gas_price * 2,
            "maxPriorityFeePerGas": w3.to_wei(0.01, "gwei"),
        }
    )

    # Sign and send
    signed_tx = account.sign_transaction(tx)
    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f"TX hash: {tx_hash.hex()}")
    print("Waiting for confirmation...")

    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
    print(f"\nStatus: {'SUCCESS' if receipt['status'] == 1 else 'FAILED'}")
    print(f"Gas used: {receipt['gasUsed']:,}")
    print(f"Block: {receipt['blockNumber']}")

    if receipt["status"] == 1:
        print("\n=== SUCCESS! Funds are now in escrow ===")
        print(f"TX: https://basescan.org/tx/{tx_hash.hex()}")
    else:
        print("\n=== FAILED ===")
        print(f"Check TX: https://basescan.org/tx/{tx_hash.hex()}")


if __name__ == "__main__":
    main()
