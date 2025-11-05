#!/bin/bash
# View recent facilitator logs

LOG_GROUP="/ecs/facilitator-production"
REGION="us-east-2"
MINUTES_AGO="${1:-10}"  # Default to last 10 minutes

echo "Fetching logs from the last $MINUTES_AGO minutes..."

# Get the most recent log stream
LOG_STREAM=$(aws logs describe-log-streams \
  --log-group-name "$LOG_GROUP" \
  --region "$REGION" \
  --order-by LastEventTime \
  --descending \
  --max-items 1 \
  --query 'logStreams[0].logStreamName' \
  --output text)

echo "Log stream: $LOG_STREAM"
echo ""

# Calculate start time (N minutes ago)
START_TIME=$(($(date +%s - $MINUTES_AGO*60)000))

# Get events
aws logs get-log-events \
  --log-group-name "$LOG_GROUP" \
  --log-stream-name "$LOG_STREAM" \
  --region "$REGION" \
  --start-time "$START_TIME" \
  --query 'events[].message' \
  --output text

echo ""
echo "---"
echo "To filter for settle debug info, run:"
echo "./scripts/view-recent-logs.sh | grep -A 100 'SETTLE REQUEST DEBUG'"
