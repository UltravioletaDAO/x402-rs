#!/usr/bin/env python3
"""
Check logs for a specific ECS task
"""
import boto3
import sys
from datetime import datetime

def get_task_logs(task_id):
    client = boto3.client('logs', region_name='us-east-2')
    log_group = '/ecs/facilitator-production'
    log_stream_prefix = f'ecs/facilitator/{task_id}'

    print(f"Looking for logs with prefix: {log_stream_prefix}")
    print("=" * 80)

    try:
        # Get log streams for this task
        streams_response = client.describe_log_streams(
            logGroupName=log_group,
            logStreamNamePrefix=log_stream_prefix,
            orderBy='LogStreamName',
            descending=True,
            limit=5
        )

        if not streams_response['logStreams']:
            print(f"No log streams found for task {task_id}")
            return

        for stream in streams_response['logStreams']:
            stream_name = stream['logStreamName']
            print(f"\nLog stream: {stream_name}")
            print("-" * 80)

            # Get logs from this stream
            events_response = client.get_log_events(
                logGroupName=log_group,
                logStreamName=stream_name,
                limit=100,
                startFromHead=False  # Get most recent
            )

            for event in events_response['events'][-50:]:  # Show last 50
                timestamp = datetime.fromtimestamp(event['timestamp'] / 1000)
                print(f"[{timestamp}] {event['message']}")

    except Exception as e:
        print(f"Error: {e}")

if __name__ == '__main__':
    if len(sys.argv) > 1:
        task_id = sys.argv[1]
    else:
        # Default to the new task
        task_id = 'c6661234882e4a6a99c907572a7223b7'

    print(f"Checking logs for task: {task_id}\n")
    get_task_logs(task_id)
