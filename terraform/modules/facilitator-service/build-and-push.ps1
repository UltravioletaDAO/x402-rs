# ============================================================================
# Build and Push Docker Images to ECR (PowerShell)
# ============================================================================
# This script builds Docker images for all Karmacadabra agents and pushes
# them to AWS ECR repositories.
#
# Prerequisites:
# - AWS CLI configured with appropriate credentials
# - Docker Desktop installed and running
# - Run from karmacadabra root directory
#
# Usage: .\build-and-push.ps1

# Stop on errors
$ErrorActionPreference = "Stop"

# Configuration
$AWS_REGION = "us-east-1"
$AWS_ACCOUNT_ID = "518898403364"
$ECR_BASE_URL = "$AWS_ACCOUNT_ID.dkr.ecr.$AWS_REGION.amazonaws.com"
$PROJECT_NAME = "karmacadabra"

# Agent definitions
# validator is at root level, others are in agents/ subdirectory
$AGENTS = @(
    @{Name = "validator"; Dockerfile = "Dockerfile.agent"; Context = "."; Path = "validator" },
    @{Name = "karma-hello"; Dockerfile = "Dockerfile.agent"; Context = "."; Path = "agents/karma-hello" },
    @{Name = "abracadabra"; Dockerfile = "Dockerfile.agent"; Context = "."; Path = "agents/abracadabra" },
    @{Name = "skill-extractor"; Dockerfile = "Dockerfile.agent"; Context = "."; Path = "agents/skill-extractor" },
    @{Name = "voice-extractor"; Dockerfile = "Dockerfile.agent"; Context = "."; Path = "agents/voice-extractor" }
)

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "Karmacadabra ECR Build and Push" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

# Check if Docker is running
Write-Host "Checking Docker..." -ForegroundColor Blue
$dockerRunning = $null
try {
    $dockerRunning = docker info 2>&1
}
catch {
    # Docker not running
}

if (-not $dockerRunning -or $LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Docker is not running. Please start Docker Desktop and try again." -ForegroundColor Red
    exit 1
}
Write-Host "Docker is running" -ForegroundColor Green
Write-Host ""

# Navigate to project root
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Resolve-Path (Join-Path $ScriptDir "..\..")
Set-Location $ProjectRoot

Write-Host "Working directory: $ProjectRoot" -ForegroundColor Blue
Write-Host ""

# Step 1: Login to ECR
Write-Host "[1/3] Logging in to ECR..." -ForegroundColor Yellow
$loginPassword = aws ecr get-login-password --region $AWS_REGION
if ($LASTEXITCODE -ne 0) {
    Write-Host "Failed to get ECR login password" -ForegroundColor Red
    exit 1
}

$loginPassword | docker login --username AWS --password-stdin $ECR_BASE_URL 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0) {
    Write-Host "Successfully logged in to ECR" -ForegroundColor Green
}
else {
    Write-Host "Failed to login to ECR" -ForegroundColor Red
    exit 1
}
Write-Host ""

# Step 2: Build images
Write-Host "[2/3] Building Docker images..." -ForegroundColor Yellow
foreach ($agent in $AGENTS) {
    $agentName = $agent.Name
    $dockerfile = $agent.Dockerfile
    $context = $agent.Context

    Write-Host "Building $agentName..." -ForegroundColor Blue

    $agentPath = $agent.Path

    # Build the image
    docker build -f $dockerfile -t "${PROJECT_NAME}/${agentName}:latest" --build-arg AGENT_NAME=$agentName --build-arg AGENT_PATH=$agentPath $context

    if ($LASTEXITCODE -eq 0) {
        Write-Host "Built $agentName successfully" -ForegroundColor Green
    }
    else {
        Write-Host "Failed to build $agentName" -ForegroundColor Red
        exit 1
    }
    Write-Host ""
}

# Step 3: Tag and push images
Write-Host "[3/3] Tagging and pushing images to ECR..." -ForegroundColor Yellow
foreach ($agent in $AGENTS) {
    $agentName = $agent.Name
    $localImage = "${PROJECT_NAME}/${agentName}:latest"
    $ecrImage = "${ECR_BASE_URL}/${PROJECT_NAME}/${agentName}:latest"

    Write-Host "Pushing $agentName..." -ForegroundColor Blue

    # Tag for ECR
    docker tag $localImage $ecrImage
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to tag $agentName" -ForegroundColor Red
        exit 1
    }

    # Push to ECR
    docker push $ecrImage
    if ($LASTEXITCODE -eq 0) {
        Write-Host "Pushed $agentName to ECR successfully" -ForegroundColor Green
        Write-Host "  $ecrImage" -ForegroundColor Gray
    }
    else {
        Write-Host "Failed to push $agentName" -ForegroundColor Red
        exit 1
    }
    Write-Host ""
}

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "All images successfully pushed to ECR!" -ForegroundColor Green
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host ""
Write-Host "1. Force new deployment to use updated images:"
Write-Host "   aws ecs update-service --cluster karmacadabra-prod --service karmacadabra-prod-validator --force-new-deployment"
Write-Host ""
Write-Host "2. Monitor deployment:"
Write-Host "   aws ecs describe-services --cluster karmacadabra-prod --services karmacadabra-prod-validator"
Write-Host ""
Write-Host "3. View logs:"
Write-Host "   aws logs tail /ecs/karmacadabra-prod/validator --follow"
Write-Host ""
