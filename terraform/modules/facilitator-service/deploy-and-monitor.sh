#!/bin/bash
# ============================================================================
# Deploy and Monitor ECS Services (Bash)
# ============================================================================
# This script forces new ECS deployments and monitors their progress
#
# Usage: bash deploy-and-monitor.sh

set -e

# Configuration
CLUSTER="karmacadabra-prod"
REGION="us-east-1"
AGENTS=("validator" "karma-hello" "abracadabra" "skill-extractor" "voice-extractor")
ALB_DNS="karmacadabra-prod-alb-1072717858.us-east-1.elb.amazonaws.com"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

echo -e "${CYAN}=========================================${NC}"
echo -e "${CYAN}Karmacadabra ECS Deployment & Monitor${NC}"
echo -e "${CYAN}=========================================${NC}"
echo ""

# Step 1: Force New Deployment
echo -e "${YELLOW}[STEP 1/5] Forcing new deployment for all services...${NC}"
echo ""

for agent in "${AGENTS[@]}"; do
    SERVICE="${CLUSTER}-${agent}"
    echo -e "${BLUE}Deploying ${agent}...${NC}"

    aws ecs update-service \
        --cluster "$CLUSTER" \
        --service "$SERVICE" \
        --force-new-deployment \
        --region "$REGION" \
        --query 'service.serviceName' \
        --output text > /dev/null

    if [ $? -eq 0 ]; then
        echo -e "${GREEN}✓ ${agent} deployment initiated${NC}"
    else
        echo -e "${RED}✗ ${agent} deployment failed${NC}"
    fi
done

echo ""
echo -e "${GREEN}All deployments initiated!${NC}"
echo ""

# Step 2: Monitor Deployment Progress
echo -e "${YELLOW}[STEP 2/5] Monitoring deployment status...${NC}"
echo ""

MAX_WAIT=600  # 10 minutes
INTERVAL=15   # Check every 15 seconds
ELAPSED=0

while [ $ELAPSED -lt $MAX_WAIT ]; do
    echo -e "${BLUE}Checking status (${ELAPSED}s elapsed)...${NC}"

    # Get service status for all agents
    SERVICES=$(printf "%s " "${AGENTS[@]/#/${CLUSTER}-}")

    STATUS=$(aws ecs describe-services \
        --cluster "$CLUSTER" \
        --services $SERVICES \
        --region "$REGION" \
        --query 'services[*].[serviceName,desiredCount,runningCount,deployments[0].rolloutState]' \
        --output text)

    echo "$STATUS" | while read -r line; do
        SERVICE_NAME=$(echo "$line" | awk '{print $1}')
        DESIRED=$(echo "$line" | awk '{print $2}')
        RUNNING=$(echo "$line" | awk '{print $3}')
        ROLLOUT=$(echo "$line" | awk '{print $4}')

        AGENT_NAME=$(echo "$SERVICE_NAME" | sed "s/${CLUSTER}-//")

        if [ "$RUNNING" -eq "$DESIRED" ] && [ "$ROLLOUT" == "COMPLETED" ]; then
            echo -e "  ${GREEN}✓ ${AGENT_NAME}: ${RUNNING}/${DESIRED} running (${ROLLOUT})${NC}"
        elif [ "$ROLLOUT" == "FAILED" ]; then
            echo -e "  ${RED}✗ ${AGENT_NAME}: ${RUNNING}/${DESIRED} running (${ROLLOUT})${NC}"
        else
            echo -e "  ${YELLOW}⟳ ${AGENT_NAME}: ${RUNNING}/${DESIRED} running (${ROLLOUT})${NC}"
        fi
    done

    # Check if all deployments are complete
    PENDING=$(echo "$STATUS" | grep -c "IN_PROGRESS" || true)

    if [ "$PENDING" -eq 0 ]; then
        echo -e "${GREEN}All deployments completed!${NC}"
        break
    fi

    echo ""
    sleep $INTERVAL
    ELAPSED=$((ELAPSED + INTERVAL))
done

