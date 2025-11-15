#!/usr/bin/env python3
"""
Fetch CloudWatch logs for Base network errors and timeouts
"""
import boto3
import time
from datetime import datetime, timedelta

def fetch_base_logs():
    client = boto3.client('logs', region_name='us-east-2')
    log_group = '/ecs/facilitator-production'

    # Get logs from last hour
    start_time = int((time.time() - 3600) * 1000)

    print(f"Fetching logs from {log_group} since {datetime.fromtimestamp(start_time/1000)}")
    print("=" * 80)

    try:
        response = client.filter_log_events(
            logGroupName=log_group,
            startTime=start_time,
            filterPattern='timeout OR 504 OR Base',
            limit=50
        )

        if not response['events']:
            print("No matching log events found for: timeout OR 504 OR Base")
            print("\nTrying to fetch recent Base-related logs...")

            response = client.filter_log_events(
                logGroupName=log_group,
                startTime=start_time,
                filterPattern='base',
                limit=50
            )

        for event in response['events']:
            timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
            print(f"[{timestamp}] {event['message']}")
            print("-" * 80)

        print(f"\nTotal events found: {len(response['events'])}")

    except Exception as e:
        print(f"Error fetching logs: {e}")

if __name__ == '__main__':
    fetch_base_logs()
