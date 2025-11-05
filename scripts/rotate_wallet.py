#!/usr/bin/env python3
"""
Facilitator Wallet Rotation Script
====================================

SECURITY: Rotate the x402 facilitator hot wallet with zero downtime

This script performs a complete facilitator wallet rotation:
1. Generates a new wallet for the facilitator
2. Displays the new address for manual funding
3. Updates AWS Secrets Manager with the new private key
4. Forces ECS redeployment to use the new wallet

WARNING: The private key is NEVER displayed in output!
Only the public address is shown for funding purposes.

Usage:
    # Step 1: Generate new wallet and show address for funding
    python scripts/rotate-facilitator-wallet.py --generate

    # Step 2: After funding, update AWS and redeploy
    python scripts/rotate-facilitator-wallet.py --deploy

    # Full rotation in one step (auto-generates and deploys)
    python scripts/rotate-facilitator-wallet.py --full
"""

import os
import sys
import json
import subprocess
from pathlib import Path
from web3 import Web3
from eth_account import Account
import boto3
from typing import Dict

# ============================================================================
# Configuration
# ============================================================================

AWS_SECRET_NAME = "facilitator-evm-private-key"
AWS_REGION = "us-east-2"
ECS_CLUSTER = "facilitator-production"
ECS_SERVICE = "facilitator-production"

# Network information
NETWORKS = {
    "avalanche-fuji": {
        "name": "Avalanche Fuji Testnet",
        "chain_id": 43113,
        "rpc": "https://avalanche-fuji-c-chain-rpc.publicnode.com",
        "explorer": "https://testnet.snowtrace.io",
        "faucet": "https://faucet.avax.network/",
        "currency": "AVAX",
        "recommended_balance": "2-5 AVAX"
    },
    "avalanche": {
        "name": "Avalanche Mainnet",
        "chain_id": 43114,
        "rpc": "https://avalanche-c-chain-rpc.publicnode.com",
        "explorer": "https://snowtrace.io",
        "currency": "AVAX",
        "recommended_balance": "5-10 AVAX"
    },
    "base-sepolia": {
        "name": "Base Sepolia Testnet",
        "chain_id": 84532,
        "rpc": "https://sepolia.base.org",
        "explorer": "https://sepolia.basescan.org",
        "faucet": "https://www.alchemy.com/faucets/base-sepolia",
        "currency": "ETH",
        "recommended_balance": "0.5-1 ETH"
    },
    "base": {
        "name": "Base Mainnet",
        "chain_id": 8453,
        "rpc": "https://mainnet.base.org",
        "explorer": "https://basescan.org",
        "currency": "ETH",
        "recommended_balance": "0.5-1 ETH"
    }
}

# ============================================================================
# Colors for terminal output
# ============================================================================

class Colors:
    HEADER = '\033[95m'
    OKBLUE = '\033[94m'
    OKCYAN = '\033[96m'
    OKGREEN = '\033[92m'
    WARNING = '\033[93m'
    FAIL = '\033[91m'
    ENDC = '\033[0m'
    BOLD = '\033[1m'

# ============================================================================
# Step 1: Generate New Wallet
# ============================================================================

def generate_new_wallet() -> Dict[str, str]:
    """Generate a new wallet for the facilitator

    Returns:
        Dict with 'address' and 'private_key' (NEVER log the private key!)
    """
    print(f"\n{Colors.HEADER}{'='*70}{Colors.ENDC}")
    print(f"{Colors.HEADER}STEP 1: Generating New Facilitator Wallet{Colors.ENDC}")
    print(f"{Colors.HEADER}{'='*70}{Colors.ENDC}\n")

    # Enable wallet features
    Account.enable_unaudited_hdwallet_features()

    # Generate new account
    print(f"{Colors.OKBLUE}[*] Generating new wallet...{Colors.ENDC}")
    account = Account.create()

    print(f"{Colors.OKGREEN}[OK] Wallet generated successfully!{Colors.ENDC}\n")

    return {
        'address': account.address,
        'private_key': account.key.hex()
    }

