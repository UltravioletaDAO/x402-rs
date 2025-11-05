#!/usr/bin/env python3
"""
Create Testnet Facilitator Secret

Creates karmacadabra-facilitator-testnet secret in AWS Secrets Manager.

Usage:
    # Set environment variable with testnet private key
    export TESTNET_PRIVATE_KEY="0x..."

    # Run script
    python scripts/create_testnet_facilitator_secret.py

Or provide via command line:
    python scripts/create_testnet_facilitator_secret.py <private_key>
"""

import json
import boto3
from eth_account import Account
import sys
import os


REGION = 'us-east-2'
TESTNET_ADDRESS = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"


def create_testnet_secret(private_key: str):
    """Create testnet facilitator secret"""

    # Normalize private key
    if not private_key.startswith('0x'):
        private_key = '0x' + private_key

    # Verify address
    try:
        account = Account.from_key(private_key)
        derived_address = account.address
    except Exception as e:
        print(f"ERROR: Invalid private key: {e}")
        return False

    if derived_address != TESTNET_ADDRESS:
        print(f"ERROR: Private key derives to {derived_address}")
        print(f"Expected: {TESTNET_ADDRESS}")
        return False

    # Create secret
    client = boto3.client('secretsmanager', region_name=REGION)

    secret_value = {
        "private_key": private_key,
        "address": derived_address
    }

    try:
        client.create_secret(
            Name='facilitator-evm-private-key-testnet',
            Description='Facilitator hot wallet for testnet networks (Avalanche Fuji, Base Sepolia)',
            SecretString=json.dumps(secret_value)
        )
        print(f"SUCCESS: Created karmacadabra-facilitator-testnet")
        print(f"  Address: {derived_address}")
        return True

    except client.exceptions.ResourceExistsException:
        print(f"INFO: Secret already exists, updating instead...")
        try:
            client.update_secret(
                SecretId='karmacadabra-facilitator-testnet',
                SecretString=json.dumps(secret_value)
            )
            print(f"SUCCESS: Updated karmacadabra-facilitator-testnet")
            print(f"  Address: {derived_address}")
            return True
        except Exception as e:
            print(f"ERROR updating secret: {e}")
            return False

    except Exception as e:
        print(f"ERROR creating secret: {e}")
        return False


def main():
    print("=" * 80)
    print("  Create Testnet Facilitator Secret")
    print("=" * 80)
    print()

    # Get private key from command line or environment
    testnet_key = None

    if len(sys.argv) > 1:
        testnet_key = sys.argv[1]
        print("Using private key from command line argument")
    elif 'TESTNET_PRIVATE_KEY' in os.environ:
        testnet_key = os.environ['TESTNET_PRIVATE_KEY']
        print("Using private key from TESTNET_PRIVATE_KEY environment variable")
    else:
        print("ERROR: No private key provided!")
        print()
        print("Usage:")
        print("  export TESTNET_PRIVATE_KEY='0x...'")
        print("  python scripts/create_testnet_facilitator_secret.py")
        print()
        print("Or:")
        print("  python scripts/create_testnet_facilitator_secret.py 0x...")
        return

    print(f"Target address: {TESTNET_ADDRESS}")
    print()

    success = create_testnet_secret(testnet_key)

    if success:
        print()
        print("=" * 80)
        print("  DONE!")
        print("=" * 80)
        print()
        print("Both facilitator secrets are now configured:")
        print("  karmacadabra-facilitator-testnet  -> 0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8")
        print("  karmacadabra-facilitator-mainnet  -> 0x103040545AC5031A11E8C03dd11324C7333a13C7")
    else:
        print()
        print("FAILED to create testnet secret")
        sys.exit(1)


if __name__ == "__main__":
    main()
