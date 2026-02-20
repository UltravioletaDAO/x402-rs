#!/usr/bin/env python3
"""Deploy a PaymentOperator via PaymentOperatorFactory.

This script calls deployOperator(config) on the factory contract to deploy
a new PaymentOperator for the facilitator wallet. The deployed operator
address must be registered in src/payment_operator/addresses.rs.

Prerequisites:
  - pip install web3
  - Facilitator wallet private key (with ETH for gas)
  - RPC URL for the target network

Usage:
  python scripts/deploy_operator.py --network base-sepolia
  python scripts/deploy_operator.py --network base-mainnet --rpc-url https://...
  python scripts/deploy_operator.py --network ethereum-sepolia --fee-recipient 0x...

The script will:
1. Connect to the target network
2. Check if an operator with this config already exists
3. Call PaymentOperatorFactory.deployOperator(config) via CREATE2
4. Print the deployed address for registration in addresses.rs
"""

import argparse
import os
import sys

try:
    from web3 import Web3
except ImportError:
    print("Error: web3 not installed. Run: pip install web3")
    sys.exit(1)

ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

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
    "optimism": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
}

# Default RPC URLs (free/public endpoints)
DEFAULT_RPCS = {
    "base-sepolia": "https://sepolia.base.org",
    "base-mainnet": "https://mainnet.base.org",
    "ethereum-sepolia": "https://ethereum-sepolia-rpc.publicnode.com",
    "polygon": "https://polygon-rpc.com",
    "arbitrum": "https://arb1.arbitrum.io/rpc",
    "celo": "https://forno.celo.org",
    "avalanche": "https://api.avax.network/ext/bc/C/rpc",
    "optimism": "https://mainnet.optimism.io",
}

# OperatorConfig struct fields
CONFIG_FIELDS = [
    "feeRecipient",
    "feeCalculator",
    "authorizeCondition",
    "authorizeRecorder",
    "chargeCondition",
    "chargeRecorder",
    "releaseCondition",
    "releaseRecorder",
    "refundInEscrowCondition",
    "refundInEscrowRecorder",
    "refundPostEscrowCondition",
    "refundPostEscrowRecorder",
]

# PaymentOperatorFactory ABI (deployOperator, getOperator, computeAddress)
FACTORY_ABI = [
    {
        "type": "function",
        "name": "deployOperator",
        "inputs": [
            {
                "name": "config",
                "type": "tuple",
                "components": [{"name": f, "type": "address"} for f in CONFIG_FIELDS],
            }
        ],
        "outputs": [{"name": "operator", "type": "address"}],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "getOperator",
        "inputs": [
            {
                "name": "config",
                "type": "tuple",
                "components": [{"name": f, "type": "address"} for f in CONFIG_FIELDS],
            }
        ],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "computeAddress",
        "inputs": [
            {
                "name": "config",
                "type": "tuple",
                "components": [{"name": f, "type": "address"} for f in CONFIG_FIELDS],
            }
        ],
        "outputs": [{"name": "operator", "type": "address"}],
        "stateMutability": "view",
    },
]


def create_config(fee_recipient):
    """Create a permissionless OperatorConfig (all conditions = zero address)."""
    return tuple(
        Web3.to_checksum_address(fee_recipient if i == 0 else ZERO_ADDRESS)
        for i in range(len(CONFIG_FIELDS))
    )


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
    parser.add_argument(
        "--fee-recipient",
        help="Fee recipient address (default: deployer wallet)",
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

    # Build factory contract
    factory = w3.eth.contract(
        address=Web3.to_checksum_address(factory_address), abi=FACTORY_ABI
    )

    # Create config
    fee_recipient = args.fee_recipient or account.address
    config = create_config(fee_recipient)
    print(f"\nConfig:")
    print(f"  feeRecipient: {config[0]}")
    print(f"  All conditions/recorders: ZERO_ADDRESS (permissionless)")

    # Check if already deployed
    existing = factory.functions.getOperator(config).call()
    if existing != ZERO_ADDRESS:
        print(f"\nAlready deployed at: {existing}")
        print(f"\nUpdate src/payment_operator/addresses.rs:")
        print(f'  address!("{existing[2:]}"),')
        return

    # Compute deterministic address
    computed = factory.functions.computeAddress(config).call()
    print(f"  Predicted address: {computed}")

    if args.dry_run:
        print(f"\n[DRY RUN] Would deploy to: {computed}")
        return

    # Estimate gas
    gas_estimate = factory.functions.deployOperator(config).estimate_gas(
        {"from": account.address}
    )
    gas_limit = int(gas_estimate * 1.3)  # 30% buffer
    print(f"\nGas estimate: {gas_estimate} (limit: {gas_limit})")

    # Build transaction with EIP-1559
    gas_price = w3.eth.gas_price
    tx = factory.functions.deployOperator(config).build_transaction(
        {
            "from": account.address,
            "nonce": w3.eth.get_transaction_count(account.address),
            "gas": gas_limit,
            "maxFeePerGas": gas_price * 4,
            "maxPriorityFeePerGas": min(w3.to_wei(1, "gwei"), gas_price),
        }
    )

    # Sign and send
    print("Sending deployOperator() transaction...")
    signed_tx = w3.eth.account.sign_transaction(tx, private_key)
    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f"TX Hash:  0x{tx_hash.hex()}")

    # Wait for receipt
    print("Waiting for confirmation...")
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)

    if receipt.status != 1:
        print(f"Error: transaction reverted (gas used: {receipt.gasUsed})")
        sys.exit(1)

    print(f"Gas used: {receipt.gasUsed}")

    # Verify deployment
    deployed = factory.functions.getOperator(config).call()
    if deployed != ZERO_ADDRESS:
        operator_address = deployed
    else:
        # Fallback: check code at computed address
        code = w3.eth.get_code(Web3.to_checksum_address(computed))
        if len(code) > 2:
            operator_address = computed
        else:
            print("Warning: could not verify deployment")
            print("Check the transaction on block explorer")
            sys.exit(1)

    print(f"\n{'='*60}")
    print(f"PaymentOperator deployed!")
    print(f"Address: {operator_address}")
    print(f"{'='*60}")
    print(f"\nUpdate src/payment_operator/addresses.rs:")
    print(f'  address!("{operator_address[2:]}"),')


if __name__ == "__main__":
    main()
