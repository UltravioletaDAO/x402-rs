#!/bin/bash
# ============================================================================
# Secrets Validation Script
# ============================================================================
# This script validates that all required secrets exist in AWS Secrets Manager
# and have the correct structure before deploying infrastructure changes.
#
# Usage:
#   ./validate_secrets.sh [--region us-east-2]
#
# Exit codes:
#   0 - All secrets valid
#   1 - Missing or invalid secrets
# ============================================================================

set -euo pipefail

# Configuration
REGION="${1:-us-east-2}"
FAILED=0

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Helper functions
error() {
    echo -e "${RED}✗ $1${NC}" >&2
    FAILED=1
}

success() {
    echo -e "${GREEN}✓ $1${NC}"
}

warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

# Check if AWS CLI is installed
if ! command -v aws &> /dev/null; then
    error "AWS CLI not installed. Install it first."
    exit 1
fi

# Check if jq is installed
if ! command -v jq &> /dev/null; then
    error "jq not installed. Install it first (needed for JSON parsing)."
    exit 1
fi

echo "============================================================================"
echo "Validating Facilitator Secrets - Region: $REGION"
echo "============================================================================"
echo ""

# ============================================================================
# Wallet Secrets (JSON with private_key field)
# ============================================================================

info "Checking wallet secrets..."
echo ""

# EVM Wallets
declare -a evm_secrets=(
    "facilitator-evm-mainnet-private-key"
    "facilitator-evm-testnet-private-key"
    "facilitator-evm-private-key"
)

for secret_name in "${evm_secrets[@]}"; do
    if aws secretsmanager describe-secret --secret-id "$secret_name" --region "$REGION" &> /dev/null; then
        secret_value=$(aws secretsmanager get-secret-value --secret-id "$secret_name" --region "$REGION" --query 'SecretString' --output text)

        if echo "$secret_value" | jq -e '.private_key' > /dev/null 2>&1; then
            private_key=$(echo "$secret_value" | jq -r '.private_key')
            if [[ -n "$private_key" && "$private_key" != "null" ]]; then
                success "$secret_name: Valid (private_key: ${private_key:0:10}...)"
            else
                error "$secret_name: Empty or null private_key"
            fi
        else
            error "$secret_name: Missing 'private_key' field in JSON"
        fi
    else
        error "$secret_name: Secret not found"
    fi
done

echo ""

# Solana Wallets
declare -a solana_secrets=(
    "facilitator-solana-mainnet-keypair"
    "facilitator-solana-testnet-keypair"
    "facilitator-solana-keypair"
)

for secret_name in "${solana_secrets[@]}"; do
    if aws secretsmanager describe-secret --secret-id "$secret_name" --region "$REGION" &> /dev/null; then
        secret_value=$(aws secretsmanager get-secret-value --secret-id "$secret_name" --region "$REGION" --query 'SecretString' --output text)

        if echo "$secret_value" | jq -e '.private_key' > /dev/null 2>&1; then
            private_key=$(echo "$secret_value" | jq -r '.private_key')
            success "$secret_name: Valid (private_key: ${private_key:0:10}...)"
        else
            error "$secret_name: Missing 'private_key' field in JSON"
        fi
    else
        error "$secret_name: Secret not found"
    fi
done

echo ""

# NEAR Wallets (needs private_key AND account_id)
declare -a near_secrets=(
    "facilitator-near-mainnet-keypair"
    "facilitator-near-testnet-keypair"
)

for secret_name in "${near_secrets[@]}"; do
    if aws secretsmanager describe-secret --secret-id "$secret_name" --region "$REGION" &> /dev/null; then
        secret_value=$(aws secretsmanager get-secret-value --secret-id "$secret_name" --region "$REGION" --query 'SecretString' --output text)

        has_private_key=$(echo "$secret_value" | jq -e '.private_key' > /dev/null 2>&1 && echo "yes" || echo "no")
        has_account_id=$(echo "$secret_value" | jq -e '.account_id' > /dev/null 2>&1 && echo "yes" || echo "no")

        if [[ "$has_private_key" == "yes" && "$has_account_id" == "yes" ]]; then
            private_key=$(echo "$secret_value" | jq -r '.private_key')
            account_id=$(echo "$secret_value" | jq -r '.account_id')
            success "$secret_name: Valid (account: $account_id, key: ${private_key:0:15}...)"
        else
            if [[ "$has_private_key" == "no" ]]; then
                error "$secret_name: Missing 'private_key' field"
            fi
            if [[ "$has_account_id" == "no" ]]; then
                error "$secret_name: Missing 'account_id' field"
            fi
        fi
    else
        error "$secret_name: Secret not found"
    fi
