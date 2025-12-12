# ============================================================================
# AWS Secrets Manager - Data Sources
# ============================================================================
# This file defines all secrets required by the facilitator.
# When adding a new blockchain network, add its secret references here.

# ----------------------------------------------------------------------------
# Wallet Secrets (Private Keys)
# ----------------------------------------------------------------------------

# EVM Wallets (mainnet and testnet)
data "aws_secretsmanager_secret" "evm_mainnet_private_key" {
  name = "facilitator-evm-mainnet-private-key"
}

data "aws_secretsmanager_secret" "evm_testnet_private_key" {
  name = "facilitator-evm-testnet-private-key"
}

# Legacy EVM wallet (for backward compatibility)
data "aws_secretsmanager_secret" "evm_private_key_legacy" {
  name = var.evm_secret_name
}

# Solana Wallets (mainnet and testnet)
data "aws_secretsmanager_secret" "solana_mainnet_keypair" {
  name = "facilitator-solana-mainnet-keypair"
}

data "aws_secretsmanager_secret" "solana_testnet_keypair" {
  name = "facilitator-solana-testnet-keypair"
}

# Legacy Solana wallet (for backward compatibility)
data "aws_secretsmanager_secret" "solana_keypair_legacy" {
  name = var.solana_secret_name
}

# NEAR Wallets (mainnet and testnet)
data "aws_secretsmanager_secret" "near_mainnet_keypair" {
  name = "facilitator-near-mainnet-keypair"
}

data "aws_secretsmanager_secret" "near_testnet_keypair" {
  name = "facilitator-near-testnet-keypair"
}

# Stellar Wallets (mainnet and testnet)
data "aws_secretsmanager_secret" "stellar_mainnet_keypair" {
  name = "facilitator-stellar-keypair-mainnet"
}

data "aws_secretsmanager_secret" "stellar_testnet_keypair" {
  name = "facilitator-stellar-keypair-testnet"
}

# ----------------------------------------------------------------------------
# RPC URL Secrets (Premium Endpoints)
# ----------------------------------------------------------------------------

# Mainnet RPC URLs (QuickNode, Alchemy, etc.)
data "aws_secretsmanager_secret" "rpc_mainnet" {
  name = "facilitator-rpc-mainnet"
}

# Testnet RPC URLs
data "aws_secretsmanager_secret" "rpc_testnet" {
  name = "facilitator-rpc-testnet"
}

# ============================================================================
# Secret ARN Outputs (for IAM policy and task definition)
# ============================================================================

locals {
  # All wallet secret ARNs that need IAM permissions
  wallet_secret_arns = [
    data.aws_secretsmanager_secret.evm_mainnet_private_key.arn,
    data.aws_secretsmanager_secret.evm_testnet_private_key.arn,
    data.aws_secretsmanager_secret.evm_private_key_legacy.arn,
    data.aws_secretsmanager_secret.solana_mainnet_keypair.arn,
    data.aws_secretsmanager_secret.solana_testnet_keypair.arn,
    data.aws_secretsmanager_secret.solana_keypair_legacy.arn,
    data.aws_secretsmanager_secret.near_mainnet_keypair.arn,
    data.aws_secretsmanager_secret.near_testnet_keypair.arn,
    data.aws_secretsmanager_secret.stellar_mainnet_keypair.arn,
    data.aws_secretsmanager_secret.stellar_testnet_keypair.arn,
  ]

  # All RPC secret ARNs that need IAM permissions
  rpc_secret_arns = [
    data.aws_secretsmanager_secret.rpc_mainnet.arn,
    data.aws_secretsmanager_secret.rpc_testnet.arn,
  ]

  # Combined list for IAM policy
  all_secret_arns = concat(local.wallet_secret_arns, local.rpc_secret_arns)
}

# ============================================================================
# ECS Task Definition Secret Mappings
# ============================================================================
# These locals define the complete mapping from environment variables to
# Secrets Manager values. This is the SINGLE SOURCE OF TRUTH for secrets.
# When adding a new network, add its required secrets here.

