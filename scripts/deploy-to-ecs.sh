#!/bin/bash
# =============================================================================
# x402 Facilitator AWS ECS Deployment Script
# Version: v1.3.11 - BSC Support Deployment
# =============================================================================
#
# Purpose: Build, push, and deploy facilitator to AWS ECS
# Usage: ./scripts/deploy-to-ecs.sh [version]
#
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warning() { echo -e "${YELLOW}[WARNING]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Configuration
VERSION="${1:-v1.3.11}"
AWS_REGION="us-east-2"
AWS_ACCOUNT_ID="518898403364"
ECR_REPOSITORY="facilitator"
CLUSTER_NAME="facilitator-production"
SERVICE_NAME="facilitator-production"
TASK_FAMILY="facilitator-production"

# Derived values
ECR_URI="${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${ECR_REPOSITORY}"
IMAGE_TAG="${VERSION}"

echo ""
log_info "╔═══════════════════════════════════════════════════════════════╗"
log_info "║    x402 Facilitator AWS ECS Deployment - ${VERSION}         ║"
log_info "║    BSC Mainnet + Testnet Support with BUSD Token             ║"
log_info "╚═══════════════════════════════════════════════════════════════╝"
echo ""

# =============================================================================
# Phase 1: Build Docker Image
# =============================================================================

log_info "═══ Phase 1: Building Docker Image ═══"
log_info "Platform: linux/amd64"
log_info "Version: ${VERSION}"
echo ""

docker build \
  --platform linux/amd64 \
  --build-arg FACILITATOR_VERSION=${VERSION} \
  -t ${ECR_REPOSITORY}:${IMAGE_TAG} \
  .

log_success "Docker image built successfully"
echo ""

# =============================================================================
# Phase 2: Tag and Push to ECR
# =============================================================================

log_info "═══ Phase 2: Push to Amazon ECR ═══"
log_info "Repository: ${ECR_URI}"
echo ""

# Tag for ECR
log_info "Tagging image for ECR..."
docker tag ${ECR_REPOSITORY}:${IMAGE_TAG} ${ECR_URI}:${IMAGE_TAG}
docker tag ${ECR_REPOSITORY}:${IMAGE_TAG} ${ECR_URI}:latest

# Login to ECR
log_info "Authenticating with ECR..."
aws ecr get-login-password --region ${AWS_REGION} | \
  docker login --username AWS --password-stdin ${ECR_URI}

# Push to ECR
log_info "Pushing image ${IMAGE_TAG}..."
docker push ${ECR_URI}:${IMAGE_TAG}

log_info "Pushing image latest..."
docker push ${ECR_URI}:latest

log_success "Image pushed to ECR successfully"
echo ""

# =============================================================================
# Phase 3: Update ECS Task Definition
# =============================================================================

log_info "═══ Phase 3: Update ECS Task Definition ═══"
echo ""

# Get current task definition
log_info "Fetching current task definition..."
aws ecs describe-task-definition \
  --task-definition ${TASK_FAMILY} \
  --region ${AWS_REGION} \
  --query 'taskDefinition' \
  > task-def-base.json

# Clean task definition (remove read-only fields)
log_info "Cleaning task definition..."
cat task-def-base.json | jq 'del(
  .taskDefinitionArn,
  .revision,
  .status,
  .requiresAttributes,
  .placementConstraints,
  .compatibilities,
  .registeredAt,
  .registeredBy
)' > task-def-clean.json

# Update image version
log_info "Updating container image to ${VERSION}..."
cat task-def-clean.json | jq \
  --arg IMAGE "${ECR_URI}:${IMAGE_TAG}" \
  '.containerDefinitions[0].image = $IMAGE' \
  > task-def-updated.json

# Register new task definition
log_info "Registering new task definition..."
REVISION=$(aws ecs register-task-definition \
  --cli-input-json file://task-def-updated.json \
  --region ${AWS_REGION} \
  --query 'taskDefinition.revision' \
  --output text)

log_success "New task definition registered: ${TASK_FAMILY}:${REVISION}"
echo ""

# Cleanup temporary files
rm -f task-def-base.json task-def-clean.json task-def-updated.json

# =============================================================================
# Phase 4: Deploy to ECS
# =============================================================================

log_info "═══ Phase 4: Deploy to Production ECS ═══"
log_info "Cluster: ${CLUSTER_NAME}"
log_info "Service: ${SERVICE_NAME}"
log_info "Task Definition: ${TASK_FAMILY}:${REVISION}"
echo ""

aws ecs update-service \
  --cluster ${CLUSTER_NAME} \
  --service ${SERVICE_NAME} \
  --task-definition ${TASK_FAMILY}:${REVISION} \
  --force-new-deployment \
  --region ${AWS_REGION} \
  --query 'service.{serviceName:serviceName,status:status,desiredCount:desiredCount}' \
  --output table

log_success "Deployment initiated"
echo ""

# Wait for deployment to start
log_info "Waiting 60 seconds for deployment to start..."
sleep 60

# =============================================================================
# Phase 5: Verify Deployment
# =============================================================================

log_info "═══ Phase 5: Verify Deployment ═══"
echo ""

# Check deployment status
log_info "Checking deployment status..."
aws ecs describe-services \
  --cluster ${CLUSTER_NAME} \
  --services ${SERVICE_NAME} \
  --region ${AWS_REGION} \
  --query 'services[0].deployments[*].{Status:status,Running:runningCount,Desired:desiredCount,RolloutState:rolloutState}' \
  --output table

echo ""

# Check version endpoint
log_info "Checking version endpoint..."
SITE_VERSION=$(curl -s https://facilitator.ultravioletadao.xyz/version || echo "ERROR")
echo "Site version: ${SITE_VERSION}"
echo ""

# Check health
log_info "Checking health endpoint..."
curl -s https://facilitator.ultravioletadao.xyz/health | jq '.'
echo ""

# Check BSC networks
log_info "Verifying BSC network support..."
BSC_NETWORKS=$(curl -s https://facilitator.ultravioletadao.xyz/supported | \
  jq -r '.kinds[] | select(.network | contains("bsc")) | "\(.network) - \(.scheme)"' || echo "ERROR")

if [ -z "$BSC_NETWORKS" ]; then
  log_error "BSC networks not found in /supported endpoint!"
  echo ""
  log_warning "Showing all networks:"
  curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network'
else
  log_success "BSC networks detected:"
  echo "${BSC_NETWORKS}"
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
log_success "DEPLOYMENT COMPLETE"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "Version: ${VERSION}"
echo "Site: https://facilitator.ultravioletadao.xyz"
echo "Features: BSC mainnet + testnet with BUSD token support"
echo ""
log_info "Monitor deployment progress:"
echo "  aws ecs describe-services --cluster ${CLUSTER_NAME} --services ${SERVICE_NAME} --region ${AWS_REGION}"
echo ""
log_info "View logs:"
echo "  aws logs tail /ecs/facilitator-production --follow --region ${AWS_REGION}"
echo ""
