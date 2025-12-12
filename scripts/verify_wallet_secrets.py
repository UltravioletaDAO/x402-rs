#!/usr/bin/env python3
"""
Verify AWS Secrets Manager contains correct wallet private keys.

This script checks that the EVM wallet secrets properly separate
mainnet and testnet wallets to prevent the critical bug where
testnet transactions used the mainnet wallet.

Usage:
    python scripts/verify_wallet_secrets.py

Expected addresses:
    Mainnet: 0x103040545AC5031A11E8C03dd11324C7333a13C7
    Testnet: 0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8
"""

import subprocess
import json
import sys

def get_secret(secret_name):
    """Get secret from AWS Secrets Manager."""
    try:
        result = subprocess.run([
            'aws', 'secretsmanager', 'get-secret-value',
            '--secret-id', secret_name,
            '--region', 'us-east-2',
            '--query', 'SecretString',
            '--output', 'text'
        ], capture_output=True, text=True, timeout=30)
        if result.returncode == 0:
            data = json.loads(result.stdout)
            return data.get('private_key', '')
    except Exception as e:
        print(f"  Error retrieving secret: {e}")
    return None

def derive_address(private_key):
    """Derive Ethereum address from private key."""
    try:
        from eth_account import Account
        return Account.from_key(private_key).address
    except ImportError:
        return "(eth_account not installed - run: pip install eth-account)"
    except Exception as e:
        return f"(error: {e})"

def main():
    print("=" * 60)
    print("AWS Secrets Manager - Wallet Address Verification")
    print("=" * 60)
    print()

    secrets = [
        ("facilitator-evm-private-key", "EVM_PRIVATE_KEY (legacy/generic)"),
        ("facilitator-evm-mainnet-private-key", "EVM_PRIVATE_KEY_MAINNET"),
        ("facilitator-evm-testnet-private-key", "EVM_PRIVATE_KEY_TESTNET"),
    ]

    results = {}
    for secret_name, env_name in secrets:
        print(f"{env_name}:")
        print(f"  Secret: {secret_name}")
        key = get_secret(secret_name)
        if key:
            addr = derive_address(key)
            print(f"  Derives to: {addr}")
            results[env_name] = addr
        else:
            print("  (could not retrieve)")
            results[env_name] = None
        print()

    print("=" * 60)
    print("Expected Addresses:")
    print("=" * 60)
    print("Mainnet wallet: 0x103040545AC5031A11E8C03dd11324C7333a13C7")
    print("Testnet wallet: 0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8")
    print()

    print("=" * 60)
    print("DIAGNOSIS:")
    print("=" * 60)

    mainnet_expected = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
    testnet_expected = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"

    legacy = results.get("EVM_PRIVATE_KEY (legacy/generic)")
    mainnet = results.get("EVM_PRIVATE_KEY_MAINNET")
    testnet = results.get("EVM_PRIVATE_KEY_TESTNET")

    all_ok = True

    if legacy:
        if legacy == mainnet_expected:
            print("[OK] Legacy key is mainnet wallet")
        elif legacy == testnet_expected:
            print("[WRONG] Legacy key is testnet wallet!")
            all_ok = False
        else:
            print(f"[UNKNOWN] Legacy key: {legacy}")
            all_ok = False

    if mainnet:
        if mainnet == mainnet_expected:
            print("[OK] Mainnet key correctly derives to mainnet wallet")
        else:
            print(f"[WRONG] Mainnet key derives to: {mainnet}")
            all_ok = False

    if testnet:
        if testnet == testnet_expected:
            print("[OK] Testnet key correctly derives to testnet wallet")
        elif testnet == mainnet_expected:
            print("[CRITICAL BUG] Testnet key has MAINNET wallet! This is the bug!")
            all_ok = False
        else:
            print(f"[WRONG] Testnet key derives to: {testnet}")
            all_ok = False

    print()
    if all_ok:
        print("All secrets are correctly configured!")
        return 0
    else:
        print("WARNING: Some secrets may be misconfigured!")
        return 1

if __name__ == "__main__":
    sys.exit(main())
