#!/bin/bash
# ============================================================================
# Build and Push Docker Images to ECR
# ============================================================================
# This script builds Docker images for all Karmacadabra agents and pushes
# them to AWS ECR repositories.
#
# Prerequisites:
# - AWS CLI configured with appropriate credentials
# - Docker installed and running
# - Run from karmacadabra root directory

set -e  # Exit on error

# Configuration
AWS_REGION="us-east-1"
AWS_ACCOUNT_ID="518898403364"
ECR_BASE_URL="${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com"
PROJECT_NAME="karmacadabra"

# Agent definitions (name:dockerfile:context)
# Format: "agent_name:path/to/Dockerfile:build/context/path"
AGENTS=(
    "validator:Dockerfile.agent:."
    "karma-hello:Dockerfile.agent:."
    "abracadabra:Dockerfile.agent:."
    "skill-extractor:Dockerfile.agent:."
    "voice-extractor:Dockerfile.agent:."
)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo "========================================="
echo "Karmacadabra ECR Build and Push"
echo "========================================="
echo ""

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
    echo -e "${RED}ERROR: Docker is not running. Please start Docker and try again.${NC}"
    exit 1
fi

# Navigate to project root (assuming script is in terraform/ecs-fargate/)
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$( cd "$SCRIPT_DIR/../.." && pwd )"
cd "$PROJECT_ROOT"

echo -e "${BLUE}Working directory: $(pwd)${NC}"
echo ""

# Step 1: Login to ECR
echo -e "${YELLOW}[1/3] Logging in to ECR...${NC}"
aws ecr get-login-password --region $AWS_REGION | \
    docker login --username AWS --password-stdin $ECR_BASE_URL

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Successfully logged in to ECR${NC}"
else
    echo -e "${RED}✗ Failed to login to ECR${NC}"
    exit 1
fi
echo ""

# Step 2: Build images
echo -e "${YELLOW}[2/3] Building Docker images...${NC}"
for agent_config in "${AGENTS[@]}"; do
    IFS=':' read -r agent_name dockerfile context <<< "$agent_config"

    echo -e "${BLUE}Building ${agent_name}...${NC}"

    # Build the image
    docker build \
        -f "$dockerfile" \
        -t "${PROJECT_NAME}/${agent_name}:latest" \
        --build-arg AGENT_NAME="${agent_name}" \
        "$context"

    if [ $? -eq 0 ]; then
        echo -e "${GREEN}✓ Built ${agent_name}${NC}"
    else
        echo -e "${RED}✗ Failed to build ${agent_name}${NC}"
        exit 1
    fi
    echo ""
done

# Step 3: Tag and push images
echo -e "${YELLOW}[3/3] Tagging and pushing images to ECR...${NC}"
for agent_config in "${AGENTS[@]}"; do
    IFS=':' read -r agent_name dockerfile context <<< "$agent_config"

    LOCAL_IMAGE="${PROJECT_NAME}/${agent_name}:latest"
    ECR_IMAGE="${ECR_BASE_URL}/${PROJECT_NAME}/${agent_name}:latest"

    echo -e "${BLUE}Pushing ${agent_name}...${NC}"

    # Tag for ECR
    docker tag "$LOCAL_IMAGE" "$ECR_IMAGE"

    # Push to ECR
    docker push "$ECR_IMAGE"

    if [ $? -eq 0 ]; then
        echo -e "${GREEN}✓ Pushed ${agent_name} to ECR${NC}"
        echo -e "   ${ECR_IMAGE}"
    else
        echo -e "${RED}✗ Failed to push ${agent_name}${NC}"
        exit 1
    fi
    echo ""
done

echo "========================================="
echo -e "${GREEN}All images successfully pushed to ECR!${NC}"
echo "========================================="
echo ""
echo "Next steps:"
echo "1. Force new deployment to use updated images:"
echo "   aws ecs update-service --cluster karmacadabra-prod --service karmacadabra-prod-validator --force-new-deployment"
echo ""
echo "2. Monitor deployment:"
echo "   aws ecs describe-services --cluster karmacadabra-prod --services karmacadabra-prod-validator"
echo ""
echo "3. View logs:"
echo "   aws logs tail /ecs/karmacadabra-prod/validator --follow"
echo ""
