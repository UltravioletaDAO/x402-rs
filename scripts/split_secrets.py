#!/usr/bin/env python3
"""
Split Facilitator Secret into Testnet and Mainnet

Migrates from single 'karmacadabra-facilitator' to:
- karmacadabra-facilitator-testnet (Avalanche Fuji + Base Sepolia)
- karmacadabra-facilitator-mainnet (Avalanche Mainnet + Base Mainnet)

Usage:
    python scripts/split_facilitator_secrets.py
"""

import json
import boto3
from eth_account import Account
import sys


REGION = 'us-east-2'
TESTNET_ADDRESS = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"
MAINNET_ADDRESS = "0x103040545AC5031A11E8C03dd11324C7333a13C7"


def get_secret(secret_name: str) -> dict:
    """Retrieve secret from AWS Secrets Manager"""
    client = boto3.client('secretsmanager', region_name=REGION)
    try:
        response = client.get_secret_value(SecretId=secret_name)
        return json.loads(response['SecretString'])
    except client.exceptions.ResourceNotFoundException:
        return None
    except Exception as e:
        print(f"Error retrieving {secret_name}: {e}")
        return None


def create_secret(secret_name: str, private_key: str, address: str, description: str):
    """Create new secret in AWS Secrets Manager"""
    client = boto3.client('secretsmanager', region_name=REGION)

    secret_value = {
        "private_key": private_key,
        "address": address
    }

    try:
        client.create_secret(
            Name=secret_name,
            Description=description,
            SecretString=json.dumps(secret_value)
        )
        print(f"  Created: {secret_name} -> {address}")
        return True
    except client.exceptions.ResourceExistsException:
        print(f"  Already exists: {secret_name}")
        return True
    except Exception as e:
        print(f"  ERROR creating {secret_name}: {e}")
        return False


def delete_secret(secret_name: str, force_delete: bool = False):
    """Delete secret from AWS Secrets Manager"""
    client = boto3.client('secretsmanager', region_name=REGION)
    try:
        if force_delete:
            client.delete_secret(SecretId=secret_name, ForceDeleteWithoutRecovery=True)
            print(f"  Deleted: {secret_name} (force, no recovery)")
        else:
            client.delete_secret(SecretId=secret_name, RecoveryWindowInDays=7)
            print(f"  Scheduled for deletion: {secret_name} (7 day recovery window)")
        return True
    except Exception as e:
        print(f"  ERROR deleting {secret_name}: {e}")
        return False


