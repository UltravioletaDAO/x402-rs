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

# Bearer token for the Bazaar curation routes (src/handlers.rs,
# discovery_admin_routes): DELETE /discovery/resources, POST
# /discovery/admin/suppress and /release, GET /discovery/admin/pending, POST
# /discovery/refresh. Until 2.51.0 nothing put it in the container -- no secret,
# no environment entry -- so in production every one of those routes answered
# 404 (fail closed, admin_auth in src/handlers.rs).
#
# An SSM Parameter Store SecureString, /facilitator/bazaar-admin-token, encrypted
# with a KMS key of its own (owner's decision 230). On the default aws/ssm key a
# SecureString is readable by any principal with ssm:GetParameter -- ReadOnlyAccess,
# and so the CI user -- because that key lets every principal of the account
# decrypt through SSM. On this key ssm:GetParameter alone reads only ciphertext:
# decrypting also takes kms:Decrypt on THIS key, which the execution role is granted
# (aws_iam_role_policy.secrets_access) and the CI user is explicitly denied
# (aws_iam_policy.cicd_infra, DenyBazaarTokenDecrypt). Its own credential, not the
# ERC-8004 or DX402 token (above): leaking it hides or deletes a catalog listing,
# and nothing else.
#
# Terraform creates the key and its alias and never touches the parameter: the ARN
# is composed (local.bazaar_admin_token_parameter_arn), so neither the value nor its
# ciphertext reaches the state, and the plan does not need the parameter to exist.
# The drift gate's "Bazaar admin token parameter is in place" step reads its
# metadata (never the value) and goes red while it is missing or on another key.
#
# BEFORE the merge that ships this, with a human's credentials, on Terraform 1.9.8,
# from WSL -- the CI user can create neither the key nor these policies:
#   terraform apply -target=aws_kms_alias.bazaar_admin_token \
#     -target=aws_iam_policy.cicd_infra -target=aws_iam_role_policy.secrets_access
# then the value, from a file only you can read, never on screen or in argv:
#   umask 077; openssl rand -hex 32 | tr -d '\n' > /dev/shm/bazaar-admin-token
#   aws ssm put-parameter --name /facilitator/bazaar-admin-token --type SecureString \
#     --key-id alias/facilitator-bazaar-admin-token \
#     --value file:///dev/shm/bazaar-admin-token --region us-east-2 \
#     --query Version --output text
#   shred -u /dev/shm/bazaar-admin-token
#
# Read it without printing it:
#   BAZAAR_ADMIN_TOKEN=$(aws ssm get-parameter --name /facilitator/bazaar-admin-token \
#     --with-decryption --region us-east-2 --query Parameter.Value --output text)
# Rotate with the same put-parameter plus --overwrite, then a deploy -- a task
# reads it when it starts.
resource "aws_kms_key" "bazaar_admin_token" {
  description             = "Encrypts the SSM parameter /facilitator/bazaar-admin-token (BAZAAR_ADMIN_TOKEN)"
  enable_key_rotation     = true
  rotation_period_in_days = 365
  deletion_window_in_days = 30

  tags = {
    Name = "facilitator-bazaar-admin-token"
  }
}

resource "aws_kms_alias" "bazaar_admin_token" {
  name          = local.bazaar_admin_token_kms_alias
  target_key_id = aws_kms_key.bazaar_admin_token.key_id
}

