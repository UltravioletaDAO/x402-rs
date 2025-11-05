# ============================================================================
# Force ECS to Pull Fresh Docker Images
# ============================================================================
# This script stops all running tasks in ECS services, forcing AWS to pull
# fresh images from ECR. This is necessary when using the `:latest` tag because
# ECS caches images and may not detect new pushes.
#
# Usage: .\force-image-pull.ps1
#
# When to use:
# - After running build-and-push.ps1 to deploy new images
# - When ECS tasks are using old cached images despite new ECR pushes
# - To ensure all services use the most recent Docker images

$ErrorActionPreference = "Stop"

# Configuration
$CLUSTER = "karmacadabra-prod"
$REGION = "us-east-1"
$AGENTS = @("validator", "karma-hello", "abracadabra", "skill-extractor", "voice-extractor")

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Force Fresh Image Pull for ECS Services" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "This will stop all running tasks, forcing ECS to pull fresh images from ECR." -ForegroundColor Yellow
Write-Host "New tasks will start automatically with the latest images." -ForegroundColor Yellow
Write-Host ""

# Confirm with user
$confirmation = Read-Host "Continue? (y/n)"
if ($confirmation -ne 'y') {
    Write-Host "Aborted." -ForegroundColor Yellow
    exit 0
}

Write-Host ""

# Stop all existing tasks
foreach ($agent in $AGENTS) {
    $SERVICE = "$CLUSTER-$agent"
    Write-Host "Stopping tasks for $agent..." -ForegroundColor Blue

    try {
        # Get running tasks for this service
        $TASK_ARNS = aws ecs list-tasks `
            --cluster $CLUSTER `
            --service-name $SERVICE `
            --desired-status RUNNING `
            --region $REGION `
            --query 'taskArns[*]' `
            --output text

        if ($TASK_ARNS) {
            $taskCount = ($TASK_ARNS -split "`t" | Where-Object { $_ }).Count
            Write-Host "  Found $taskCount running task(s)" -ForegroundColor White

            # Stop each task
            foreach ($task in $TASK_ARNS -split "`t") {
                if ($task) {
                    $taskId = $task.Split('/')[-1]
                    aws ecs stop-task `
                        --cluster $CLUSTER `
                        --task $task `
                        --region $REGION `
                        --query 'task.taskArn' `
                        --output text | Out-Null
                    Write-Host "  ✓ Stopped task: $taskId" -ForegroundColor Green
                }
            }
        }
        else {
            Write-Host "  No running tasks" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "  ✗ Error stopping tasks: $_" -ForegroundColor Red
    }

    Write-Host ""
}

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "All tasks stopped successfully!" -ForegroundColor Green
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "ECS will now automatically launch new tasks using fresh images from ECR." -ForegroundColor Yellow
Write-Host ""
Write-Host "Monitor deployment progress:" -ForegroundColor Cyan
Write-Host "  .\deploy-and-monitor.ps1" -ForegroundColor Green
Write-Host ""
Write-Host "Or check service status:" -ForegroundColor Cyan
Write-Host "  aws ecs describe-services --cluster $CLUSTER --services $CLUSTER-validator --region $REGION" -ForegroundColor Green
Write-Host ""
