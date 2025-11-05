#!/bin/bash
# Blacklist Fix Validation Script
#
# This script validates that the blacklist fix is working correctly in both
# local Docker images and deployed production environments.
#
# Usage:
#   ./scripts/validate_blacklist_fix.sh local    # Test local Docker image
#   ./scripts/validate_blacklist_fix.sh prod     # Test production deployment

set -e

COLOR_GREEN='\033[0;32m'
COLOR_RED='\033[0;31m'
COLOR_YELLOW='\033[1;33m'
COLOR_BLUE='\033[0;34m'
COLOR_RESET='\033[0m'

ENVIRONMENT="${1:-local}"

echo -e "${COLOR_BLUE}========================================${COLOR_RESET}"
echo -e "${COLOR_BLUE}Blacklist Fix Validation Script${COLOR_RESET}"
echo -e "${COLOR_BLUE}Environment: $ENVIRONMENT${COLOR_RESET}"
echo -e "${COLOR_BLUE}========================================${COLOR_RESET}"
echo ""

# Configuration
if [ "$ENVIRONMENT" = "prod" ]; then
    BASE_URL="https://facilitator.prod.ultravioletadao.xyz"
    IMAGE_NAME="<AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com/facilitator:latest"
elif [ "$ENVIRONMENT" = "local" ]; then
    BASE_URL="http://localhost:8080"
    IMAGE_NAME="facilitator:latest"
else
    echo -e "${COLOR_RED}Invalid environment: $ENVIRONMENT${COLOR_RESET}"
    echo "Usage: $0 [local|prod]"
    exit 1
fi

FAILED=0

# Test 1: Verify config/blacklist.json exists in Docker image
echo -e "${COLOR_BLUE}[Test 1] Checking if config/blacklist.json exists in Docker image...${COLOR_RESET}"
if [ "$ENVIRONMENT" = "local" ]; then
    if docker run --rm --entrypoint ls "$IMAGE_NAME" -la /app/config/blacklist.json 2>/dev/null; then
        echo -e "${COLOR_GREEN}✓ PASS: blacklist.json found in Docker image at /app/config/blacklist.json${COLOR_RESET}"
    else
        echo -e "${COLOR_RED}✗ FAIL: blacklist.json NOT found in Docker image${COLOR_RESET}"
        echo -e "${COLOR_YELLOW}  This means the Dockerfile fix was not applied.${COLOR_RESET}"
        FAILED=1
    fi
    echo ""
else
    echo -e "${COLOR_YELLOW}⊘ SKIP: Cannot inspect remote production image directly${COLOR_RESET}"
    echo ""
fi

# Test 2: Check /health endpoint is responding
echo -e "${COLOR_BLUE}[Test 2] Checking if facilitator is running...${COLOR_RESET}"
if curl -sf "$BASE_URL/health" > /dev/null 2>&1; then
    echo -e "${COLOR_GREEN}✓ PASS: Facilitator is running and responding${COLOR_RESET}"
else
    echo -e "${COLOR_RED}✗ FAIL: Facilitator is not responding at $BASE_URL${COLOR_RESET}"
    echo -e "${COLOR_YELLOW}  Make sure the facilitator is running before running this script.${COLOR_RESET}"
    FAILED=1
    # Can't continue if facilitator isn't running
    exit 1
fi
echo ""

# Test 3: Verify /blacklist endpoint exists
echo -e "${COLOR_BLUE}[Test 3] Checking if /blacklist endpoint exists...${COLOR_RESET}"
BLACKLIST_RESPONSE=$(curl -sf "$BASE_URL/blacklist" || echo "FAILED")
if [ "$BLACKLIST_RESPONSE" = "FAILED" ]; then
    echo -e "${COLOR_RED}✗ FAIL: /blacklist endpoint not responding${COLOR_RESET}"
    echo -e "${COLOR_YELLOW}  This indicates the new endpoint was not deployed.${COLOR_RESET}"
    FAILED=1