locals {
  bazaar_admin_token_parameter     = "/facilitator/bazaar-admin-token"
  bazaar_admin_token_parameter_arn = "arn:aws:ssm:${var.aws_region}:${data.aws_caller_identity.current.account_id}:parameter${local.bazaar_admin_token_parameter}"
  bazaar_admin_token_kms_alias     = "alias/facilitator-bazaar-admin-token"
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
# Stack identities (X-UVD-Stack-Key, src/rate_policy.rs)
# ----------------------------------------------------------------------------

# One secret per stack service, holding ONLY the SHA-256 of that service's key:
# {"sha256": "<64 hex>"}. A caller presenting the key skips the per-IP budgets.
#
# The key itself lives in a separate secret in the CLIENT's own store, and this
# execution role is never granted it: with one secret carrying both fields, the
# GetSecretValue this role needs would read every key in the clear, whatever
# field ECS injects. A digest that leaks authenticates nobody.
#
# The task reads each digest from its SSM copy,
# /facilitator/stack-key-digest-<service>/sha256 (task definition mappings,
# below), so that parameter is what a change has to reach. It holds the BARE
# value ECS injects -- "<64 hex>", or "<new>,<old>" -- never the JSON document
# scripts/stack_key.py writes: a parameter holding {"sha256": ...} is skipped
# with a warning and the service silently becomes a third party (429s). Revoke
# by overwriting it with a value that is not a digest, such as `revoked`
# (src/rate_policy.rs skips it with a warning; SSM refuses an empty value) --
# never delete it: ECS refuses to start a task whose valueFrom names a parameter
# that is missing. Rotate with "<new>,<old>" for one restart. Generate both
# halves with scripts/stack_key.py; the full procedure is
# docs/handoffs/X4-STACK-429.md section 6, written for the Secrets Manager
# secret, which the task no longer reads.
data "aws_secretsmanager_secret" "stack_key_digest_execution_market" {
  name = "facilitator-stack-key-digest-execution-market"
}

data "aws_secretsmanager_secret" "stack_key_digest_karmakadabra" {
  name = "facilitator-stack-key-digest-karmakadabra"
}

data "aws_secretsmanager_secret" "stack_key_digest_describe_net" {
  name = "facilitator-stack-key-digest-describe-net"
}

data "aws_secretsmanager_secret" "stack_key_digest_meshrelay" {
  name = "facilitator-stack-key-digest-meshrelay"
}

# Emporium reads the bazaar with its own key (decision 144). Declared WITHOUT a
# value. The task definition reads the digest from the SSM parameter
# /facilitator/stack-key-digest-emporium/sha256, not from this secret (decision
# 266, mappings below), and that parameter does not exist yet: put the bare
# "<64 hex>" there first (scripts/stack_key.py generate --service emporium;
# SecureString, --key-id alias/facilitator-bazaar-admin-token), then set
# var.stack_key_emporium_loaded = true. Loading only this secret leaves every
# new task unable to start.
#
# Nothing a deploy applies references this resource. A reference -- even in the
# branch of a conditional that is not taken -- would put its creation inside the
# image deploy's -target closure, and the CI user cannot create secrets. The
# execution role's Secrets Manager grant reaches it through the data source
# below, by name, once the variable is set.
resource "aws_secretsmanager_secret" "stack_key_digest_emporium" {
  name        = "facilitator-stack-key-digest-emporium"
  description = "SHA-256 of Emporium's X-UVD-Stack-Key (field sha256). Loaded by hand."

  tags = {
    Name = "facilitator-stack-key-digest-emporium"
  }
}

data "aws_secretsmanager_secret" "stack_key_digest_emporium" {
  count = var.stack_key_emporium_loaded ? 1 : 0
  name  = "facilitator-stack-key-digest-emporium"
}

# ----------------------------------------------------------------------------
# IP allowlist (src/ip_allowlist.rs)
# ----------------------------------------------------------------------------

# The client addresses that skip the per-IP rate policy -- the owner's tests from
# home, which carry no stack key (decision 144) -- live in var.ip_allowlist_secret_name
# (uvd/allowlist/home). It is ONE secret for the whole stack, created and loaded
# by hand outside Terraform, and deliberately declared by no repository: two
# states declaring it would collide on the second apply. This one only reads it,
# by name (aws_iam_role_policy.ip_allowlist_read in main.tf). The task reads it
# itself, at runtime and again every refresh -- not through the task
# definition -- so a new address needs no deploy, and a secret that does not
# exist yet is an empty list, not a task that fails to start.

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
    data.aws_secretsmanager_secret.hedera_testnet.arn,
    data.aws_secretsmanager_secret.hedera_mainnet.arn,
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
    [data.aws_secretsmanager_secret.facilitator_receipt_key.arn],
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

  # Stack key digests. Only the facilitator's half: the clients' key secrets
  # stay out of this list, so this role can never read a key.
  stack_key_digest_arns = concat([
    data.aws_secretsmanager_secret.stack_key_digest_execution_market.arn,
    data.aws_secretsmanager_secret.stack_key_digest_karmakadabra.arn,
    data.aws_secretsmanager_secret.stack_key_digest_describe_net.arn,
    data.aws_secretsmanager_secret.stack_key_digest_meshrelay.arn,
    ], var.stack_key_emporium_loaded ? [
    data.aws_secretsmanager_secret.stack_key_digest_emporium[0].arn,
  ] : [])

  # Combined list for IAM policy
  all_secret_arns = concat(
    local.wallet_secret_arns,
    local.rpc_secret_arns,
    local.admin_secret_arns,
    local.stack_key_digest_arns
  )
}

