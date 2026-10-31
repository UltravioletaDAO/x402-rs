# Owner's decision 266: the task definition reads every secret from an SSM Parameter Store
# SecureString under /facilitator/, and none from Secrets Manager (secrets.tf). Against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
#
# The table below is the live task definition's (revision 482, measured 2026-10-31): the same 45 names,
# each pointed at the parameter copied from the secret field it used to read. A name that
# disappears, a new one, or a parameter that moves goes red here. That each parameter holds the
# same value as its secret is not something a plan can see: scripts/verificar_ssm_igual.py.

mock_provider "aws" {
  # As in bazaar_admin_token.tftest.hcl: the task definition validates its role ARNs, and a mock
  # makes up random strings. No account id in either: this repo is public.
  mock_resource "aws_iam_role" {
    defaults = {
      arn = "arn:aws:iam:::role/mock"
    }
  }
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-2::certificate/mock"
    }
  }
}
mock_provider "archive" {}

# Setup, first, as in nat_guard.tftest.hcl: the teardown walks the whole configuration, and the
# certificate validation records take their for_each keys from the certificates.
run "the_certificates_exist" {
  command = apply

  plan_options {
    target = [aws_acm_certificate.main, aws_acm_certificate.metrics]
  }
}

run "every_secret_of_production_comes_from_its_ssm_copy" {
  command = apply

  # production.auto.tfvars, plus the defaults it relies on (enable_dx402, the ipfs backend,
  # no Emporium digest yet): every conditional secret production injects is on.
  variables {
    hedera_enabled_testnet    = true
    hedera_enabled_mainnet    = true
    arc_mainnet_enabled       = true
    enable_dx402              = true
    dx402_storage_backend     = "ipfs"
    stack_key_emporium_loaded = false
  }

  plan_options {
    target = [
      aws_ecs_task_definition.facilitator,
      aws_iam_role_policy.secrets_access,
    ]
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].secrets :
      startswith(s.valueFrom, "arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter/facilitator/")
    ])
    error_message = "Every secret of the task definition must be an SSM parameter under /facilitator/, by its ARN: none from Secrets Manager (decision 266)."
  }

  # Same names, each on the parameter its secret field was copied to. jsonencode sorts the keys,
  # so this compares the two tables entry by entry (and does not look at the order).
  assert {
    condition = jsonencode({
      for s in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].secrets :
      s.name => trimprefix(s.valueFrom, "arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter")
      }) == jsonencode({
      RECEIPT_SIGNING_KEY                   = "/facilitator/receipt-signing-key/private_key"
      HEDERA_PRIVATE_KEY_TESTNET            = "/facilitator/hedera-testnet-keypair/private_key"
      HEDERA_ACCOUNT_ID_TESTNET             = "/facilitator/hedera-testnet-keypair/account_id"
      HEDERA_PRIVATE_KEY_MAINNET            = "/facilitator/hedera-mainnet-keypair/private_key"
      HEDERA_ACCOUNT_ID_MAINNET             = "/facilitator/hedera-mainnet-keypair/account_id"
      EVM_PRIVATE_KEY_MAINNET               = "/facilitator/evm-mainnet-private-key/private_key"
      EVM_PRIVATE_KEY_TESTNET               = "/facilitator/evm-testnet-private-key/private_key"
      EVM_PRIVATE_KEY                       = "/facilitator/evm-private-key/private_key"
      SOLANA_PRIVATE_KEY_MAINNET            = "/facilitator/solana-mainnet-keypair/private_key"
      SOLANA_PRIVATE_KEY_TESTNET            = "/facilitator/solana-testnet-keypair/private_key"
      SOLANA_PRIVATE_KEY                    = "/facilitator/solana-keypair/private_key"
      NEAR_PRIVATE_KEY_MAINNET              = "/facilitator/near-mainnet-keypair/private_key"
      NEAR_ACCOUNT_ID_MAINNET               = "/facilitator/near-mainnet-keypair/account_id"
      NEAR_PRIVATE_KEY_TESTNET              = "/facilitator/near-testnet-keypair/private_key"
      NEAR_ACCOUNT_ID_TESTNET               = "/facilitator/near-testnet-keypair/account_id"
      STELLAR_PRIVATE_KEY_MAINNET           = "/facilitator/stellar-keypair-mainnet"
      STELLAR_PRIVATE_KEY_TESTNET           = "/facilitator/stellar-keypair-testnet"
      SUI_PRIVATE_KEY_MAINNET               = "/facilitator/sui-keypair-mainnet"
      SUI_PRIVATE_KEY_TESTNET               = "/facilitator/sui-keypair-testnet"
      ALGORAND_MNEMONIC_MAINNET             = "/facilitator/algorand-mnemonic-mainnet"
      ALGORAND_MNEMONIC_TESTNET             = "/facilitator/algorand-mnemonic-testnet"
      RPC_URL_BASE                          = "/facilitator/rpc-mainnet/base"
      RPC_URL_AVALANCHE                     = "/facilitator/rpc-mainnet/avalanche"
      RPC_URL_POLYGON                       = "/facilitator/rpc-mainnet/polygon"
      RPC_URL_OPTIMISM                      = "/facilitator/rpc-mainnet/optimism"
      RPC_URL_CELO                          = "/facilitator/rpc-mainnet/celo"
      RPC_URL_HYPEREVM                      = "/facilitator/rpc-mainnet/hyperevm"
      RPC_URL_ETHEREUM                      = "/facilitator/rpc-mainnet/ethereum"
      RPC_URL_ARBITRUM                      = "/facilitator/rpc-mainnet/arbitrum"
      RPC_URL_UNICHAIN                      = "/facilitator/rpc-mainnet/unichain"
      RPC_URL_SOLANA                        = "/facilitator/rpc-mainnet/solana"
      RPC_URL_NEAR                          = "/facilitator/rpc-mainnet/near"
      RPC_URL_ARC                           = "/facilitator/rpc-mainnet/arc"
      RPC_URL_SOLANA_DEVNET                 = "/facilitator/rpc-testnet/solana-devnet"
      RPC_URL_ARBITRUM_SEPOLIA              = "/facilitator/rpc-testnet/arbitrum-sepolia"
      RPC_URL_NEAR_TESTNET                  = "/facilitator/rpc-testnet/near"
      ERC8004_ADMIN_TOKEN                   = "/facilitator/erc8004-admin-token/token"
      BAZAAR_ADMIN_TOKEN                    = "/facilitator/bazaar-admin-token"
      DX402_SIGNING_KEY                     = "/facilitator/dx402-signing-key/private_key"
      DX402_ADMIN_TOKEN                     = "/facilitator/dx402-admin-token/token"
      DX402_PINATA_JWT                      = "/facilitator/dx402-pinata/jwt"
      UVD_STACK_KEY_SHA256_EXECUTION_MARKET = "/facilitator/stack-key-digest-execution-market/sha256"
      UVD_STACK_KEY_SHA256_KARMAKADABRA     = "/facilitator/stack-key-digest-karmakadabra/sha256"
      UVD_STACK_KEY_SHA256_DESCRIBE_NET     = "/facilitator/stack-key-digest-describe-net/sha256"
      UVD_STACK_KEY_SHA256_MESHRELAY        = "/facilitator/stack-key-digest-meshrelay/sha256"
    })
    error_message = "The task definition's secrets must be exactly the 45 names production injects, each on the /facilitator/ parameter copied from the secret field it read before decision 266."
  }

  # A name the `for` above would collapse: two entries for one variable.
  assert {
    condition     = length(jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].secrets) == 45
    error_message = "The task definition must carry 45 secrets, each name once."
  }

  # One SSM grant, on the whole /facilitator/ tree the parameters live in...
  assert {
    condition = length([
      for st in jsondecode(aws_iam_role_policy.secrets_access.policy).Statement : st
      if try(st.Effect, "") == "Allow" &&
      try(st.Action, null) == ["ssm:GetParameters"] &&
      try(st.Resource, null) == "arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter/facilitator/*"
    ]) == 1
    error_message = "The execution role must be granted ssm:GetParameters on /facilitator/*, once."
  }

  assert {
    condition = alltrue([
      for st in jsondecode(aws_iam_role_policy.secrets_access.policy).Statement :
      !anytrue([for a in flatten([try(st.Action, [])]) : startswith(a, "ssm:")]) ||
      try(st.Resource, null) == "arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter/facilitator/*"
    ])
    error_message = "No SSM grant of the execution role may reach outside /facilitator/."
  }

  # ...and the Secrets Manager grant stays, unchanged, until the task has run on the copies:
  # reverting the mapping is the rollback, and it needs this grant. The 27 secrets of
  # local.all_secret_arns with these variables (the live policy's count, 2026-10-22), counted
  # rather than compared with the local: 1.9.8 panics when an assertion reads a local the
  # targeted apply did not evaluate, and the mock makes the ARNs up anyway.
  assert {
    condition = length([
      for st in jsondecode(aws_iam_role_policy.secrets_access.policy).Statement : st
      if try(st.Effect, "") == "Allow" &&
      try(st.Action, null) == ["secretsmanager:GetSecretValue"] &&
      try(length(distinct(st.Resource)), 0) == 27
    ]) == 1
    error_message = "The execution role keeps secretsmanager:GetSecretValue on the 27 secrets of local.all_secret_arns until the rollback is no longer needed."
  }
}

run "the_emporium_digest_comes_from_its_parameter_once_loaded" {
  command = apply

  variables {
    stack_key_emporium_loaded = true
  }

  plan_options {
    target = [aws_ecs_task_definition.facilitator]
  }

  assert {
    condition = [
      for s in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].secrets :
      s.valueFrom if s.name == "UVD_STACK_KEY_SHA256_EMPORIUM"
    ] == ["arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter/facilitator/stack-key-digest-emporium/sha256"]
    error_message = "UVD_STACK_KEY_SHA256_EMPORIUM must come once, from /facilitator/stack-key-digest-emporium/sha256."
  }
}