# ============================================================================
# Step 2: Display Funding Instructions
# ============================================================================

def display_funding_instructions(address: str):
    """Display funding instructions for the new wallet"""
    print(f"\n{Colors.HEADER}{'='*70}{Colors.ENDC}")
    print(f"{Colors.HEADER}STEP 2: Fund New Wallet{Colors.ENDC}")
    print(f"{Colors.HEADER}{'='*70}{Colors.ENDC}\n")

    print(f"{Colors.BOLD}New Facilitator Wallet Address:{Colors.ENDC}")
    print(f"{Colors.OKCYAN}{address}{Colors.ENDC}\n")

    print(f"{Colors.BOLD}Fund this wallet on the following networks:{Colors.ENDC}\n")

    for network_id, network in NETWORKS.items():
        print(f"{Colors.OKBLUE}--- {network['name']} ---{Colors.ENDC}")
        print(f"  Explorer: {network['explorer']}/address/{address}")
        if 'faucet' in network:
            print(f"  Faucet:   {network['faucet']}")
        print(f"  Recommended: {network['recommended_balance']}")
        print()

    print(f"{Colors.WARNING}[!] IMPORTANT: Fund ALL networks before proceeding to deployment!{Colors.ENDC}\n")

# ============================================================================
# Step 3: Update AWS Secrets Manager
# ============================================================================

def update_aws_secrets(private_key: str, address: str, dry_run: bool = True) -> bool:
    """Update AWS Secrets Manager with new facilitator private key

    Args:
        private_key: The new private key (NEVER logged!)
        address: The public address (for verification only)
        dry_run: If True, only simulate the update

    Returns:
        True if successful, False otherwise
    """
    print(f"\n{Colors.HEADER}{'='*70}{Colors.ENDC}")
    print(f"{Colors.HEADER}STEP 3: Update AWS Secrets Manager{Colors.ENDC}")
    print(f"{Colors.HEADER}{'='*70}{Colors.ENDC}\n")

    if dry_run:
        print(f"{Colors.WARNING}[DRY RUN] Would update AWS Secrets Manager:{Colors.ENDC}")
        print(f"  Secret Name: {AWS_SECRET_NAME}")
        print(f"  Region: {AWS_REGION}")
        print(f"  New Address: {address}")
        print(f"\n{Colors.WARNING}[DRY RUN] No changes made.{Colors.ENDC}\n")
        return True

    try:
        print(f"{Colors.OKBLUE}[*] Connecting to AWS Secrets Manager...{Colors.ENDC}")
        client = boto3.client('secretsmanager', region_name=AWS_REGION)

        # Prepare secret value
        secret_value = {
            "private_key": private_key,
            "address": address
        }

        print(f"{Colors.OKBLUE}[*] Updating secret '{AWS_SECRET_NAME}'...{Colors.ENDC}")

        try:
            # Try to update existing secret
            client.put_secret_value(
                SecretId=AWS_SECRET_NAME,
                SecretString=json.dumps(secret_value)
            )
            print(f"{Colors.OKGREEN}[OK] Secret updated successfully!{Colors.ENDC}\n")

        except client.exceptions.ResourceNotFoundException:
            # Secret doesn't exist, create it
            print(f"{Colors.WARNING}[!] Secret not found, creating new secret...{Colors.ENDC}")
            client.create_secret(
                Name=AWS_SECRET_NAME,
                Description="Karmacadabra x402 Facilitator Hot Wallet",
                SecretString=json.dumps(secret_value)
            )
            print(f"{Colors.OKGREEN}[OK] Secret created successfully!{Colors.ENDC}\n")

        return True

    except Exception as e:
        print(f"{Colors.FAIL}[X] Error updating AWS Secrets Manager: {e}{Colors.ENDC}\n")
        return False

