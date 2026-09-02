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

# Sui Wallets (mainnet and testnet)
data "aws_secretsmanager_secret" "sui_mainnet_keypair" {
  name = "facilitator-sui-keypair-mainnet"
}

data "aws_secretsmanager_secret" "sui_testnet_keypair" {
  name = "facilitator-sui-keypair-testnet"
}

# Algorand Wallets (mainnet and testnet)
data "aws_secretsmanager_secret" "algorand_mainnet_mnemonic" {
  name = "facilitator-algorand-mnemonic-mainnet"
}

data "aws_secretsmanager_secret" "algorand_testnet_mnemonic" {
  name = "facilitator-algorand-mnemonic-testnet"
}

# ----------------------------------------------------------------------------
# Admin Credentials
# ----------------------------------------------------------------------------

# Bearer token for POST /feedback/revoke.
#
# Deliberately NOT the bazaar admin token: the ERC-8004 Reputation Registry
# authorises revokeFeedback by msg.sender, which is the facilitator, so this one
# credential can erase any feedback the registry attributes to our wallet -
# irreversibly, and for third parties. Leaking the bazaar token hides a catalog
# listing; leaking this one destroys reputation. Different blast radius,
# different secret.
#
# The facilitator fails CLOSED: with no ERC8004_ADMIN_TOKEN in the environment
# the route answers 404 and is indistinguishable from one that does not exist.
data "aws_secretsmanager_secret" "erc8004_admin_token" {
  name = "facilitator-erc8004-admin-token"
}

# DX402 receipt signing key.
#
# The facilitator signs an EIP-712 EvidenceReceipt with this so a third party can
# verify, offline and without calling us, that we attested a given payment
# produced a given piece of evidence.
#
# This is NOT a payment wallet and must never be one. It signs attestations, not
# transfers, so it holds no funds and needs no gas — keeping it separate means a
# leak forges receipts but moves no money, and rotating it costs nothing.
#
# Looked up only when enable_dx402 is true, so the bucket and table can be
# provisioned before the secret exists. Create it with
# scripts/dx402-bootstrap-secret.sh.
data "aws_secretsmanager_secret" "dx402_signing_key" {
  count = var.enable_dx402 ? 1 : 0
  name  = "facilitator-dx402-signing-key"
}

# DX402 repair token.
#
# Gates POST /dx402/repair, which audits an anchor whose pointer resolves to
# nothing and, with ?write=true, corrects it - RE-SIGNING the receipt, because
# `pointer` is part of the EIP-712 type hash.
#
# Its own secret, deliberately not the ERC-8004 or bazaar token. Those have
# different blast radii and this one rewrites a facilitator-signed attestation:
# sharing a token would mean whoever can suppress a catalog listing can also
# re-sign somebody's evidence receipt.
#
# Fails CLOSED: with no DX402_ADMIN_TOKEN in the environment the route answers
# 404 and is indistinguishable from one that does not exist. That is also why
# this is not optional infrastructure to skip - without it the repair route can
# never be reached, and the audit of the anchors written before the pointer was
# reconciled cannot run at all.
#
# Create it the same way as the signing key, with any high-entropy value:
#   aws secretsmanager create-secret --name facilitator-dx402-admin-token \
#     --secret-string "{\"token\":\"$(openssl rand -hex 32)\"}" --region us-east-2
data "aws_secretsmanager_secret" "dx402_admin_token" {
  count = var.enable_dx402 ? 1 : 0
  name  = "facilitator-dx402-admin-token"
}