else
    echo -e "${COLOR_GREEN}✓ PASS: /blacklist endpoint is responding${COLOR_RESET}"
    echo ""

    # Test 4: Validate blacklist response structure
    echo -e "${COLOR_BLUE}[Test 4] Validating blacklist response structure...${COLOR_RESET}"

    # Check for required fields
    TOTAL_BLOCKED=$(echo "$BLACKLIST_RESPONSE" | jq -r '.totalBlocked // empty')
    EVM_COUNT=$(echo "$BLACKLIST_RESPONSE" | jq -r '.evmCount // empty')
    SOLANA_COUNT=$(echo "$BLACKLIST_RESPONSE" | jq -r '.solanaCount // empty')
    LOADED_AT_STARTUP=$(echo "$BLACKLIST_RESPONSE" | jq -r '.loadedAtStartup // empty')

    if [ -z "$TOTAL_BLOCKED" ] || [ -z "$EVM_COUNT" ] || [ -z "$SOLANA_COUNT" ] || [ -z "$LOADED_AT_STARTUP" ]; then
        echo -e "${COLOR_RED}✗ FAIL: Blacklist response missing required fields${COLOR_RESET}"
        echo "Response: $BLACKLIST_RESPONSE"
        FAILED=1
    else
        echo -e "${COLOR_GREEN}✓ PASS: Response structure is valid${COLOR_RESET}"
        echo "  Total blocked: $TOTAL_BLOCKED"
        echo "  EVM count: $EVM_COUNT"
        echo "  Solana count: $SOLANA_COUNT"
        echo "  Loaded at startup: $LOADED_AT_STARTUP"
    fi
fi
echo ""

# Test 5: Verify blacklist is actually loaded (not empty)
echo -e "${COLOR_BLUE}[Test 5] Verifying blacklist was loaded successfully...${COLOR_RESET}"
if [ -n "$TOTAL_BLOCKED" ]; then
    if [ "$TOTAL_BLOCKED" -eq 0 ]; then
        echo -e "${COLOR_RED}✗ FAIL: Blacklist is EMPTY (0 blocked addresses)${COLOR_RESET}"
        echo -e "${COLOR_YELLOW}  This means the blacklist file was not loaded or is empty.${COLOR_RESET}"
        FAILED=1
    else
        echo -e "${COLOR_GREEN}✓ PASS: Blacklist loaded with $TOTAL_BLOCKED blocked addresses${COLOR_RESET}"
    fi

    if [ "$LOADED_AT_STARTUP" != "true" ]; then
        echo -e "${COLOR_RED}✗ FAIL: loadedAtStartup is false${COLOR_RESET}"
        echo -e "${COLOR_YELLOW}  This indicates blacklist failed to load at startup.${COLOR_RESET}"
        FAILED=1
    else
        echo -e "${COLOR_GREEN}✓ PASS: Blacklist was loaded at startup${COLOR_RESET}"
    fi
fi
echo ""

# Test 6: Verify malicious wallet is in blacklist
echo -e "${COLOR_BLUE}[Test 6] Checking if malicious Solana wallet is blacklisted...${COLOR_RESET}"
MALICIOUS_WALLET="41fx2qju8qceppdlwnypgxahagj3dfvi8bhfumteq3az"
if echo "$BLACKLIST_RESPONSE" | jq -e ".entries[] | select(.wallet == \"$MALICIOUS_WALLET\")" > /dev/null 2>&1; then
    echo -e "${COLOR_GREEN}✓ PASS: Malicious wallet $MALICIOUS_WALLET is in blacklist${COLOR_RESET}"
    REASON=$(echo "$BLACKLIST_RESPONSE" | jq -r ".entries[] | select(.wallet == \"$MALICIOUS_WALLET\") | .reason")
    echo "  Reason: $REASON"
else
    echo -e "${COLOR_RED}✗ FAIL: Malicious wallet $MALICIOUS_WALLET NOT found in blacklist${COLOR_RESET}"
    echo -e "${COLOR_YELLOW}  WARNING: The wallet that drained funds is not blocked!${COLOR_RESET}"
    FAILED=1
fi
echo ""

# Test 7: Display full blacklist entries
echo -e "${COLOR_BLUE}[Test 7] Current blacklist entries:${COLOR_RESET}"
echo "$BLACKLIST_RESPONSE" | jq -r '.entries[] | "  - [\(.account_type)] \(.wallet) - Reason: \(.reason)"'
echo ""

# Final summary
echo -e "${COLOR_BLUE}========================================${COLOR_RESET}"
if [ $FAILED -eq 0 ]; then
    echo -e "${COLOR_GREEN}✓ ALL TESTS PASSED${COLOR_RESET}"
    echo -e "${COLOR_GREEN}Blacklist fix is working correctly!${COLOR_RESET}"
    exit 0
else
    echo -e "${COLOR_RED}✗ SOME TESTS FAILED${COLOR_RESET}"
    echo -e "${COLOR_YELLOW}Please review the failures above and fix them before deploying.${COLOR_RESET}"
    echo ""
    echo "Common issues:"
    echo "  1. Dockerfile not updated - Add 'COPY --from=builder /app/config /app/config'"
    echo "  2. Image not rebuilt - Run './scripts/build-and-push.sh v1.2.1-blacklist-fix'"
    echo "  3. Old image deployed - Force ECS service update with new task definition"
    echo "  4. Blacklist file missing locally - Check config/blacklist.json exists"
    exit 1
fi
