#!/usr/bin/env python3
"""Deploy a PaymentOperator via PaymentOperatorFactory.

This script calls createOperator() on the factory contract to deploy
a new PaymentOperator for the facilitator wallet. The deployed operator
address must be registered in src/payment_operator/addresses.rs.

Prerequisites:
  - pip install web3
  - Facilitator wallet private key (with ETH for gas)
  - RPC URL for the target network

Usage:
  python scripts/deploy_operator.py --network base-sepolia
  python scripts/deploy_operator.py --network base-mainnet --rpc-url https://...

The script will:
1. Connect to the target network
2. Call PaymentOperatorFactory.createOperator()
3. Parse the OperatorCreated event to get the deployed address
4. Print the address for registration in addresses.rs
"""

import argparse
import json
import os
import sys

try:
    from web3 import Web3
except ImportError:
    print("Error: web3 not installed. Run: pip install web3")
    sys.exit(1)

# Factory addresses per network (from addresses.rs)
FACTORY_ADDRESSES = {
    "base-sepolia": "0x97d53e63A9CB97556c00BeFd325AF810c9b267B2",
    "base-mainnet": "0x3D0837fF8Ea36F417261577b9BA568400A840260",
    "ethereum-sepolia": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
    "ethereum-mainnet": "0xed02d3E5167BCc9582D851885A89b050AB816a56",
    "polygon": "0xb33D6502EdBbC47201cd1E53C49d703EC0a660b8",
    "arbitrum": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
    "celo": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
    "monad": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
    "avalanche": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
}

# Default RPC URLs (free/public endpoints)
DEFAULT_RPCS = {
    "base-sepolia": "https://sepolia.base.org",
    "base-mainnet": "https://mainnet.base.org",
    "ethereum-sepolia": "https://rpc.sepolia.org",
    "polygon": "https://polygon-rpc.com",
    "arbitrum": "https://arb1.arbitrum.io/rpc",
    "celo": "https://forno.celo.org",
    "avalanche": "https://api.avax.network/ext/bc/C/rpc",
}

# Minimal ABI for PaymentOperatorFactory.createOperator()
FACTORY_ABI = json.loads("""[
    {
        "type": "function",
        "name": "createOperator",
        "inputs": [],
        "outputs": [
            {"name": "", "type": "address", "internalType": "address"}
        ],
        "stateMutability": "nonpayable"
    },
    {
        "type": "event",
        "name": "OperatorCreated",
        "inputs": [
            {"name": "operator", "type": "address", "indexed": true},
            {"name": "owner", "type": "address", "indexed": true}
        ]
    }
]""")


def main():
    parser = argparse.ArgumentParser(description="Deploy PaymentOperator via factory")
    parser.add_argument(
        "--network",
        required=True,
        choices=list(FACTORY_ADDRESSES.keys()),
        help="Target network",
    )
    parser.add_argument("--rpc-url", help="Override RPC URL")
    parser.add_argument(
        "--private-key",
        help="Facilitator wallet private key (or set EVM_PRIVATE_KEY env var)",
    )
    parser.add_argument("--dry-run", action="store_true", help="Only simulate, don't send tx")
    args = parser.parse_args()

    # Get private key
    private_key = args.private_key or os.environ.get("EVM_PRIVATE_KEY")
    if not private_key:
        print("Error: provide --private-key or set EVM_PRIVATE_KEY env var")
        sys.exit(1)

    # Get RPC URL
    rpc_url = args.rpc_url or DEFAULT_RPCS.get(args.network)
    if not rpc_url:
        print(f"Error: no default RPC for {args.network}, provide --rpc-url")
        sys.exit(1)

    factory_address = FACTORY_ADDRESSES[args.network]

    print(f"Network:  {args.network}")
    print(f"RPC:      {rpc_url}")
    print(f"Factory:  {factory_address}")

    # Connect
    w3 = Web3(Web3.HTTPProvider(rpc_url))
    if not w3.is_connected():
        print("Error: cannot connect to RPC")
        sys.exit(1)

    chain_id = w3.eth.chain_id
    print(f"Chain ID: {chain_id}")

    # Set up account
    account = w3.eth.account.from_key(private_key)
    print(f"Wallet:   {account.address}")

    balance = w3.eth.get_balance(account.address)
    print(f"Balance:  {w3.from_wei(balance, 'ether')} ETH")

    if balance == 0:
        print("Error: wallet has no ETH for gas")
        sys.exit(1)

    # Build transaction
    factory = w3.eth.contract(
        address=Web3.to_checksum_address(factory_address), abi=FACTORY_ABI
    )

    nonce = w3.eth.get_transaction_count(account.address)

    tx = factory.functions.createOperator().build_transaction(
        {
            "from": account.address,
            "nonce": nonce,
            "gas": 500_000,  # createOperator typically uses ~200k gas
            "chainId": chain_id,
        }
    )

    if args.dry_run:
        print("\n[DRY RUN] Transaction built but not sent:")
        print(f"  To:    {tx['to']}")
        print(f"  Gas:   {tx['gas']}")
        print(f"  Nonce: {tx['nonce']}")
        return

    # Sign and send
    print("\nSending createOperator() transaction...")
    signed_tx = w3.eth.account.sign_transaction(tx, private_key)
    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f"TX Hash:  {tx_hash.hex()}")

    # Wait for receipt
    print("Waiting for confirmation...")
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)

    if receipt.status != 1:
        print(f"Error: transaction reverted (status={receipt.status})")
        sys.exit(1)

    print(f"Gas used: {receipt.gasUsed}")

    # Parse OperatorCreated event
    logs = factory.events.OperatorCreated().process_receipt(receipt)
    if logs:
        operator_address = logs[0].args.operator
        print(f"\n{'='*60}")
        print(f"PaymentOperator deployed!")
        print(f"Address: {operator_address}")
        print(f"{'='*60}")
        print(f"\nUpdate src/payment_operator/addresses.rs:")
        print(f'  payment_operator: Some(address!("{operator_address[2:]}")),')
    else:
        print("\nWarning: OperatorCreated event not found in logs")
        print("Check the transaction on block explorer to find the operator address")


if __name__ == "__main__":
    main()
