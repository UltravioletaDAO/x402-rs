#!/usr/bin/env bash
#
# Create the DX402 receipt-signing key in AWS Secrets Manager.
#
# Run this ONCE, before setting enable_dx402 = true in terraform.tfvars. The
# bucket, table and IAM policies exist regardless; this is the last missing
# prerequisite.
#
#   ./scripts/dx402-bootstrap-secret.sh
#
# What the key is, and what it is not:
#
#   IS      the key the facilitator signs EIP-712 EvidenceReceipts with, so a
#           third party can verify offline -- without calling us -- that we
#           attested a given payment produced a given piece of evidence.
#
#   IS NOT  a payment wallet. It signs attestations, never transfers. It holds
#           no funds and needs no gas. Keeping it separate from the facilitator
#           wallets means a leak forges receipts but moves no money, and
#           rotating it costs nothing but a note that receipts before date X
#           verify against the old address.
#
# The key is generated LOCALLY and never leaves this machine except into Secrets
# Manager. It is not printed. Only the derived public address is shown, which is
# what integrators check receipts against.

set -euo pipefail

SECRET_NAME="facilitator-dx402-signing-key"
REGION="${AWS_REGION:-us-east-2}"

echo "==> DX402 receipt-signing key bootstrap"
echo "    secret : ${SECRET_NAME}"
echo "    region : ${REGION}"
echo

if ! command -v aws >/dev/null 2>&1; then
  echo "ERROR: the aws CLI is not on PATH." >&2
  exit 1
fi

if ! aws sts get-caller-identity --region "$REGION" >/dev/null 2>&1; then
  echo "ERROR: no usable AWS credentials for ${REGION}." >&2
  echo "       Run 'aws sso login' (or set your profile) and try again." >&2
  exit 1
fi

# Refuse to clobber. Overwriting this key silently invalidates every receipt
# already issued -- they would still verify, but against an address nobody
# publishes any more.
if aws secretsmanager describe-secret \
  --secret-id "$SECRET_NAME" --region "$REGION" >/dev/null 2>&1; then
  echo "Secret ${SECRET_NAME} already exists. Nothing to do."
  echo
  echo "To ROTATE it deliberately (this does not invalidate old receipts, but"
  echo "they will verify against the previous address):"
  echo "  aws secretsmanager put-secret-value --secret-id ${SECRET_NAME} \\"
  echo "    --region ${REGION} --secret-string '{\"private_key\":\"0x...\"}'"
  exit 0
fi

# 32 random bytes from the OS CSPRNG. `openssl rand` is used rather than
# /dev/urandom + shell mangling so the bytes reach hex without ever touching a
# shell variable that could end up in history.
if ! command -v openssl >/dev/null 2>&1; then
  echo "ERROR: openssl is required to generate the key." >&2
  exit 1
fi

PRIVATE_KEY="0x$(openssl rand -hex 32)"

echo "==> Creating secret..."
aws secretsmanager create-secret \
  --name "$SECRET_NAME" \
  --description "DX402 EIP-712 evidence receipt signing key. Attestations only - holds no funds, needs no gas." \
  --secret-string "{\"private_key\":\"${PRIVATE_KEY}\"}" \
  --region "$REGION" \
  --output text --query 'ARN'

echo
echo "==> Done."
echo
echo "The RECEIPT SIGNER ADDRESS is what integrators verify against. Read it"
echo "from the running facilitator once DX402 is on:"
echo
echo "  curl -s https://facilitator.ultravioletadao.xyz/dx402/stats | jq -r .receiptSigner"
echo
echo "Next steps:"
echo "  1. set  enable_dx402 = true  in terraform/environments/production/terraform.tfvars"
echo "  2. terraform apply -target=aws_ecs_task_definition.facilitator \\"
echo "                     -target=aws_ecs_service.facilitator"
echo "  3. verify: curl -s https://facilitator.ultravioletadao.xyz/supported \\"
echo "               | jq '.extensions'   # expect \"durable-evidence\""