# ============================================================================
# ECS Task Definition Secret Mappings
# ============================================================================
# These locals define the complete mapping from environment variables to the
# values ECS injects. This is the SINGLE SOURCE OF TRUTH for secrets.
# When adding a new network, add its required secrets here.
#
# Every value comes from an SSM Parameter Store SecureString under /facilitator/
# (owner's decision 266), never from Secrets Manager. The parameters are on the
# key behind alias/facilitator-bazaar-admin-token -- its name is historical: the
# bazaar token was the first parameter on it, and the copies of every other
# secret went onto the same key -- so the execution role's kms:Decrypt grant
# (main.tf, aws_iam_role_policy.secrets_access) covers them all.
#
# SSM cannot pick a field out of a JSON value the way a Secrets Manager
# valueFrom `<arn>:<field>::` does, so each field the task reads is a parameter
# of its own, named after the secret it was copied from (2026-10-16):
#   secret facilitator-<name>, field <field>   ->  /facilitator/<name>/<field>
#   secret facilitator-<name>, whole string    ->  /facilitator/<name>
# scripts/verificar_ssm_igual.py compares each pair in memory and prints only
# ok / DIFIERE. A new secret is a new parameter: put-parameter --type
# SecureString --key-id alias/facilitator-bazaar-admin-token, then add it here.
#
# The ARN is composed, never read: a data or resource "aws_ssm_parameter" would
# put the value or its ciphertext in the state, and the plan does not need the
# parameter to exist. A missing one stops the task from starting instead.
#
# The Secrets Manager data sources above, and the execution role's
# secretsmanager:GetSecretValue on them (local.all_secret_arns), stay until the
# task has run on these parameters: reverting this mapping is the rollback, and
# it needs that grant. They go together, in a change of their own, before the
# originals are deleted.

