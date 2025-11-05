# Build and Push Guide

This guide explains how to build Docker images and push them to AWS ECR for deployment to ECS Fargate.

## Prerequisites

1. **Docker** installed and running
   - Linux/Mac: `docker --version`
   - Windows: Docker Desktop running

2. **AWS CLI** configured with credentials
   - Run: `aws sts get-caller-identity`
   - Should show your AWS account ID: 518898403364

3. **Dockerfile** for agents
   - Create `Dockerfile.agent` in the project root
   - The script expects this Dockerfile to accept `AGENT_NAME` build arg

## Quick Start

### Linux/Mac/WSL

```bash
cd /path/to/karmacadabra/terraform/ecs-fargate
bash build-and-push.sh
```

### Windows PowerShell

```powershell
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate
.\build-and-push.ps1
```

### Windows Command Prompt

```cmd
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate
build-and-push.bat
```

## What the Script Does

The script performs three main steps:

### 1. ECR Login (Step 1/3)
```bash
aws ecr get-login-password --region us-east-2 | \
    docker login --username AWS --password-stdin \
    518898403364.dkr.ecr.us-east-2.amazonaws.com
```

### 2. Build Images (Step 2/3)
For each agent (validator, karma-hello, abracadabra, skill-extractor, voice-extractor):
```bash
docker build \
    -f Dockerfile.agent \
    -t karmacadabra/<agent-name>:latest \
    --build-arg AGENT_NAME=<agent-name> \
    .
```

### 3. Tag and Push (Step 3/3)
For each agent:
```bash
docker tag karmacadabra/<agent-name>:latest \
    518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/<agent-name>:latest

docker push \
    518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/<agent-name>:latest
```

## ECR Repository URLs

After pushing, images will be available at:

- **Validator**: `518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/validator:latest`
- **Karma-Hello**: `518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/karma-hello:latest`
- **Abracadabra**: `518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/abracadabra:latest`
- **Skill-Extractor**: `518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/skill-extractor:latest`
- **Voice-Extractor**: `518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/voice-extractor:latest`

## Force New Deployment

After pushing new images, force ECS to redeploy with updated images:

```bash
# Deploy single agent
aws ecs update-service \
    --cluster facilitator-production \
    --service facilitator-production \
    --force-new-deployment \
    --region us-east-2

# Deploy all agents
for agent in validator karma-hello abracadabra skill-extractor voice-extractor; do
    aws ecs update-service \
        --cluster facilitator-production \
        --service facilitator-production-$agent \
        --force-new-deployment \
        --region us-east-2
done
```

## Monitor Deployment

### Check Service Status
```bash
aws ecs describe-services \
    --cluster facilitator-production \
    --services facilitator-production \
    --region us-east-2
```

### View Running Tasks
```bash
aws ecs list-tasks \
    --cluster facilitator-production \
    --service-name facilitator-production \
    --region us-east-2
```

### View Logs
```bash
# Stream logs for specific agent
aws logs tail /ecs/facilitator-production/facilitator --follow --region us-east-2

# View recent logs
aws logs tail /ecs/facilitator-production/facilitator --since 1h --region us-east-2
```

## Troubleshooting

### Docker not running
**Error**: `Cannot connect to the Docker daemon`

**Solution**: Start Docker Desktop (Windows) or Docker daemon (Linux)

### ECR login fails
**Error**: `error getting credentials`

**Solution**:
```bash
# Check AWS credentials
aws sts get-caller-identity

# Re-configure if needed
aws configure
```

### Build fails
**Error**: `Dockerfile not found`

**Solution**: Ensure you're running from the correct directory and `Dockerfile.agent` exists in the project root

### Push fails with "denied"
**Error**: `denied: User is not authorized`

**Solution**:
```bash
# Verify ECR permissions
aws ecr describe-repositories --repository-names karmacadabra/validator

# If repositories don't exist, run terraform first
cd terraform/ecs-fargate
terraform apply
```

### Image size too large
**Warning**: Large images increase pull time and costs

**Solution**:
- Use multi-stage builds
- Use `.dockerignore` file
- Minimize layers
- Use Alpine-based images when possible

## Advanced Usage

### Build Single Agent

```bash
# Linux/Mac
AGENT_NAME="validator"
docker build -f Dockerfile.agent -t karmacadabra/$AGENT_NAME:latest --build-arg AGENT_NAME=$AGENT_NAME .
docker tag karmacadabra/$AGENT_NAME:latest 518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/$AGENT_NAME:latest
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/$AGENT_NAME:latest
```

### Build with Version Tag

```bash
VERSION="v1.0.0"
AGENT_NAME="validator"

docker build -f Dockerfile.agent -t karmacadabra/$AGENT_NAME:$VERSION .
docker tag karmacadabra/$AGENT_NAME:$VERSION \
    518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/$AGENT_NAME:$VERSION
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/$AGENT_NAME:$VERSION
```

### Clean Up Local Images

```bash
# Remove all karmacadabra images
docker rmi $(docker images 'karmacadabra/*' -q)

# Remove all ECR-tagged images
docker rmi $(docker images '518898403364.dkr.ecr.us-east-2.amazonaws.com/karmacadabra/*' -q)
```

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Build and Push to ECR

on:
  push:
    branches: [main]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Configure AWS credentials
        uses: aws-actions/configure-aws-credentials@v2
        with:
          aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
          aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
          aws-region: us-east-2

      - name: Build and push
        run: bash terraform/ecs-fargate/build-and-push.sh
```

## Cost Optimization

**ECR Storage Costs**:
- $0.10/GB per month
- Lifecycle policies configured to keep only:
  - Latest 5 images
  - Images from last 30 days

**Build Optimization**:
- Use Docker BuildKit for faster builds
- Enable layer caching
- Build only changed agents

```bash
# Enable BuildKit
export DOCKER_BUILDKIT=1

# Build with cache
docker build --cache-from karmacadabra/validator:latest ...
```

## Next Steps

After successful push:
1. ✅ Images are in ECR
2. ⏭️ Force new deployment (see above)
3. ⏭️ Monitor deployment status
4. ⏭️ Test health endpoints
5. ⏭️ View logs for any issues

For infrastructure changes, see `terraform/ecs-fargate/README.md`
