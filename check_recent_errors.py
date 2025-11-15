#!/usr/bin/env python3
"""
Check recent errors in CloudWatch logs, especially for Base network
"""
import boto3
import time
from datetime import datetime

def check_errors():
    client = boto3.client('logs', region_name='us-east-2')
    log_group = '/ecs/facilitator-production'

    # Get logs from last 30 minutes
    start_time = int((time.time() - 1800) * 1000)

    print(f"Checking logs from {log_group}")
    print(f"Since: {datetime.fromtimestamp(start_time/1000)}")
    print("=" * 80)

    try:
        # Check for errors
        print("\n[*] Looking for ERROR/WARN logs...")
        response = client.filter_log_events(
            logGroupName=log_group,
            startTime=start_time,
            filterPattern='ERROR OR WARN OR timeout OR failed',
            limit=100
        )

        if response['events']:
            print(f"\nFound {len(response['events'])} error/warning entries:\n")
            for event in response['events'][-30:]:  # Show last 30
                timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
                print(f"[{timestamp}] {event['message']}")
                print("-" * 80)
        else:
            print("[OK] No errors or warnings found\n")

        # Check for Base-specific activity
        print("\n[*] Looking for Base network activity...")
        response = client.filter_log_events(
            logGroupName=log_group,
            startTime=start_time,
            filterPattern='base OR 8453',
            limit=100
        )

        if response['events']:
            print(f"\nFound {len(response['events'])} Base-related entries:\n")
            for event in response['events'][-30:]:  # Show last 30
                timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
                print(f"[{timestamp}] {event['message']}")
                print("-" * 80)
        else:
            print("[WARN] No Base network activity found\n")

        # Check for settle/verify requests
        print("\n[*] Looking for settle/verify requests...")
        response = client.filter_log_events(
            logGroupName=log_group,
            startTime=start_time,
            filterPattern='settle OR verify',
            limit=100
        )

        if response['events']:
            print(f"\nFound {len(response['events'])} settle/verify requests:\n")
            for event in response['events'][-30:]:  # Show last 30
                timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
                print(f"[{timestamp}] {event['message']}")
                print("-" * 80)
        else:
            print("[WARN] No settle/verify requests found\n")

    except Exception as e:
        print(f"Error: {e}")

if __name__ == '__main__':
    check_errors()
