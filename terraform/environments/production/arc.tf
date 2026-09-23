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
  # Public endpoints only. A credential-bearing endpoint belongs in Secrets Manager.
  arc_rpc_environment = concat(
    var.arc_mainnet_enabled ? [{ name = "RPC_URL_ARC", value = "https://rpc.mainnet.arc.io" }] : [],
    var.arc_testnet_enabled ? [{ name = "RPC_URL_ARC_TESTNET", value = "https://rpc.testnet.arc.io" }] : [],
  )
}
