# ============================================================================
# Deploy and Monitor ECS Services (PowerShell)
# ============================================================================
# This script forces new ECS deployments and monitors their progress
#
# Usage: .\deploy-and-monitor.ps1

$ErrorActionPreference = "Stop"

# Configuration
$CLUSTER = "karmacadabra-prod"
$REGION = "us-east-1"
$AGENTS = @("validator", "karma-hello", "abracadabra", "skill-extractor", "voice-extractor")
$ALB_DNS = "karmacadabra-prod-alb-1072717858.us-east-1.elb.amazonaws.com"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "Karmacadabra ECS Deployment & Monitor" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

# Step 1: Force New Deployment
Write-Host "[STEP 1/5] Forcing new deployment for all services..." -ForegroundColor Yellow
Write-Host ""

foreach ($agent in $AGENTS) {
    $SERVICE = "$CLUSTER-$agent"
    Write-Host "Deploying $agent..." -ForegroundColor Blue

    try {
        aws ecs update-service `
            --cluster $CLUSTER `
            --service $SERVICE `
            --force-new-deployment `
            --region $REGION `
            --query 'service.serviceName' `
            --output text | Out-Null

        Write-Host "✓ $agent deployment initiated" -ForegroundColor Green
    }
    catch {
        Write-Host "✗ $agent deployment failed" -ForegroundColor Red
    }
}

Write-Host ""
Write-Host "All deployments initiated!" -ForegroundColor Green
Write-Host ""

# Step 2: Stop Existing Tasks (Force Fresh Image Pull)
Write-Host "[STEP 2/6] Stopping existing tasks to force fresh image pulls..." -ForegroundColor Yellow
Write-Host ""

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
            # Stop each task
            foreach ($task in $TASK_ARNS -split "`t") {
                if ($task) {
                    aws ecs stop-task `
                        --cluster $CLUSTER `
                        --task $task `
                        --region $REGION `
                        --query 'task.taskArn' `
                        --output text | Out-Null
                    Write-Host "  ✓ Stopped task: $task" -ForegroundColor Green
                }
            }
        }
        else {
            Write-Host "  No tasks to stop" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "  ⚠ Error stopping tasks" -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host "All existing tasks stopped. ECS will now launch new tasks with fresh images..." -ForegroundColor Green
Write-Host "Waiting 30 seconds for new tasks to start..." -ForegroundColor Blue
Start-Sleep -Seconds 30
Write-Host ""

# Step 3: Monitor Deployment Progress
Write-Host "[STEP 3/6] Monitoring deployment status..." -ForegroundColor Yellow
Write-Host ""

$MAX_WAIT = 600  # 10 minutes
$INTERVAL = 15   # Check every 15 seconds
$ELAPSED = 0

while ($ELAPSED -lt $MAX_WAIT) {
    Write-Host "Checking status ($ELAPSED`s elapsed)..." -ForegroundColor Blue

    # Build services list
    $SERVICES = $AGENTS | ForEach-Object { "$CLUSTER-$_" }

    # Get service status
    $STATUS = aws ecs describe-services `
        --cluster $CLUSTER `
        --services $SERVICES `
        --region $REGION `
        --query 'services[*].[serviceName,desiredCount,runningCount,deployments[0].rolloutState]' `
        --output text

    $PENDING = 0
    $STATUS -split "`n" | ForEach-Object {
        $parts = $_ -split "`t"
        $SERVICE_NAME = $parts[0]
        $DESIRED = $parts[1]
        $RUNNING = $parts[2]
        $ROLLOUT = $parts[3]

        $AGENT_NAME = $SERVICE_NAME -replace "$CLUSTER-", ""

        if ($RUNNING -eq $DESIRED -and $ROLLOUT -eq "COMPLETED") {
            Write-Host "  ✓ ${AGENT_NAME}: ${RUNNING}/${DESIRED} running ($ROLLOUT)" -ForegroundColor Green
        }
        elseif ($ROLLOUT -eq "FAILED") {
            Write-Host "  ✗ ${AGENT_NAME}: ${RUNNING}/${DESIRED} running ($ROLLOUT)" -ForegroundColor Red
        }
        else {
            Write-Host "  ⟳ ${AGENT_NAME}: ${RUNNING}/${DESIRED} running ($ROLLOUT)" -ForegroundColor Yellow
            $PENDING++
        }
    }

    if ($PENDING -eq 0) {
        Write-Host "All deployments completed!" -ForegroundColor Green
        break
    }

    Write-Host ""
    Start-Sleep -Seconds $INTERVAL
    $ELAPSED += $INTERVAL
}

