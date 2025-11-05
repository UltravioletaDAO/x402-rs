#!/bin/bash
# Watch facilitator logs for debugging settle requests

LOG_GROUP="/ecs/facilitator-production"
REGION="us-east-2"

echo "Fetching latest log stream..."

# Get the most recent log stream
LOG_STREAM=$(aws logs describe-log-streams \
  --log-group-name "$LOG_GROUP" \
  --region "$REGION" \
  --order-by LastEventTime \
  --descending \
  --max-items 1 \
  --query 'logStreams[0].logStreamName' \
  --output text)

echo "Watching log stream: $LOG_STREAM"
echo "Press Ctrl+C to stop"
echo ""

# Function to get latest events
get_latest_events() {
  local start_time=$1
  aws logs get-log-events \
    --log-group-name "$LOG_GROUP" \
    --log-stream-name "$LOG_STREAM" \
    --region "$REGION" \
    --start-time "$start_time" \
    --query 'events[].message' \
    --output text
}

# Start from current time
START_TIME=$(date +%s)000

# Poll for new logs every 2 seconds
while true; do
  EVENTS=$(get_latest_events "$START_TIME")

  if [ -n "$EVENTS" ]; then
    echo "$EVENTS"
    # Update start time to avoid duplicates
    START_TIME=$(($(date +%s)000))
  fi

  sleep 2
done