# ============================================================================
# Step 4: Redeploy Facilitator on ECS
# ============================================================================

def redeploy_facilitator(dry_run: bool = True) -> bool:
    """Force new deployment of facilitator ECS service

    Args:
        dry_run: If True, only simulate the deployment

    Returns:
        True if successful, False otherwise
    """
    print(f"\n{Colors.HEADER}{'='*70}{Colors.ENDC}")
    print(f"{Colors.HEADER}STEP 4: Redeploy Facilitator on ECS{Colors.ENDC}")
    print(f"{Colors.HEADER}{'='*70}{Colors.ENDC}\n")

    if dry_run:
        print(f"{Colors.WARNING}[DRY RUN] Would force ECS deployment:{Colors.ENDC}")
        print(f"  Cluster: {ECS_CLUSTER}")
        print(f"  Service: {ECS_SERVICE}")
        print(f"\n{Colors.WARNING}[DRY RUN] No deployment triggered.{Colors.ENDC}\n")
        return True

    try:
        print(f"{Colors.OKBLUE}[*] Using deployment script...{Colors.ENDC}")

        result = subprocess.run(
            ['python', 'scripts/deploy-to-fargate.py', 'facilitator'],
            cwd=Path(__file__).parent.parent,
            capture_output=True,
            text=True
        )

        if result.returncode == 0:
            print(f"{Colors.OKGREEN}[OK] Facilitator redeployed successfully!{Colors.ENDC}\n")
            return True
        else:
            print(f"{Colors.FAIL}[X] Deployment failed:{Colors.ENDC}")
            print(result.stderr)
            return False

    except Exception as e:
        print(f"{Colors.FAIL}[X] Error redeploying facilitator: {e}{Colors.ENDC}\n")
        return False

# ============================================================================
# Step 5: Verify New Wallet
# ============================================================================

def verify_new_wallet(address: str):
    """Verify the new wallet is operational"""
    print(f"\n{Colors.HEADER}{'='*70}{Colors.ENDC}")
    print(f"{Colors.HEADER}STEP 5: Verify New Wallet{Colors.ENDC}")
    print(f"{Colors.HEADER}{'='*70}{Colors.ENDC}\n")

    print(f"{Colors.OKBLUE}[*] Checking facilitator logs...{Colors.ENDC}")

    try:
        # Check CloudWatch logs for the new address
        result = subprocess.run(
            [
                'python', '-c',
                f"import boto3; logs = boto3.client('logs', region_name='{AWS_REGION}'); "
                f"events = logs.filter_log_events(logGroupName='/ecs/karmacadabra-prod/facilitator', limit=10); "
                f"[print(e['message']) for e in events.get('events', []) if 'Initialized provider' in e['message'] or '{address.lower()}' in e['message'].lower()]"
            ],
            capture_output=True,
            text=True
        )

        if result.returncode == 0 and result.stdout:
            print(f"{Colors.OKGREEN}[OK] New wallet detected in logs!{Colors.ENDC}")
            print(result.stdout)
        else:
            print(f"{Colors.WARNING}[!] Waiting for facilitator to start with new wallet...{Colors.ENDC}")

    except Exception as e:
        print(f"{Colors.WARNING}[!] Could not verify logs: {e}{Colors.ENDC}")

    print(f"\n{Colors.BOLD}Verify the new wallet address in facilitator logs:{Colors.ENDC}")
    print(f"  Expected Address: {address}")
    print(f"\n{Colors.BOLD}Check /supported endpoint:{Colors.ENDC}")
    print(f"  curl https://facilitator.ultravioletadao.xyz/supported\n")

# ============================================================================
# Main Workflow
# ============================================================================