if ($ELAPSED -ge $MAX_WAIT) {
    Write-Host "Warning: Deployment monitoring timed out after $MAX_WAIT`s" -ForegroundColor Yellow
    Write-Host "Services may still be deploying. Check manually with:" -ForegroundColor Yellow
    Write-Host "  aws ecs describe-services --cluster $CLUSTER --services $($SERVICES -join ' ')"
}

Write-Host ""

# Step 4: Test Health Endpoints
Write-Host "[STEP 4/6] Testing health endpoints..." -ForegroundColor Yellow
Write-Host ""

Write-Host "Testing via ALB (path-based routing)..." -ForegroundColor Blue
foreach ($agent in $AGENTS) {
    $URL = "http://$ALB_DNS/$agent/health"

    Write-Host "  $agent`: " -NoNewline

    try {
        $response = Invoke-WebRequest -Uri $URL -TimeoutSec 5 -UseBasicParsing -ErrorAction Stop
        if ($response.StatusCode -eq 200) {
            Write-Host "✓ OK (HTTP $($response.StatusCode))" -ForegroundColor Green
        }
        else {
            Write-Host "⚠ HTTP $($response.StatusCode)" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "✗ Connection failed" -ForegroundColor Red
    }
}

Write-Host ""
Write-Host "Testing via custom domains (may fail if DNS not propagated)..." -ForegroundColor Blue
foreach ($agent in $AGENTS) {
    $URL = "http://$agent.karmacadabra.ultravioletadao.xyz/health"

    Write-Host "  $agent`: " -NoNewline

    try {
        $response = Invoke-WebRequest -Uri $URL -TimeoutSec 5 -UseBasicParsing -ErrorAction Stop
        if ($response.StatusCode -eq 200) {
            Write-Host "✓ OK (HTTP $($response.StatusCode))" -ForegroundColor Green
        }
        else {
            Write-Host "⚠ HTTP $($response.StatusCode)" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "⚠ DNS not propagated yet" -ForegroundColor Yellow
    }
}

Write-Host ""

# Step 5: Show Recent Logs
Write-Host "[STEP 5/6] Showing recent logs (last 20 lines per agent)..." -ForegroundColor Yellow
Write-Host ""

foreach ($agent in $AGENTS) {
    $LOG_GROUP = "/ecs/$CLUSTER/$agent"

    Write-Host "=== $agent logs ===" -ForegroundColor Blue

    try {
        $logs = aws logs tail $LOG_GROUP `
            --since 5m `
            --format short `
            --region $REGION 2>&1

        if ($logs) {
            $logs | Select-Object -Last 20
        }
        else {
            Write-Host "  No logs available yet" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "  No logs available yet" -ForegroundColor Yellow
    }

    Write-Host ""
}

# Step 6: Summary
Write-Host "[STEP 6/6] Deployment Summary" -ForegroundColor Yellow
Write-Host ""

Write-Host "Access URLs:" -ForegroundColor Cyan
Write-Host ""
Write-Host "Custom Domains (Recommended):" -ForegroundColor Green
foreach ($agent in $AGENTS) {
    Write-Host "  http://$agent.karmacadabra.ultravioletadao.xyz/health"
}

Write-Host ""
Write-Host "ALB Path-Based:" -ForegroundColor Green
foreach ($agent in $AGENTS) {
    Write-Host "  http://$ALB_DNS/$agent/health"
}

Write-Host ""
Write-Host "Useful Commands:" -ForegroundColor Cyan
Write-Host ""
Write-Host "Monitor logs:" -ForegroundColor Green
Write-Host "  aws logs tail /ecs/$CLUSTER/validator --follow"
Write-Host ""
Write-Host "Check service status:" -ForegroundColor Green
Write-Host "  aws ecs describe-services --cluster $CLUSTER --services $CLUSTER-validator"
Write-Host ""
Write-Host "List running tasks:" -ForegroundColor Green
Write-Host "  aws ecs list-tasks --cluster $CLUSTER --desired-status RUNNING"
Write-Host ""
Write-Host "SSH into container:" -ForegroundColor Green
Write-Host "  aws ecs execute-command --cluster $CLUSTER --task <TASK_ID> --container validator --interactive --command '/bin/bash'"
Write-Host ""

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "Deployment monitoring complete!" -ForegroundColor Green
Write-Host "=========================================" -ForegroundColor Cyan
