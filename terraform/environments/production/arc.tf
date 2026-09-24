# Arc activation is separate from shipping its code. Enable each network only
# after funding its signer and passing the canary in docs/networks/arc.md.
variable "arc_mainnet_enabled" {
  description = "Serve USDC exact payments on Arc mainnet (chain 5042)."
  type        = bool
  default     = false
}

variable "arc_testnet_enabled" {
  description = "Serve USDC exact payments on Arc testnet (chain 5042002)."
  type        = bool
  default     = false
}

variable "arc_minimum_gas_usdc" {
  description = "Operator override of the Arc low-balance alert, in native USDC (not wei or ETH). Null derives it like every other EVM chain (alerts.tf)."
  type        = number
  default     = null

  validation {
    condition     = var.arc_minimum_gas_usdc == null || coalesce(var.arc_minimum_gas_usdc, 1) > 0
    error_message = "Arc gas reserve must be positive."
  }
}

locals {
  # Arc mainnet settles through the `arc` key of facilitator-rpc-mainnet
  # (secrets.tf): the public rpc.mainnet.arc.io limits by IP and answers 429
  # without Retry-After, and this is the payment path. A URL that carries a key
  # is a `secrets` entry of the task definition, never an `environment` one, and
  # one variable cannot be both; `all_task_secrets` (secrets.tf) takes this.
  arc_rpc_secrets = var.arc_mainnet_enabled ? [{
    name      = "RPC_URL_ARC"
    valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:arc::"
  }] : []

  # Public endpoints only: Arc testnet has no premium endpoint.
  arc_rpc_environment = var.arc_testnet_enabled ? [{ name = "RPC_URL_ARC_TESTNET", value = "https://rpc.testnet.arc.io" }] : []

  # The balances Lambda only reads a balance on each chain, which the public
  # endpoints serve, and a Lambda has no `secrets` block: giving it the premium
  # URL would put the key in its plain environment. So it keeps the public ones.
  arc_balance_rpc_environment = merge(
    var.arc_mainnet_enabled ? { RPC_URL_ARC = "https://rpc.mainnet.arc.io" } : {},
    var.arc_testnet_enabled ? { RPC_URL_ARC_TESTNET = "https://rpc.testnet.arc.io" } : {},
  )
}
