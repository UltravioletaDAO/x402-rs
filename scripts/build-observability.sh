#!/bin/bash
# =============================================================================
# Build and Push Observability Stack Images to ECR
# x402-rs Payment Facilitator - Ultravioleta DAO
# =============================================================================
#
# Usage:
#   ./scripts/build-observability.sh [TAG]
#
# Examples:
#   ./scripts/build-observability.sh           # Uses "latest" tag
#   ./scripts/build-observability.sh v1.0.0    # Uses specific tag
#
# Prerequisites:
#   - AWS CLI configured with ECR permissions
#   - Docker running
#   - ECR repositories created (via Terraform)
#
# =============================================================================

set -euo pipefail

TAG="${1:-latest}"
REGION="us-east-2"
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
ECR_BASE="${ACCOUNT_ID}.dkr.ecr.${REGION}.amazonaws.com"

echo "============================================="
echo "Building observability images (tag: ${TAG})"
echo "ECR: ${ECR_BASE}"
echo "============================================="

# Login to ECR
echo "[1/5] Logging in to ECR..."
aws ecr get-login-password --region "${REGION}" | \
  docker login --username AWS --password-stdin "${ECR_BASE}"

# Build all 4 images from the project root
cd "$(dirname "$0")/.."
PROJECT_ROOT="$(pwd)"

echo "[2/5] Building OTel Collector..."
docker build -f docker/Dockerfile.otel-collector -t "${ECR_BASE}/facilitator-otel-collector:${TAG}" "${PROJECT_ROOT}"

echo "[3/5] Building Prometheus..."
docker build -f docker/Dockerfile.prometheus -t "${ECR_BASE}/facilitator-prometheus:${TAG}" "${PROJECT_ROOT}"

echo "[4/5] Building Tempo..."
docker build -f docker/Dockerfile.tempo -t "${ECR_BASE}/facilitator-tempo:${TAG}" "${PROJECT_ROOT}"

echo "[5/5] Building Grafana..."
docker build -f docker/Dockerfile.grafana -t "${ECR_BASE}/facilitator-grafana:${TAG}" "${PROJECT_ROOT}"

echo ""
echo "============================================="
echo "Pushing images to ECR..."
echo "============================================="

docker push "${ECR_BASE}/facilitator-otel-collector:${TAG}"
docker push "${ECR_BASE}/facilitator-prometheus:${TAG}"
docker push "${ECR_BASE}/facilitator-tempo:${TAG}"
docker push "${ECR_BASE}/facilitator-grafana:${TAG}"

echo ""
echo "============================================="
echo "[OK] All observability images pushed to ECR"
echo ""
echo "Images:"
echo "  ${ECR_BASE}/facilitator-otel-collector:${TAG}"
echo "  ${ECR_BASE}/facilitator-prometheus:${TAG}"
echo "  ${ECR_BASE}/facilitator-tempo:${TAG}"
echo "  ${ECR_BASE}/facilitator-grafana:${TAG}"
echo ""
echo "Next steps:"
echo "  1. cd terraform/environments/production"
echo "  2. terraform plan -out=observability.tfplan"
echo "  3. terraform apply observability.tfplan"
echo "  4. aws ecs update-service --cluster facilitator-production \\"
echo "       --service facilitator-production --force-new-deployment --region ${REGION}"
echo "============================================="
