#!/usr/bin/env python3
"""
Migrate Facilitator Secrets from .env to AWS Secrets Manager

This script helps identify which private key is currently being used
and prompts you to assign it to testnet or mainnet.

Usage:
    python scripts/migrate_facilitator_secrets.py
"""

import os
from pathlib import Path
from eth_account import Account


def load_env_file(env_path: str) -> dict:
    """Load .env file and parse key=value pairs"""
    env_vars = {}

    if not os.path.exists(env_path):
        return env_vars

    with open(env_path, 'r') as f:
        for line in f:
            line = line.strip()
            # Skip comments and empty lines
            if not line or line.startswith('#'):
                continue

            # Parse key=value
            if '=' in line:
                key, value = line.split('=', 1)
                key = key.strip()
                value = value.strip()
                env_vars[key] = value

    return env_vars


def main():
    print("=" * 80)
    print("  Facilitator Secrets Migration Helper")
    print("=" * 80)
    print()

    # Expected addresses
    TESTNET_ADDRESS = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"
    MAINNET_ADDRESS = "0x103040545AC5031A11E8C03dd11324C7333a13C7"

    # Check x402-rs/.env
    env_path = Path(__file__).parent.parent / "x402-rs" / ".env"

    if not env_path.exists():
        print(f"❌ No .env file found at: {env_path}")
        print()
        print("Please create x402-rs/.env or run setup_facilitator_secrets.py directly")
        print("with your private keys.")
        return

    print(f"📄 Reading {env_path}")
    print()

    env_vars = load_env_file(str(env_path))

    # Look for EVM_PRIVATE_KEY
    evm_private_key = env_vars.get('EVM_PRIVATE_KEY', '')

    if not evm_private_key or evm_private_key.startswith('0x0000'):
        print("⚠️  No valid EVM_PRIVATE_KEY found in .env file")
        print()
        print("Current value:", evm_private_key[:20] + "..." if evm_private_key else "(empty)")
        print()
        print("Please either:")
        print("  1. Add your private key to x402-rs/.env as EVM_PRIVATE_KEY=0x...")
        print("  2. Run setup_facilitator_secrets.py and enter keys manually")
        return

    # Derive address
    try:
        if not evm_private_key.startswith('0x'):
            evm_private_key = '0x' + evm_private_key

        account = Account.from_key(evm_private_key)
        current_address = account.address

        print(f"✅ Found EVM_PRIVATE_KEY in .env")
        print(f"   Derives to address: {current_address}")
        print()

        # Check which network this belongs to
        if current_address == TESTNET_ADDRESS:
            print("🔍 This matches TESTNET address!")
            print(f"   {TESTNET_ADDRESS}")
            print()
            print("This key should be stored in: karmacadabra-facilitator-testnet")
            print()
            print("⚠️  You still need to add the MAINNET key!")
            print(f"   Expected mainnet address: {MAINNET_ADDRESS}")

        elif current_address == MAINNET_ADDRESS:
            print("🔍 This matches MAINNET address!")
            print(f"   {MAINNET_ADDRESS}")
            print()
            print("This key should be stored in: karmacadabra-facilitator-mainnet")
            print()
            print("⚠️  You still need to add the TESTNET key!")
            print(f"   Expected testnet address: {TESTNET_ADDRESS}")

        else:
            print("❓ This address doesn't match testnet or mainnet!")
            print()
            print(f"   Current:  {current_address}")
            print(f"   Testnet:  {TESTNET_ADDRESS}")
            print(f"   Mainnet:  {MAINNET_ADDRESS}")
            print()
            print("Please verify which network this key is for, or use different keys.")

        print()
        print("-" * 80)
        print("Next Steps:")
        print("-" * 80)
        print()
        print("1. Run: python scripts/setup_facilitator_secrets.py")
        print("   This will prompt you to enter both testnet and mainnet private keys")
        print()
        print("2. The script will create:")
        print("   • karmacadabra-facilitator-testnet")
        print("   • karmacadabra-facilitator-mainnet")
        print()
        print("3. Then update terraform and docker-compose to use these secrets")

    except Exception as e:
        print(f"❌ Error processing private key: {e}")
        print()
        print("Make sure EVM_PRIVATE_KEY is a valid Ethereum private key")


if __name__ == "__main__":
    main()
