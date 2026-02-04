#!/usr/bin/env python3
"""
Deploy a simple PaymentOperator for testing the escrow scheme.

This script:
1. Computes the deterministic address for a PaymentOperator config
2. Deploys the PaymentOperator via the factory (if not already deployed)
3. Returns the deployed address to use as paymentInfo.operator
"""

import os
import json
import boto3
from web3 import Web3
from eth_account import Account

# Base Mainnet
CHAIN_ID = 8453
RPC_URL = os.environ.get("RPC_URL_BASE_MAINNET", "https://mainnet.base.org")

# Contract addresses from x402r-sdk
FACTORY = "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838"  # PaymentOperatorFactory

# Our fee recipient (use facilitator wallet or any address)
# For testing, we'll just use the test wallet
FEE_RECIPIENT = "0xD3868E1eD738CED6945A574a7c769433BeD5d474"  # Test wallet

ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

# PaymentOperatorFactory ABI (minimal)
FACTORY_ABI = [
    {
        "type": "function",
        "name": "computeAddress",
        "inputs": [
            {
                "name": "config",
                "type": "tuple",
                "components": [
                    {"name": "feeRecipient", "type": "address"},
                    {"name": "feeCalculator", "type": "address"},
                    {"name": "authorizeCondition", "type": "address"},
                    {"name": "authorizeRecorder", "type": "address"},
                    {"name": "chargeCondition", "type": "address"},
                    {"name": "chargeRecorder", "type": "address"},
                    {"name": "releaseCondition", "type": "address"},
                    {"name": "releaseRecorder", "type": "address"},
                    {"name": "refundInEscrowCondition", "type": "address"},
                    {"name": "refundInEscrowRecorder", "type": "address"},
                    {"name": "refundPostEscrowCondition", "type": "address"},
                    {"name": "refundPostEscrowRecorder", "type": "address"},
                ],
            },
        ],
        "outputs": [{"name": "operator", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getOperator",
        "inputs": [
            {
                "name": "config",
                "type": "tuple",
                "components": [
                    {"name": "feeRecipient", "type": "address"},
                    {"name": "feeCalculator", "type": "address"},
                    {"name": "authorizeCondition", "type": "address"},
                    {"name": "authorizeRecorder", "type": "address"},
                    {"name": "chargeCondition", "type": "address"},
                    {"name": "chargeRecorder", "type": "address"},
                    {"name": "releaseCondition", "type": "address"},
                    {"name": "releaseRecorder", "type": "address"},
                    {"name": "refundInEscrowCondition", "type": "address"},
                    {"name": "refundInEscrowRecorder", "type": "address"},
                    {"name": "refundPostEscrowCondition", "type": "address"},
                    {"name": "refundPostEscrowRecorder", "type": "address"},
                ],
            },
        ],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "deployOperator",
        "inputs": [
            {
                "name": "config",
                "type": "tuple",
                "components": [
                    {"name": "feeRecipient", "type": "address"},
                    {"name": "feeCalculator", "type": "address"},
                    {"name": "authorizeCondition", "type": "address"},
                    {"name": "authorizeRecorder", "type": "address"},
                    {"name": "chargeCondition", "type": "address"},
                    {"name": "chargeRecorder", "type": "address"},
                    {"name": "releaseCondition", "type": "address"},
                    {"name": "releaseRecorder", "type": "address"},
                    {"name": "refundInEscrowCondition", "type": "address"},
                    {"name": "refundInEscrowRecorder", "type": "address"},
                    {"name": "refundPostEscrowCondition", "type": "address"},
                    {"name": "refundPostEscrowRecorder", "type": "address"},
                ],
            },
        ],
        "outputs": [{"name": "operator", "type": "address"}],
        "stateMutability": "nonpayable",
    },
]


def get_private_key():
    """Get test wallet private key from AWS Secrets Manager."""
    client = boto3.client("secretsmanager", region_name="us-east-2")
    response = client.get_secret_value(SecretId="lighthouse-buyer-tester")
    return response["SecretString"]


def create_simple_config(fee_recipient):
    """
    Create a simple PaymentOperator config with all conditions set to ZERO_ADDRESS.

    This creates a fully permissionless operator:
    - Anyone can call authorize, charge, release, refund
    - No fee calculator (no operator fees)
    - No recorders (no state tracking beyond escrow)
    """
    return (
        Web3.to_checksum_address(fee_recipient),  # feeRecipient
        Web3.to_checksum_address(ZERO_ADDRESS),   # feeCalculator (no fees)
        Web3.to_checksum_address(ZERO_ADDRESS),   # authorizeCondition (anyone)
        Web3.to_checksum_address(ZERO_ADDRESS),   # authorizeRecorder (none)
        Web3.to_checksum_address(ZERO_ADDRESS),   # chargeCondition (anyone)
        Web3.to_checksum_address(ZERO_ADDRESS),   # chargeRecorder (none)
        Web3.to_checksum_address(ZERO_ADDRESS),   # releaseCondition (anyone)
        Web3.to_checksum_address(ZERO_ADDRESS),   # releaseRecorder (none)
        Web3.to_checksum_address(ZERO_ADDRESS),   # refundInEscrowCondition (anyone)
        Web3.to_checksum_address(ZERO_ADDRESS),   # refundInEscrowRecorder (none)
        Web3.to_checksum_address(ZERO_ADDRESS),   # refundPostEscrowCondition (anyone)
        Web3.to_checksum_address(ZERO_ADDRESS),   # refundPostEscrowRecorder (none)
    )


def main():
    print("=== PaymentOperator Deployment ===\n")

    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    if not w3.is_connected():
        print(f"ERROR: Cannot connect to {RPC_URL}")
        return

    print(f"Connected to Base Mainnet (chainId: {w3.eth.chain_id})")

    factory = w3.eth.contract(
        address=Web3.to_checksum_address(FACTORY),
        abi=FACTORY_ABI
    )

    # Create config
    config = create_simple_config(FEE_RECIPIENT)
    print(f"\nConfig:")
    print(f"  feeRecipient: {config[0]}")
    print(f"  All conditions/recorders: ZERO_ADDRESS (permissionless)")

    # Compute deterministic address
    print("\n1. Computing deterministic address...")
    computed_address = factory.functions.computeAddress(config).call()
    print(f"   Computed address: {computed_address}")

    # Check if already deployed
    print("\n2. Checking if already deployed...")
    deployed_address = factory.functions.getOperator(config).call()

    if deployed_address != ZERO_ADDRESS:
        print(f"   Already deployed at: {deployed_address}")
        print(f"\n=== Use this as paymentInfo.operator: {deployed_address} ===")
        return deployed_address

    print("   Not yet deployed")

    # Deploy
    print("\n3. Deploying PaymentOperator...")
    private_key = get_private_key()
    account = Account.from_key(private_key)

    # Check balance
    balance = w3.eth.get_balance(account.address)
    print(f"   Deployer: {account.address}")
    print(f"   ETH balance: {w3.from_wei(balance, 'ether')} ETH")

    if balance < w3.to_wei(0.001, 'ether'):
        print("   ERROR: Insufficient ETH for gas")
        return None

    # Build transaction - use higher gas limit for contract deployment
    tx = factory.functions.deployOperator(config).build_transaction({
        'from': account.address,
        'nonce': w3.eth.get_transaction_count(account.address),
        'gas': 1500000,  # Increased for contract deployment
        'maxFeePerGas': w3.eth.gas_price * 2,
        'maxPriorityFeePerGas': w3.to_wei(0.001, 'gwei'),
    })

    # Sign and send
    signed_tx = account.sign_transaction(tx)
    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f"   TX hash: {tx_hash.hex()}")

    # Wait for receipt
    print("   Waiting for confirmation...")
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash)

    if receipt['status'] == 1:
        # Get deployed address
        deployed_address = factory.functions.getOperator(config).call()
        print(f"   SUCCESS! Deployed at: {deployed_address}")
        print(f"\n=== Use this as paymentInfo.operator: {deployed_address} ===")
        return deployed_address
    else:
        print(f"   FAILED! TX reverted")
        return None


if __name__ == "__main__":
    main()