locals {
  facilitator_parameter_arn_prefix = "arn:aws:ssm:${var.aws_region}:${data.aws_caller_identity.current.account_id}:parameter/facilitator"

  # ----------------------------------------------------------------------------
  # Wallet Private Keys
  # ----------------------------------------------------------------------------
  wallet_secrets = [
    # EVM wallets (network-specific)
    {
      name      = "EVM_PRIVATE_KEY_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/evm-mainnet-private-key/private_key"
    },
    {
      name      = "EVM_PRIVATE_KEY_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/evm-testnet-private-key/private_key"
    },
    # Legacy EVM wallet (fallback for backward compatibility)
    {
      name      = "EVM_PRIVATE_KEY"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/evm-private-key/private_key"
    },

    # Solana wallets (network-specific)
    {
      name      = "SOLANA_PRIVATE_KEY_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/solana-mainnet-keypair/private_key"
    },
    {
      name      = "SOLANA_PRIVATE_KEY_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/solana-testnet-keypair/private_key"
    },
    # Legacy Solana wallet (fallback for backward compatibility)
    {
      name      = "SOLANA_PRIVATE_KEY"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/solana-keypair/private_key"
    },

    # NEAR wallets (network-specific with account IDs)
    {
      name      = "NEAR_PRIVATE_KEY_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/near-mainnet-keypair/private_key"
    },
    {
      name      = "NEAR_ACCOUNT_ID_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/near-mainnet-keypair/account_id"
    },
    {
      name      = "NEAR_PRIVATE_KEY_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/near-testnet-keypair/private_key"
    },
    {
      name      = "NEAR_ACCOUNT_ID_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/near-testnet-keypair/account_id"
    },

    # Stellar wallets (network-specific, plain string format)
    {
      name      = "STELLAR_PRIVATE_KEY_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stellar-keypair-mainnet"
    },
    {
      name      = "STELLAR_PRIVATE_KEY_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stellar-keypair-testnet"
    },

    # Sui wallets (network-specific, plain string format - bech32 suiprivkey)
    {
      name      = "SUI_PRIVATE_KEY_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/sui-keypair-mainnet"
    },
    {
      name      = "SUI_PRIVATE_KEY_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/sui-keypair-testnet"
    },

    # Algorand wallets (network-specific, 25-word mnemonic format)
    {
      name      = "ALGORAND_MNEMONIC_MAINNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/algorand-mnemonic-mainnet"
    },
    {
      name      = "ALGORAND_MNEMONIC_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/algorand-mnemonic-testnet"
    },
  ]

  # ----------------------------------------------------------------------------
  # Mainnet RPC URLs (copies of the fields of the facilitator-rpc-mainnet secret)
  # ----------------------------------------------------------------------------
  # Current networks with premium mainnet RPCs:
  # - base, avalanche, polygon, optimism, celo, hyperevm, ethereum, arbitrum, unichain, solana, near
  # - arc, only while arc_mainnet_enabled: `local.arc_rpc_secrets` in arc.tf
  mainnet_rpc_secrets = [
    # EVM Networks
    {
      name      = "RPC_URL_BASE"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/base"
    },
    {
      name      = "RPC_URL_AVALANCHE"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/avalanche"
    },
    {
      name      = "RPC_URL_POLYGON"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/polygon"
    },
    {
      name      = "RPC_URL_OPTIMISM"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/optimism"
    },
    {
      name      = "RPC_URL_CELO"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/celo"
    },
    {
      name      = "RPC_URL_HYPEREVM"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/hyperevm"
    },
    {
      name      = "RPC_URL_ETHEREUM"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/ethereum"
    },
    {
      name      = "RPC_URL_ARBITRUM"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/arbitrum"
    },
    {
      name      = "RPC_URL_UNICHAIN"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/unichain"
    },

    # Non-EVM Networks
    {
      name      = "RPC_URL_SOLANA"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/solana"
    },
    {
      name      = "RPC_URL_NEAR"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-mainnet/near"
    },
  ]

  # ----------------------------------------------------------------------------
  # Testnet RPC URLs (copies of the fields of the facilitator-rpc-testnet secret)
  # ----------------------------------------------------------------------------
  # Current networks with premium testnet RPCs:
  # - solana-devnet, arbitrum-sepolia, near (testnet)
  testnet_rpc_secrets = [
    {
      name      = "RPC_URL_SOLANA_DEVNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-testnet/solana-devnet"
    },
    {
      name      = "RPC_URL_ARBITRUM_SEPOLIA"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-testnet/arbitrum-sepolia"
    },
    {
      name      = "RPC_URL_NEAR_TESTNET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/rpc-testnet/near"
    },
  ]

  # ----------------------------------------------------------------------------
  # Admin credentials
  # ----------------------------------------------------------------------------
  admin_secrets = concat([
    {
      name      = "ERC8004_ADMIN_TOKEN"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/erc8004-admin-token/token"
    },
    {
      # Created in SSM from the start (above), never a Secrets Manager copy.
      name      = "BAZAAR_ADMIN_TOKEN"
      valueFrom = local.bazaar_admin_token_parameter_arn
    },
    ], var.enable_dx402 ? [
    {
      name      = "DX402_SIGNING_KEY"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/dx402-signing-key/private_key"
    },
    {
      name      = "DX402_ADMIN_TOKEN"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/dx402-admin-token/token"
    },
    ] : [], var.enable_dx402 && var.dx402_storage_backend == "ipfs" ? [
    {
      # The JWT alone -- it is what the v3 upload API takes, and its payload
      # already embeds the api key and secret, so nothing else needs to travel.
      name      = "DX402_PINATA_JWT"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/dx402-pinata/jwt"
    },
  ] : [])

  # ----------------------------------------------------------------------------
  # Stack key digests
  # ----------------------------------------------------------------------------
  # The names are what src/rate_policy.rs reads: ENV_STACK_KEY_SHA256_PREFIX
  # followed by the service name upper-cased, '-' as '_' (env_var_for), one per
  # entry of DEFAULT_STACK_SERVICES. A name that drifts from that rule is never
  # read, and the service stays a third party with no error anywhere.
  stack_key_digest_secrets = concat([
    {
      name      = "UVD_STACK_KEY_SHA256_EXECUTION_MARKET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stack-key-digest-execution-market/sha256"
    },
    {
      name      = "UVD_STACK_KEY_SHA256_KARMAKADABRA"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stack-key-digest-karmakadabra/sha256"
    },
    {
      name      = "UVD_STACK_KEY_SHA256_DESCRIBE_NET"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stack-key-digest-describe-net/sha256"
    },
    {
      name      = "UVD_STACK_KEY_SHA256_MESHRELAY"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stack-key-digest-meshrelay/sha256"
    },
    ], var.stack_key_emporium_loaded ? [
    {
      # Like the four above, read from SSM: before setting stack_key_emporium_loaded,
      # put the digest in /facilitator/stack-key-digest-emporium/sha256 as well
      # (variables.tf). That parameter does not exist yet.
      name      = "UVD_STACK_KEY_SHA256_EMPORIUM"
      valueFrom = "${local.facilitator_parameter_arn_prefix}/stack-key-digest-emporium/sha256"
    },
  ] : [])

  # Combined secrets array for task definition
  all_task_secrets = concat(
    local.receipt_secrets,
    local.hedera_secrets,
    local.wallet_secrets,
    local.mainnet_rpc_secrets,
    local.arc_rpc_secrets,
    local.testnet_rpc_secrets,
    local.admin_secrets,
    local.stack_key_digest_secrets
  )
}