if [ $ELAPSED -ge $MAX_WAIT ]; then
    echo -e "${YELLOW}Warning: Deployment monitoring timed out after ${MAX_WAIT}s${NC}"
    echo -e "${YELLOW}Services may still be deploying. Check manually with:${NC}"
    echo "  aws ecs describe-services --cluster $CLUSTER --services ${SERVICES}"
fi

echo ""

# Step 3: Test Health Endpoints
echo -e "${YELLOW}[STEP 3/5] Testing health endpoints...${NC}"
echo ""

echo -e "${BLUE}Testing via ALB (path-based routing)...${NC}"
for agent in "${AGENTS[@]}"; do
    URL="http://${ALB_DNS}/${agent}/health"

    echo -n "  ${agent}: "
    RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" -m 5 "$URL" 2>/dev/null || echo "000")

    if [ "$RESPONSE" == "200" ]; then
        echo -e "${GREEN}✓ OK (HTTP $RESPONSE)${NC}"
    elif [ "$RESPONSE" == "000" ]; then
        echo -e "${RED}✗ Connection failed${NC}"
    else
        echo -e "${YELLOW}⚠ HTTP $RESPONSE${NC}"
    fi
done

echo ""
echo -e "${BLUE}Testing via custom domains (may fail if DNS not propagated)...${NC}"
for agent in "${AGENTS[@]}"; do
    URL="http://${agent}.karmacadabra.ultravioletadao.xyz/health"

    echo -n "  ${agent}: "
    RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" -m 5 "$URL" 2>/dev/null || echo "000")

    if [ "$RESPONSE" == "200" ]; then
        echo -e "${GREEN}✓ OK (HTTP $RESPONSE)${NC}"
    elif [ "$RESPONSE" == "000" ]; then
        echo -e "${YELLOW}⚠ DNS not propagated yet${NC}"
    else
        echo -e "${YELLOW}⚠ HTTP $RESPONSE${NC}"
    fi
done

echo ""

# Step 4: Show Recent Logs
echo -e "${YELLOW}[STEP 4/5] Showing recent logs (last 20 lines per agent)...${NC}"
echo ""

for agent in "${AGENTS[@]}"; do
    LOG_GROUP="/ecs/${CLUSTER}/${agent}"

    echo -e "${BLUE}=== ${agent} logs ===${NC}"

    aws logs tail "$LOG_GROUP" \
        --since 5m \
        --format short \
        --region "$REGION" \
        2>/dev/null | tail -20 || echo -e "${YELLOW}  No logs available yet${NC}"

    echo ""
done

# Step 5: Summary
echo -e "${YELLOW}[STEP 5/5] Deployment Summary${NC}"
echo ""

echo -e "${CYAN}Access URLs:${NC}"
echo ""
echo -e "${GREEN}Custom Domains (Recommended):${NC}"
for agent in "${AGENTS[@]}"; do
    echo "  http://${agent}.karmacadabra.ultravioletadao.xyz/health"
done

echo ""
echo -e "${GREEN}ALB Path-Based:${NC}"
for agent in "${AGENTS[@]}"; do
    echo "  http://${ALB_DNS}/${agent}/health"
done

echo ""
echo -e "${CYAN}Useful Commands:${NC}"
echo ""
echo -e "${GREEN}Monitor logs:${NC}"
echo "  aws logs tail /ecs/${CLUSTER}/validator --follow"
echo ""
echo -e "${GREEN}Check service status:${NC}"
echo "  aws ecs describe-services --cluster ${CLUSTER} --services ${CLUSTER}-validator"
echo ""
echo -e "${GREEN}List running tasks:${NC}"
echo "  aws ecs list-tasks --cluster ${CLUSTER} --desired-status RUNNING"
echo ""
echo -e "${GREEN}SSH into container:${NC}"
echo "  aws ecs execute-command --cluster ${CLUSTER} --task <TASK_ID> --container validator --interactive --command '/bin/bash'"
echo ""

echo -e "${CYAN}=========================================${NC}"
echo -e "${GREEN}Deployment monitoring complete!${NC}"
echo -e "${CYAN}=========================================${NC}"