def main():
    print("=" * 80)
    print("  Split Facilitator Secrets: Testnet vs Mainnet")
    print("=" * 80)
    print()

    # Step 1: Check current facilitator secret
    print("[1] Checking existing 'karmacadabra-facilitator' secret...")
    current_secret = get_secret('karmacadabra-facilitator')

    if not current_secret:
        print("    No existing secret found.")
        print("    Will create both testnet and mainnet from scratch.")
        mainnet_private_key = None
    else:
        current_address = current_secret.get('address')
        print(f"    Found: {current_address}")

        if current_address == MAINNET_ADDRESS:
            print("    This is the MAINNET wallet - will copy to mainnet secret")
            mainnet_private_key = current_secret.get('private_key')
        elif current_address == TESTNET_ADDRESS:
            print("    This is the TESTNET wallet - will copy to testnet secret")
            testnet_private_key = current_secret.get('private_key')
            mainnet_private_key = None
        else:
            print(f"    WARNING: Address doesn't match expected testnet or mainnet!")
            print(f"    Expected testnet:  {TESTNET_ADDRESS}")
            print(f"    Expected mainnet:  {MAINNET_ADDRESS}")
            print(f"    Got:               {current_address}")
            mainnet_private_key = None

    print()

    # Step 2: Check if new secrets already exist
    print("[2] Checking if new secrets already exist...")
    testnet_exists = get_secret('karmacadabra-facilitator-testnet')
    mainnet_exists = get_secret('karmacadabra-facilitator-mainnet')

    if testnet_exists:
        print(f"    karmacadabra-facilitator-testnet exists: {testnet_exists.get('address')}")
    if mainnet_exists:
        print(f"    karmacadabra-facilitator-mainnet exists: {mainnet_exists.get('address')}")

    print()

    # Step 3: Create mainnet secret
    print("[3] Setting up MAINNET secret...")

    if mainnet_exists:
        print(f"    Already exists: {mainnet_exists.get('address')}")
    elif mainnet_private_key:
        print(f"    Copying from existing secret...")
        create_secret(
            'karmacadabra-facilitator-mainnet',
            mainnet_private_key,
            MAINNET_ADDRESS,
            'Facilitator hot wallet for mainnet networks (Avalanche Mainnet, Base Mainnet)'
        )
    else:
        print(f"    No mainnet private key available.")
        print(f"    Please enter mainnet private key (derives to {MAINNET_ADDRESS}):")
        mainnet_key = input("    Private key: ").strip()

        # Verify
        if not mainnet_key.startswith('0x'):
            mainnet_key = '0x' + mainnet_key
        account = Account.from_key(mainnet_key)
        if account.address != MAINNET_ADDRESS:
            print(f"    ERROR: Key derives to {account.address}, expected {MAINNET_ADDRESS}")
            return

        create_secret(
            'karmacadabra-facilitator-mainnet',
            mainnet_key,
            MAINNET_ADDRESS,
            'Facilitator hot wallet for mainnet networks (Avalanche Mainnet, Base Mainnet)'
        )

    print()

    # Step 4: Create testnet secret
    print("[4] Setting up TESTNET secret...")

    if testnet_exists:
        print(f"    Already exists: {testnet_exists.get('address')}")
    else:
        print(f"    Please enter testnet private key (derives to {TESTNET_ADDRESS}):")
        testnet_key = input("    Private key: ").strip()

        # Verify
        if not testnet_key.startswith('0x'):
            testnet_key = '0x' + testnet_key
        account = Account.from_key(testnet_key)
        if account.address != TESTNET_ADDRESS:
            print(f"    ERROR: Key derives to {account.address}, expected {TESTNET_ADDRESS}")
            return

        create_secret(
            'karmacadabra-facilitator-testnet',
            testnet_key,
            TESTNET_ADDRESS,
            'Facilitator hot wallet for testnet networks (Avalanche Fuji, Base Sepolia)'
        )

    print()

    # Step 5: Verify both secrets exist
    print("[5] Verifying new secrets...")
    testnet_check = get_secret('karmacadabra-facilitator-testnet')
    mainnet_check = get_secret('karmacadabra-facilitator-mainnet')

    testnet_ok = testnet_check and testnet_check.get('address') == TESTNET_ADDRESS
    mainnet_ok = mainnet_check and mainnet_check.get('address') == MAINNET_ADDRESS

    if testnet_ok:
        print(f"    Testnet OK: {TESTNET_ADDRESS}")
    else:
        print(f"    Testnet FAILED!")

    if mainnet_ok:
        print(f"    Mainnet OK: {MAINNET_ADDRESS}")
    else:
        print(f"    Mainnet FAILED!")

    print()

    if not (testnet_ok and mainnet_ok):
        print("=" * 80)
        print("  FAILED: Could not verify both secrets!")
        print("=" * 80)
        return

    # Step 6: Clean up old secret
    print("[6] Cleaning up old 'karmacadabra-facilitator' secret...")

    if current_secret:
        print("    The old secret will be scheduled for deletion (7 day recovery).")
        confirm = input("    Delete old secret? (yes/no): ").strip().lower()

        if confirm in ['yes', 'y']:
            delete_secret('karmacadabra-facilitator')
        else:
            print("    Skipped deletion. You can delete manually later.")
    else:
        print("    No old secret to clean up.")

    print()
    print("=" * 80)
    print("  SUCCESS: Facilitator secrets split successfully!")
    print("=" * 80)
    print()
    print("Created secrets:")
    print(f"  karmacadabra-facilitator-testnet  -> {TESTNET_ADDRESS}")
    print(f"  karmacadabra-facilitator-mainnet  -> {MAINNET_ADDRESS}")
    print()
    print("Next steps:")
    print("  1. Update terraform/ecs-fargate/main.tf to use network-specific secrets")
    print("  2. Update docker-compose.yml to use network-specific secrets")
    print("  3. Test with: python tests/test_facilitator.py")


if __name__ == "__main__":
    main()
