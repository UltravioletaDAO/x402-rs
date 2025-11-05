# ============================================================================
# Diagnose ECS Deployment Issues (Verbose)
# ============================================================================

$ErrorActionPreference = "Continue"

$CLUSTER = "karmacadabra-prod"
$REGION = "us-east-1"
$AGENTS = @("validator", "karma-hello", "abracadabra", "skill-extractor", "voice-extractor")

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "ECS Deployment Diagnostics" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

foreach ($agent in $AGENTS) {
    $SERVICE = "$CLUSTER-$agent"

    Write-Host "=== Analyzing $agent ===" -ForegroundColor Yellow
    Write-Host ""

    # Get service details
    Write-Host "Service Status:" -ForegroundColor Blue
    $serviceInfo = aws ecs describe-services `
        --cluster $CLUSTER `
        --services $SERVICE `
        --region $REGION `
        --output json | ConvertFrom-Json

    $service = $serviceInfo.services[0]

    Write-Host "  Desired: $($service.desiredCount)" -ForegroundColor White
    Write-Host "  Running: $($service.runningCount)" -ForegroundColor White
    Write-Host "  Pending: $($service.pendingCount)" -ForegroundColor White

    # Show deployment status
    Write-Host ""
    Write-Host "Deployments:" -ForegroundColor Blue
    foreach ($deployment in $service.deployments) {
        Write-Host "  ID: $($deployment.id)" -ForegroundColor White
        Write-Host "  Status: $($deployment.status)" -ForegroundColor White
        Write-Host "  Rollout State: $($deployment.rolloutState)" -ForegroundColor White
        Write-Host "  Desired: $($deployment.desiredCount)" -ForegroundColor White
        Write-Host "  Running: $($deployment.runningCount)" -ForegroundColor White
        Write-Host "  Pending: $($deployment.pendingCount)" -ForegroundColor White
        Write-Host "  Failed: $($deployment.failedTasks)" -ForegroundColor Red

        if ($deployment.rolloutStateReason) {
            Write-Host "  Reason: $($deployment.rolloutStateReason)" -ForegroundColor Yellow
        }
        Write-Host ""
    }

    # Show events
    Write-Host "Recent Events:" -ForegroundColor Blue
    $service.events | Select-Object -First 5 | ForEach-Object {
        $timestamp = [DateTime]::Parse($_.createdAt).ToString("HH:mm:ss")
        Write-Host "  [$timestamp] $($_.message)" -ForegroundColor Gray
    }
    Write-Host ""

    # Get task details
    Write-Host "Tasks:" -ForegroundColor Blue

    # Check running tasks
    $runningTasks = aws ecs list-tasks `
        --cluster $CLUSTER `
        --service-name $SERVICE `
        --desired-status RUNNING `
        --region $REGION `
        --output json | ConvertFrom-Json

    if ($runningTasks.taskArns.Count -gt 0) {
        Write-Host "  Running Tasks: $($runningTasks.taskArns.Count)" -ForegroundColor Green

        $taskDetails = aws ecs describe-tasks `
            --cluster $CLUSTER `
            --tasks $runningTasks.taskArns `
            --region $REGION `
            --output json | ConvertFrom-Json

        foreach ($task in $taskDetails.tasks) {
            $taskId = $task.taskArn.Split('/')[-1]
            Write-Host "    Task: $taskId" -ForegroundColor White
            Write-Host "      Status: $($task.lastStatus)" -ForegroundColor White
            Write-Host "      Health: $($task.healthStatus)" -ForegroundColor White
        }
    }
    else {
        Write-Host "  No running tasks" -ForegroundColor Yellow
    }

    # Check stopped tasks
    $stoppedTasks = aws ecs list-tasks `
        --cluster $CLUSTER `
        --service-name $SERVICE `
        --desired-status STOPPED `
        --region $REGION `
        --output json | ConvertFrom-Json

    if ($stoppedTasks.taskArns.Count -gt 0) {
        Write-Host "  Stopped Tasks: $($stoppedTasks.taskArns.Count)" -ForegroundColor Red

        # Get details of most recent stopped task
        $recentStopped = $stoppedTasks.taskArns | Select-Object -First 1

        $taskDetails = aws ecs describe-tasks `
            --cluster $CLUSTER `
            --tasks $recentStopped `
            --region $REGION `
            --output json | ConvertFrom-Json

        $task = $taskDetails.tasks[0]
        $taskId = $task.taskArn.Split('/')[-1]

        Write-Host ""
        Write-Host "  Most Recent Stopped Task: $taskId" -ForegroundColor Red
        Write-Host "    Stopped Reason: $($task.stoppedReason)" -ForegroundColor Yellow
        Write-Host "    Stop Code: $($task.stopCode)" -ForegroundColor Yellow

        if ($task.containers.Count -gt 0) {
            Write-Host "    Container Status:" -ForegroundColor Yellow
            foreach ($container in $task.containers) {
                Write-Host "      - $($container.name)" -ForegroundColor White
                Write-Host "        Exit Code: $($container.exitCode)" -ForegroundColor White
                Write-Host "        Reason: $($container.reason)" -ForegroundColor White

                if ($container.exitCode -ne 0 -or $container.reason) {
                    Write-Host "        >>> CONTAINER FAILED <<<" -ForegroundColor Red
                }
            }
        }
    }

    # Show recent logs
    Write-Host ""
    Write-Host "Recent Logs (last 10 lines):" -ForegroundColor Blue
    $logGroup = "/ecs/$CLUSTER/$agent"

    try {
        $logs = aws logs tail $logGroup `
            --since 5m `
            --format short `
            --region $REGION 2>&1

        if ($logs -and $logs -notmatch "ResourceNotFoundException") {
            $logs | Select-Object -Last 10 | ForEach-Object {
                Write-Host "  $_" -ForegroundColor Gray
            }
        }
        else {
            Write-Host "  No logs available yet" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "  No logs available yet" -ForegroundColor Yellow
    }

    Write-Host ""
    Write-Host "---" -ForegroundColor DarkGray
    Write-Host ""
}

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "Diagnosis Complete" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Common Issues:" -ForegroundColor Yellow
Write-Host "  1. Image not found in ECR - Run build-and-push.ps1 first" -ForegroundColor White
Write-Host "  2. Secrets not in AWS Secrets Manager - Check karmacadabra secret" -ForegroundColor White
Write-Host "  3. Container crashes on startup - Check logs above" -ForegroundColor White
Write-Host "  4. Health check fails - Verify /health endpoint works" -ForegroundColor White
Write-Host "  5. Insufficient resources - Tasks need CPU/memory" -ForegroundColor White
Write-Host ""
Write-Host "Next Steps:" -ForegroundColor Yellow
Write-Host "  - Check stopped task reasons above" -ForegroundColor White
Write-Host "  - Review container exit codes" -ForegroundColor White
Write-Host "  - Examine logs for errors" -ForegroundColor White
Write-Host "  - Verify Docker images exist in ECR" -ForegroundColor White
Write-Host ""
