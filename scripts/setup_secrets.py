#!/usr/bin/env python3
"""
Setup Facilitator Secrets in AWS Secrets Manager

Creates separate secrets for testnet and mainnet facilitator wallets:
- karmacadabra-facilitator-testnet: For Avalanche Fuji and Base Sepolia
- karmacadabra-facilitator-mainnet: For Avalanche Mainnet and Base Mainnet

Usage:
    python scripts/setup_facilitator_secrets.py

This script will:
1. Prompt for testnet private key
2. Prompt for mainnet private key
3. Create/update AWS secrets with proper naming convention
4. Verify the secrets were created correctly
"""

import json
import boto3
from eth_account import Account
import sys


def get_address_from_private_key(private_key: str) -> str:
    """Derive Ethereum address from private key"""
    if not private_key.startswith('0x'):
        private_key = '0x' + private_key
    account = Account.from_key(private_key)
    return account.address


def create_or_update_secret(secret_name: str, private_key: str, description: str):
    """Create or update AWS secret with facilitator private key"""
    client = boto3.client('secretsmanager')

    # Get address for verification
    address = get_address_from_private_key(private_key)

    # Prepare secret value
    secret_value = {
        "private_key": private_key,
        "address": address
    }

    try:
        # Try to update existing secret
        client.update_secret(
            SecretId=secret_name,
            Description=description,
            SecretString=json.dumps(secret_value)
        )
        print(f"✅ Updated secret: {secret_name}")
        print(f"   Address: {address}")

    except client.exceptions.ResourceNotFoundException:
        # Create new secret
        client.create_secret(
            Name=secret_name,
            Description=description,
            SecretString=json.dumps(secret_value)
        )
        print(f"✅ Created secret: {secret_name}")
        print(f"   Address: {address}")


def verify_secret(secret_name: str, expected_address: str):
    """Verify secret was created correctly"""
    client = boto3.client('secretsmanager')

    try:
        response = client.get_secret_value(SecretId=secret_name)
        secret_data = json.loads(response['SecretString'])

        stored_address = secret_data.get('address')

        if stored_address == expected_address:
            print(f"   ✓ Verified: {secret_name} → {stored_address}")
            return True
        else:
            print(f"   ✗ ERROR: Address mismatch!")
            print(f"     Expected: {expected_address}")
            print(f"     Got: {stored_address}")
            return False

    except Exception as e:
        print(f"   ✗ ERROR verifying {secret_name}: {e}")
        return False


def main():
    print("=" * 80)
    print("  Setup Facilitator Secrets in AWS Secrets Manager")
    print("=" * 80)
    print()

    # Expected addresses (from user's specification)
    TESTNET_ADDRESS = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"
    MAINNET_ADDRESS = "0x103040545AC5031A11E8C03dd11324C7333a13C7"

    print("This script will create/update two AWS secrets:")
    print(f"  1. karmacadabra-facilitator-testnet  → {TESTNET_ADDRESS}")
    print(f"  2. karmacadabra-facilitator-mainnet  → {MAINNET_ADDRESS}")
    print()
    print("⚠️  WARNING: This will store private keys in AWS Secrets Manager.")
    print("⚠️  Make sure you're using the correct private keys!")
    print()

    # Confirm before proceeding
    confirm = input("Continue? (yes/no): ").strip().lower()
    if confirm not in ['yes', 'y']:
        print("Aborted.")
        return

    print()
    print("-" * 80)
    print("TESTNET Facilitator (Avalanche Fuji + Base Sepolia)")
    print("-" * 80)
    print()

    testnet_key = input(f"Enter testnet private key (will derive {TESTNET_ADDRESS}): ").strip()

    # Verify address matches
    testnet_addr = get_address_from_private_key(testnet_key)
    if testnet_addr != TESTNET_ADDRESS:
        print(f"❌ ERROR: Private key derives to {testnet_addr}, expected {TESTNET_ADDRESS}")
        print("   Please verify you entered the correct testnet private key.")
        return

    print()
    print("-" * 80)
    print("MAINNET Facilitator (Avalanche Mainnet + Base Mainnet)")
    print("-" * 80)
    print()

    mainnet_key = input(f"Enter mainnet private key (will derive {MAINNET_ADDRESS}): ").strip()

    # Verify address matches
    mainnet_addr = get_address_from_private_key(mainnet_key)
    if mainnet_addr != MAINNET_ADDRESS:
        print(f"❌ ERROR: Private key derives to {mainnet_addr}, expected {MAINNET_ADDRESS}")
        print("   Please verify you entered the correct mainnet private key.")
        return

    print()
    print("-" * 80)
    print("Creating AWS Secrets...")
    print("-" * 80)
    print()

    # Create testnet secret
    create_or_update_secret(
        secret_name="karmacadabra-facilitator-testnet",
        private_key=testnet_key,
        description="Facilitator hot wallet for testnet networks (Avalanche Fuji, Base Sepolia)"
    )

    print()

    # Create mainnet secret
    create_or_update_secret(
        secret_name="karmacadabra-facilitator-mainnet",
        private_key=mainnet_key,
        description="Facilitator hot wallet for mainnet networks (Avalanche Mainnet, Base Mainnet)"
    )

    print()
    print("-" * 80)
    print("Verifying secrets...")
    print("-" * 80)
    print()

    testnet_ok = verify_secret("karmacadabra-facilitator-testnet", TESTNET_ADDRESS)
    mainnet_ok = verify_secret("karmacadabra-facilitator-mainnet", MAINNET_ADDRESS)

    print()

    if testnet_ok and mainnet_ok:
        print("=" * 80)
        print("  ✅ SUCCESS: All secrets created and verified!")
        print("=" * 80)
        print()
        print("Next steps:")
        print("  1. Update terraform/ecs-fargate/main.tf to use network-specific secrets")
        print("  2. Update docker-compose.yml to use network-specific secrets")
        print("  3. Update x402-rs to fetch secrets from AWS based on network")
        print()
        print("Secrets created:")
        print(f"  • karmacadabra-facilitator-testnet  → {TESTNET_ADDRESS}")
        print(f"  • karmacadabra-facilitator-mainnet  → {MAINNET_ADDRESS}")
    else:
        print("=" * 80)
        print("  ❌ FAILURE: Secret verification failed!")
        print("=" * 80)
        sys.exit(1)


if __name__ == "__main__":
    main()