locals {
  # ----------------------------------------------------------------------------
  # Wallet Private Keys
  # ----------------------------------------------------------------------------
  wallet_secrets = [
    # EVM wallets (network-specific)
    {
      name      = "EVM_PRIVATE_KEY_MAINNET"
      valueFrom = "${data.aws_secretsmanager_secret.evm_mainnet_private_key.arn}:private_key::"
    },
    {
      name      = "EVM_PRIVATE_KEY_TESTNET"
      valueFrom = "${data.aws_secretsmanager_secret.evm_testnet_private_key.arn}:private_key::"
    },
    # Legacy EVM wallet (fallback for backward compatibility)
    {
      name      = "EVM_PRIVATE_KEY"
      valueFrom = "${data.aws_secretsmanager_secret.evm_private_key_legacy.arn}:private_key::"
    },

    # Solana wallets (network-specific)
    {
      name      = "SOLANA_PRIVATE_KEY_MAINNET"
      valueFrom = "${data.aws_secretsmanager_secret.solana_mainnet_keypair.arn}:private_key::"
    },
    {
      name      = "SOLANA_PRIVATE_KEY_TESTNET"
      valueFrom = "${data.aws_secretsmanager_secret.solana_testnet_keypair.arn}:private_key::"
    },
    # Legacy Solana wallet (fallback for backward compatibility)
    {
      name      = "SOLANA_PRIVATE_KEY"
      valueFrom = "${data.aws_secretsmanager_secret.solana_keypair_legacy.arn}:private_key::"
    },

    # NEAR wallets (network-specific with account IDs)
    {
      name      = "NEAR_PRIVATE_KEY_MAINNET"
      valueFrom = "${data.aws_secretsmanager_secret.near_mainnet_keypair.arn}:private_key::"
    },
    {
      name      = "NEAR_ACCOUNT_ID_MAINNET"
      valueFrom = "${data.aws_secretsmanager_secret.near_mainnet_keypair.arn}:account_id::"
    },
    {
      name      = "NEAR_PRIVATE_KEY_TESTNET"
      valueFrom = "${data.aws_secretsmanager_secret.near_testnet_keypair.arn}:private_key::"
    },
    {
      name      = "NEAR_ACCOUNT_ID_TESTNET"
      valueFrom = "${data.aws_secretsmanager_secret.near_testnet_keypair.arn}:account_id::"
    },

    # Stellar wallets (network-specific, plain string format)
    {
      name      = "STELLAR_PRIVATE_KEY_MAINNET"
      valueFrom = data.aws_secretsmanager_secret.stellar_mainnet_keypair.arn
    },
    {
      name      = "STELLAR_PRIVATE_KEY_TESTNET"
      valueFrom = data.aws_secretsmanager_secret.stellar_testnet_keypair.arn
    },
  ]

  # ----------------------------------------------------------------------------
  # Mainnet RPC URLs (from facilitator-rpc-mainnet secret)
  # ----------------------------------------------------------------------------
  # Current networks with premium mainnet RPCs:
  # - base, avalanche, polygon, optimism, celo, hyperevm, ethereum, arbitrum, unichain, solana, near
  mainnet_rpc_secrets = [
    # EVM Networks
    {
      name      = "RPC_URL_BASE"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:base::"
    },
    {
      name      = "RPC_URL_AVALANCHE"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:avalanche::"
    },
    {
      name      = "RPC_URL_POLYGON"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:polygon::"
    },
    {
      name      = "RPC_URL_OPTIMISM"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:optimism::"
    },
    {
      name      = "RPC_URL_CELO"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:celo::"
    },
    {
      name      = "RPC_URL_HYPEREVM"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:hyperevm::"
    },
    {
      name      = "RPC_URL_ETHEREUM"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:ethereum::"
    },
    {
      name      = "RPC_URL_ARBITRUM"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:arbitrum::"
    },
    {
      name      = "RPC_URL_UNICHAIN"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:unichain::"
    },

    # Non-EVM Networks
    {
      name      = "RPC_URL_SOLANA"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:solana::"
    },
    {
      name      = "RPC_URL_NEAR"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:near::"
    },
  ]

  # ----------------------------------------------------------------------------
  # Testnet RPC URLs (from facilitator-rpc-testnet secret)
  # ----------------------------------------------------------------------------
  # Current networks with testnet RPCs in Secrets Manager:
  # - solana-devnet, arbitrum-sepolia, near (testnet)
  testnet_rpc_secrets = [
    {
      name      = "RPC_URL_SOLANA_DEVNET"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_testnet.arn}:solana-devnet::"
    },
    {
      name      = "RPC_URL_ARBITRUM_SEPOLIA"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_testnet.arn}:arbitrum-sepolia::"
    },
    {
      name      = "RPC_URL_NEAR_TESTNET"
      valueFrom = "${data.aws_secretsmanager_secret.rpc_testnet.arn}:near::"
    },
  ]

  # Combined secrets array for task definition
  all_task_secrets = concat(
    local.wallet_secrets,
    local.mainnet_rpc_secrets,
    local.testnet_rpc_secrets
  )
}
