#!/bin/bash
# Build and push facilitator Docker image to AWS ECR

set -e

# Configuration
AWS_REGION="${AWS_REGION:-us-east-2}"
AWS_ACCOUNT_ID="${AWS_ACCOUNT_ID:-518898403364}"
ECR_REPOSITORY="facilitator"
IMAGE_TAG="${1:-latest}"

echo "🐳 Building facilitator Docker image..."
echo "   Tag: ${IMAGE_TAG}"

# Build image with version information
docker build --build-arg FACILITATOR_VERSION=${IMAGE_TAG} -t ${ECR_REPOSITORY}:${IMAGE_TAG} .

# Tag for ECR
docker tag ${ECR_REPOSITORY}:${IMAGE_TAG} \
  ${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${ECR_REPOSITORY}:${IMAGE_TAG}

# Also tag as latest if this isn't already latest
if [ "${IMAGE_TAG}" != "latest" ]; then
  docker tag ${ECR_REPOSITORY}:${IMAGE_TAG} \
    ${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${ECR_REPOSITORY}:latest
fi

echo "🔐 Logging in to AWS ECR..."
aws ecr get-login-password --region ${AWS_REGION} | \
  docker login --username AWS --password-stdin \
  ${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com

echo "📤 Pushing image to ECR..."
docker push ${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${ECR_REPOSITORY}:${IMAGE_TAG}

if [ "${IMAGE_TAG}" != "latest" ]; then
  docker push ${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${ECR_REPOSITORY}:latest
fi

echo "✅ Image pushed successfully!"
echo "   Repository: ${ECR_REPOSITORY}"
echo "   Tags: ${IMAGE_TAG}, latest"
echo ""
echo "To deploy to ECS:"
echo "  aws ecs update-service --cluster facilitator-prod --service facilitator-prod --force-new-deployment"
