# Facilitator Secrets Migration: Testnet vs Mainnet

## Overview

The facilitator now supports 4 networks and requires separate hot wallets for testnet and mainnet environments.

### Network Mapping

| Network | Type | Wallet |
|---------|------|--------|
| Avalanche Fuji | Testnet | `karmacadabra-facilitator-testnet` |
| Base Sepolia | Testnet | `karmacadabra-facilitator-testnet` |
| Avalanche Mainnet | Mainnet | `karmacadabra-facilitator-mainnet` |
| Base Mainnet | Mainnet | `karmacadabra-facilitator-mainnet` |

### Wallet Addresses

- **Testnet**: `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`
- **Mainnet**: `0x103040545AC5031A11E8C03dd11324C7333a13C7`

## Migration Status

### ✅ Completed

- [x] Created `karmacadabra-facilitator-mainnet` secret in AWS Secrets Manager (us-east-1)
- [x] Created `karmacadabra-facilitator-testnet` secret in AWS Secrets Manager (us-east-1)
- [x] Created migration scripts in `scripts/` directory
- [x] Updated facilitator tests to use correct wallet per network
- [x] Verified all 6 facilitator integration tests pass
- [x] Confirmed wallet balances on all 4 networks

### 🔄 Remaining Tasks

- [ ] Update Rust facilitator code to load network-specific secrets (when deploying)
- [ ] Update Terraform configuration (terraform/ecs-fargate/main.tf)
- [ ] Update Docker Compose configuration (docker-compose.yml)
- [ ] Delete old `karmacadabra-facilitator` secret (optional, 7-day recovery window)
- [ ] Apply same pattern to agent wallets (karma-hello, validator, etc.)

## Step-by-Step Migration

### Step 1: Create Testnet Secret (REQUIRED)

Run this command with your testnet private key:

```bash
# Set environment variable
export TESTNET_PRIVATE_KEY="0x..."

# Create secret
python scripts/create_testnet_facilitator_secret.py
```

The script will:
- Verify the key derives to `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`
- Create `karmacadabra-facilitator-testnet` in AWS Secrets Manager
- Confirm both testnet and mainnet secrets exist

### Step 2: Verify Secrets

```bash
aws secretsmanager list-secrets --region us-east-1 \
  --query 'SecretList[?contains(Name, `facilitator`)].{Name:Name, Address:Description}' \
  --output table
```

Expected output:
```
karmacadabra-facilitator-testnet  -> 0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8
karmacadabra-facilitator-mainnet  -> 0x103040545AC5031A11E8C03dd11324C7333a13C7
```

### Step 3: Update Facilitator Code (x402-rs)

The Rust facilitator needs to load the appropriate private key based on network. Two approaches:

#### Option A: Environment-based (Simplest)

Use different environment variables for testnet vs mainnet:

```bash
# For testnet deployments
EVM_PRIVATE_KEY_TESTNET=<from AWS Secrets Manager>

# For mainnet deployments
EVM_PRIVATE_KEY_MAINNET=<from AWS Secrets Manager>
```

#### Option B: Network-aware (Better)

Modify `x402-rs/src/network.rs` to map networks to secrets:

```rust
impl Network {
    pub fn facilitator_secret_name(&self) -> &'static str {
        match self {
            Network::AvalancheFuji | Network::BaseSepolia => {
                "karmacadabra-facilitator-testnet"
            }
            Network::Avalanche | Network::Base => {
                "karmacadabra-facilitator-mainnet"
            }
        }
    }
}
```

**Recommended**: Use Option B for production (network-aware secret loading).

### Step 4: Update Terraform

Modify `terraform/ecs-fargate/main.tf` to inject both secrets:

```hcl
# Add data sources for both secrets
data "aws_secretsmanager_secret" "facilitator_testnet" {
  name = "karmacadabra-facilitator-testnet"
}

data "aws_secretsmanager_secret" "facilitator_mainnet" {
  name = "karmacadabra-facilitator-mainnet"
}

# Update facilitator task definition secrets
secrets = [
  {
    name      = "EVM_PRIVATE_KEY_TESTNET"
    valueFrom = "${data.aws_secretsmanager_secret.facilitator_testnet.arn}:private_key::"
  },
  {
    name      = "EVM_PRIVATE_KEY_MAINNET"
    valueFrom = "${data.aws_secretsmanager_secret.facilitator_mainnet.arn}:private_key::"
  }
]
```

### Step 5: Update Docker Compose

Modify `docker-compose.yml` to load from AWS Secrets Manager:

```yaml
facilitator:
  environment:
    # Load testnet key for local development (Fuji + Base Sepolia)
    - EVM_PRIVATE_KEY=${EVM_PRIVATE_KEY_TESTNET}
  volumes:
    - ~/.aws:/root/.aws:ro  # For AWS CLI access to secrets
```

For production, modify startup script to fetch from AWS:

```bash
# In facilitator entrypoint
export EVM_PRIVATE_KEY_TESTNET=$(aws secretsmanager get-secret-value \
  --secret-id karmacadabra-facilitator-testnet \
  --region us-east-1 \
  --query 'SecretString' --output text | jq -r '.private_key')

export EVM_PRIVATE_KEY_MAINNET=$(aws secretsmanager get-secret-value \
  --secret-id karmacadabra-facilitator-mainnet \
  --region us-east-1 \
  --query 'SecretString' --output text | jq -r '.private_key')
```

### Step 6: Test All Networks

Run comprehensive tests:

```bash
python tests/test_facilitator.py
```

Expected: All 6 tests pass with correct wallets for each network.

### Step 7: Clean Up Old Secret

Once everything works, delete the old secret:

```bash
aws secretsmanager delete-secret \
  --secret-id karmacadabra-facilitator \
  --region us-east-1 \
  --recovery-window-in-days 7
```

## Security Notes

- ⚠️ **NEVER** commit private keys to git
- ⚠️ **NEVER** expose private keys in logs or environment variable listings
- ✅ Always use AWS Secrets Manager for production
- ✅ Rotate keys every 30-90 days
- ✅ Monitor wallet balances and set up alerts for low balance

## Rollback Plan

If issues arise:

1. The old `karmacadabra-facilitator` secret still exists (7-day recovery window)
2. Restore it: `aws secretsmanager restore-secret --secret-id karmacadabra-facilitator --region us-east-1`
3. Revert terraform/docker-compose changes
4. Re-deploy with old configuration

## Future: Agent Wallets

After facilitator migration is complete, apply the same pattern to agent wallets:

- `karmacadabra-karma-hello-testnet` + `karmacadabra-karma-hello-mainnet`
- `karmacadabra-validator-testnet` + `karmacadabra-validator-mainnet`
- `karmacadabra-skill-extractor-testnet` + `karmacadabra-skill-extractor-mainnet`
- `karmacadabra-voice-extractor-testnet` + `karmacadabra-voice-extractor-mainnet`
- `karmacadabra-abracadabra-testnet` + `karmacadabra-abracadabra-mainnet`