def main():
    """Main entry point"""

    # Parse arguments
    mode = 'help'
    if '--generate' in sys.argv:
        mode = 'generate'
    elif '--deploy' in sys.argv:
        mode = 'deploy'
    elif '--full' in sys.argv:
        mode = 'full'

    # Display banner
    print(f"\n{Colors.HEADER}{'='*70}{Colors.ENDC}")
    print(f"{Colors.HEADER}FACILITATOR WALLET ROTATION{Colors.ENDC}")
    print(f"{Colors.HEADER}{'='*70}{Colors.ENDC}")

    if mode == 'help':
        print(__doc__)
        return

    # GENERATE mode: Create new wallet and show funding instructions
    if mode == 'generate':
        wallet = generate_new_wallet()
        display_funding_instructions(wallet['address'])

        # Save to temporary file (gitignored)
        temp_file = Path(__file__).parent.parent / '.facilitator_wallet_temp.json'
        with open(temp_file, 'w') as f:
            json.dump(wallet, f)

        print(f"{Colors.OKGREEN}[OK] Wallet saved to: {temp_file}{Colors.ENDC}")
        print(f"{Colors.WARNING}!  NEXT STEP: Fund the wallet on all networks, then run:{Colors.ENDC}")
        print(f"{Colors.OKCYAN}    python scripts/rotate-facilitator-wallet.py --deploy{Colors.ENDC}\n")
        return

    # DEPLOY mode: Update AWS and redeploy
    if mode == 'deploy':
        temp_file = Path(__file__).parent.parent / '.facilitator_wallet_temp.json'

        if not temp_file.exists():
            print(f"{Colors.FAIL}[X] No wallet found! Run --generate first.{Colors.ENDC}\n")
            return

        with open(temp_file, 'r') as f:
            wallet = json.load(f)

        # Confirm funding
        print(f"\n{Colors.WARNING}Have you funded the wallet on ALL networks?{Colors.ENDC}")
        print(f"  Address: {wallet['address']}")

        try:
            response = input(f"\n{Colors.BOLD}Proceed with deployment? (yes/no): {Colors.ENDC}")
            if response.lower() not in ['yes', 'y']:
                print(f"{Colors.WARNING}[!] Deployment cancelled.{Colors.ENDC}\n")
                return
        except (EOFError, KeyboardInterrupt):
            print(f"\n{Colors.WARNING}[!] Deployment cancelled.{Colors.ENDC}\n")
            return

        # Update AWS
        if not update_aws_secrets(wallet['private_key'], wallet['address'], dry_run=False):
            print(f"{Colors.FAIL}[X] Failed to update AWS Secrets Manager. Aborting.{Colors.ENDC}\n")
            return

        # Redeploy
        if not redeploy_facilitator(dry_run=False):
            print(f"{Colors.FAIL}[X] Failed to redeploy facilitator.{Colors.ENDC}\n")
            return

        # Verify
        verify_new_wallet(wallet['address'])

        # Clean up temp file
        temp_file.unlink()
        print(f"{Colors.OKGREEN}[OK] Wallet rotation complete! Temporary file deleted.{Colors.ENDC}\n")

    # FULL mode: Do everything at once
    if mode == 'full':
        wallet = generate_new_wallet()
        display_funding_instructions(wallet['address'])

        print(f"{Colors.WARNING}!  MANUAL STEP REQUIRED: Fund the wallet on all networks!{Colors.ENDC}\n")

        try:
            response = input(f"{Colors.BOLD}Have you funded the wallet? (yes/no): {Colors.ENDC}")
            if response.lower() not in ['yes', 'y']:
                print(f"{Colors.WARNING}[!] Rotation cancelled.{Colors.ENDC}\n")
                return
        except (EOFError, KeyboardInterrupt):
            print(f"\n{Colors.WARNING}[!] Rotation cancelled.{Colors.ENDC}\n")
            return

        # Update and deploy
        if update_aws_secrets(wallet['private_key'], wallet['address'], dry_run=False):
            redeploy_facilitator(dry_run=False)
            verify_new_wallet(wallet['address'])
            print(f"{Colors.OKGREEN}[OK] Full wallet rotation complete!{Colors.ENDC}\n")

if __name__ == "__main__":
    main()