# Pinata credentials for the ipfs storage backend.
#
# Looked up only when dx402_storage_backend is "ipfs", so a deployment that never
# wanted IPFS does not need the secret to exist. The JWT embeds the API key AND
# secret in its payload, so it is equivalent to full credentials -- it lives here
# and never in a task-definition environment variable, same rule as the RPC URLs.
#
# NOTE: the JWT carries an `exp`. When it expires the failure is SILENT: anchors
# fall back to S3 and only the logs say so. Watch the expiry.
data "aws_secretsmanager_secret" "dx402_pinata" {
  count = var.enable_dx402 && var.dx402_storage_backend == "ipfs" ? 1 : 0
  name  = "facilitator-dx402-pinata"
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
    data.aws_secretsmanager_secret.sui_mainnet_keypair.arn,
    data.aws_secretsmanager_secret.sui_testnet_keypair.arn,
    data.aws_secretsmanager_secret.algorand_mainnet_mnemonic.arn,
    data.aws_secretsmanager_secret.algorand_testnet_mnemonic.arn,
  ]

  # Admin credential ARNs that need IAM permissions
  admin_secret_arns = concat(
    [data.aws_secretsmanager_secret.erc8004_admin_token.arn],
    var.enable_dx402 ? [data.aws_secretsmanager_secret.dx402_signing_key[0].arn] : [],
    var.enable_dx402 ? [data.aws_secretsmanager_secret.dx402_admin_token[0].arn] : [],
    var.enable_dx402 && var.dx402_storage_backend == "ipfs" ? [data.aws_secretsmanager_secret.dx402_pinata[0].arn] : []
  )

  # All RPC secret ARNs that need IAM permissions
  rpc_secret_arns = [
    data.aws_secretsmanager_secret.rpc_mainnet.arn,
    data.aws_secretsmanager_secret.rpc_testnet.arn,
  ]

  # Combined list for IAM policy
  all_secret_arns = concat(
    local.wallet_secret_arns,
    local.rpc_secret_arns,
    local.admin_secret_arns
  )
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

    # Sui wallets (network-specific, plain string format - bech32 suiprivkey)
    {
      name      = "SUI_PRIVATE_KEY_MAINNET"
      valueFrom = data.aws_secretsmanager_secret.sui_mainnet_keypair.arn
    },
    {
      name      = "SUI_PRIVATE_KEY_TESTNET"
      valueFrom = data.aws_secretsmanager_secret.sui_testnet_keypair.arn
    },

    # Algorand wallets (network-specific, 25-word mnemonic format)
    {
      name      = "ALGORAND_MNEMONIC_MAINNET"
      valueFrom = data.aws_secretsmanager_secret.algorand_mainnet_mnemonic.arn
    },
    {
      name      = "ALGORAND_MNEMONIC_TESTNET"
      valueFrom = data.aws_secretsmanager_secret.algorand_testnet_mnemonic.arn
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

  # ----------------------------------------------------------------------------
  # Admin credentials
  # ----------------------------------------------------------------------------
  admin_secrets = concat([
    {
      name      = "ERC8004_ADMIN_TOKEN"
      valueFrom = "${data.aws_secretsmanager_secret.erc8004_admin_token.arn}:token::"
    },
    ], var.enable_dx402 ? [
    {
      name      = "DX402_SIGNING_KEY"
      valueFrom = "${data.aws_secretsmanager_secret.dx402_signing_key[0].arn}:private_key::"
    },
    {
      name      = "DX402_ADMIN_TOKEN"
      valueFrom = "${data.aws_secretsmanager_secret.dx402_admin_token[0].arn}:token::"
    },
    ] : [], var.enable_dx402 && var.dx402_storage_backend == "ipfs" ? [
    {
      # The JWT alone -- it is what the v3 upload API takes, and its payload
      # already embeds the api key and secret, so nothing else needs to travel.
      name      = "DX402_PINATA_JWT"
      valueFrom = "${data.aws_secretsmanager_secret.dx402_pinata[0].arn}:jwt::"
    },
  ] : [])

  # Combined secrets array for task definition
  all_task_secrets = concat(
    local.wallet_secrets,
    local.mainnet_rpc_secrets,
    local.testnet_rpc_secrets,
    local.admin_secrets
  )
}