done

echo ""

# Stellar Wallets (plain string, not JSON)
declare -a stellar_secrets=(
    "facilitator-stellar-keypair-mainnet"
    "facilitator-stellar-keypair-testnet"
)

for secret_name in "${stellar_secrets[@]}"; do
    if aws secretsmanager describe-secret --secret-id "$secret_name" --region "$REGION" &> /dev/null; then
        secret_value=$(aws secretsmanager get-secret-value --secret-id "$secret_name" --region "$REGION" --query 'SecretString' --output text)

        if [[ $secret_value == S* ]]; then
            success "$secret_name: Valid (key: ${secret_value:0:5}...)"
        else
            error "$secret_name: Invalid format (Stellar secret keys must start with 'S')"
        fi
    else
        error "$secret_name: Secret not found"
    fi
done

echo ""

# ============================================================================
# RPC URL Secrets (JSON with network keys)
# ============================================================================

info "Checking RPC URL secrets..."
echo ""

# Mainnet RPCs
secret_name="facilitator-rpc-mainnet"
if aws secretsmanager describe-secret --secret-id "$secret_name" --region "$REGION" &> /dev/null; then
    secret_value=$(aws secretsmanager get-secret-value --secret-id "$secret_name" --region "$REGION" --query 'SecretString' --output text)

    # Expected networks in mainnet RPC secret
    expected_networks=("base" "avalanche" "polygon" "optimism" "celo" "hyperevm" "ethereum" "arbitrum" "unichain" "solana" "near")

    network_count=$(echo "$secret_value" | jq 'keys | length')
    networks=$(echo "$secret_value" | jq -r 'keys | join(", ")')

    success "$secret_name: Found $network_count networks: $networks"

    # Check for expected networks
    for network in "${expected_networks[@]}"; do
        # Use --arg to safely handle network names with special characters
        if echo "$secret_value" | jq -e --arg net "$network" '.[$net]' > /dev/null 2>&1; then
            rpc_url=$(echo "$secret_value" | jq -r --arg net "$network" '.[$net]')
            # Truncate URL to hide API keys
            short_url="${rpc_url:0:30}..."
            success "  - $network: $short_url"
        else
            warning "  - $network: MISSING (using fallback or public RPC)"
        fi
    done
else
    error "$secret_name: Secret not found"
fi

echo ""

# Testnet RPCs
secret_name="facilitator-rpc-testnet"
if aws secretsmanager describe-secret --secret-id "$secret_name" --region "$REGION" &> /dev/null; then
    secret_value=$(aws secretsmanager get-secret-value --secret-id "$secret_name" --region "$REGION" --query 'SecretString' --output text)

    network_count=$(echo "$secret_value" | jq 'keys | length')
    networks=$(echo "$secret_value" | jq -r 'keys | join(", ")')

    success "$secret_name: Found $network_count networks: $networks"

    # Testnet RPCs are optional (can use public endpoints)
    for network in $(echo "$secret_value" | jq -r 'keys[]'); do
        # Escape network name for jq (handles dashes and special chars)
        rpc_url=$(echo "$secret_value" | jq -r --arg net "$network" '.[$net]')
        short_url="${rpc_url:0:30}..."
        success "  - $network: $short_url"
    done
else
    error "$secret_name: Secret not found"
fi

echo ""

# ============================================================================
# Summary
# ============================================================================

echo "============================================================================"
if [ $FAILED -eq 0 ]; then
    success "All secrets validated successfully!"
    echo ""
    info "Terraform can safely deploy with these secrets."
    echo "============================================================================"
    exit 0
else
    error "Some secrets are missing or invalid!"
    echo ""
    info "Fix the errors above before running 'terraform apply'."
    info "See SECRETS_MANAGEMENT.md for secret structure and creation instructions."
    echo "============================================================================"
    exit 1
fi
