#!/bin/bash
# fast-build.sh - Build Docker image using native Linux filesystem for 5-10x speedup
#
# Problem: Building on /mnt/z/ (Windows NTFS via WSL2 9P) is extremely slow
# because every file operation crosses the WSL2 <-> Windows boundary.
#
# Solution: rsync to ~/x402-rs-build (native ext4), build there, tag normally.
#
# Usage:
#   ./scripts/fast-build.sh v1.32.1          # Build + tag
#   ./scripts/fast-build.sh v1.32.1 --push   # Build + tag + push to ECR
#
# First run: ~60s rsync + ~2min build = ~3min total
# Subsequent runs: ~5s rsync + ~30s build = ~35s total (incremental)

set -euo pipefail

VERSION="${1:?Usage: $0 <version> [--push]}"
PUSH="${2:-}"

# Paths
SOURCE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="$HOME/x402-rs-build"
ECR_REPO="518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator"

# ── Kill any previous build processes to avoid parallel builds competing ──
PREV_BUILDS=$(pgrep -f "fast-build.sh" | grep -v $$ || true)
if [ -n "$PREV_BUILDS" ]; then
    echo "[PRE] Killing previous build processes: $PREV_BUILDS"
    echo "$PREV_BUILDS" | xargs kill 2>/dev/null || true
    sleep 1
fi
# Also kill any orphaned docker build processes for facilitator
PREV_DOCKER=$(pgrep -f "docker build.*x402-rs-build" || true)
if [ -n "$PREV_DOCKER" ]; then
    echo "[PRE] Killing orphaned docker build processes: $PREV_DOCKER"
    echo "$PREV_DOCKER" | xargs kill 2>/dev/null || true
    sleep 1
fi

echo "=== Fast Build: $VERSION ==="
echo "Source: $SOURCE_DIR"
echo "Build:  $BUILD_DIR"
echo ""

# Step 1: Rsync to native Linux filesystem
echo "[1/4] Syncing to native filesystem..."
SYNC_START=$(date +%s)
rsync -a --delete \
    --exclude target/ \
    --exclude .git/ \
    --exclude .unused/ \
    --exclude node_modules/ \
    --exclude nul \
    "$SOURCE_DIR/" "$BUILD_DIR/"
SYNC_END=$(date +%s)
echo "      Synced in $((SYNC_END - SYNC_START))s"

# Step 2: Build Docker image
echo "[2/4] Building Docker image..."
BUILD_START=$(date +%s)
docker build --platform linux/amd64 -t "facilitator:$VERSION" "$BUILD_DIR"
BUILD_END=$(date +%s)
echo "      Built in $((BUILD_END - BUILD_START))s"

# Step 3: Tag for ECR
echo "[3/4] Tagging for ECR..."
docker tag "facilitator:$VERSION" "$ECR_REPO:$VERSION"
docker tag "facilitator:$VERSION" "$ECR_REPO:latest"

# Step 4: Push (optional)
if [ "$PUSH" = "--push" ]; then
    echo "[4/4] Pushing to ECR..."
    aws ecr get-login-password --region us-east-2 | \
        docker login --username AWS --password-stdin "$ECR_REPO" 2>/dev/null
    docker push "$ECR_REPO:$VERSION"
    docker push "$ECR_REPO:latest"
    echo "      Pushed $ECR_REPO:$VERSION"
else
    echo "[4/4] Skipping push (use --push to push to ECR)"
fi

TOTAL_END=$(date +%s)
echo ""
echo "=== Done: $VERSION ==="
echo "    Sync:  $((SYNC_END - SYNC_START))s"
echo "    Build: $((BUILD_END - BUILD_START))s"
echo "    Total: $((TOTAL_END - SYNC_START))s"
echo ""
echo "Image: facilitator:$VERSION"
echo "ECR:   $ECR_REPO:$VERSION"
