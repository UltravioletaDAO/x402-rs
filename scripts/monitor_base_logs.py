#!/usr/bin/env python3
"""
Monitor CloudWatch logs for Base network activity
"""
import boto3
import time
from datetime import datetime

def monitor_logs():
    client = boto3.client('logs', region_name='us-east-2')
    log_group = '/ecs/facilitator-production'

    # Get logs from last 5 minutes
    start_time = int((time.time() - 300) * 1000)

    print(f"Monitoring logs from {log_group}")
    print(f"Looking for Base network activity since {datetime.fromtimestamp(start_time/1000)}")
    print("=" * 80)

    try:
        response = client.filter_log_events(
            logGroupName=log_group,
            startTime=start_time,
            limit=100
        )

        base_related = []
        for event in response['events']:
            message = event['message']
            if any(keyword in message.lower() for keyword in ['base', 'settle', 'verify', '8453']):
                timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
                base_related.append((timestamp, message))

        if base_related:
            print(f"Found {len(base_related)} Base-related log entries:\n")
            for timestamp, message in base_related[-20:]:  # Show last 20
                print(f"[{timestamp}] {message}")
                print("-" * 80)
        else:
            print("No Base-related activity in the last 5 minutes.")
            print("\nShowing last 10 log entries:")
            for event in response['events'][-10:]:
                timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
                print(f"[{timestamp}] {event['message']}")
                print("-" * 80)

    except Exception as e:
        print(f"Error: {e}")

if __name__ == '__main__':
    monitor_logs()
