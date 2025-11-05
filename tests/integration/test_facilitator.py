#!/usr/bin/env python3
"""
Test facilitator with verbose logging to CloudWatch
"""
import sys
import time
sys.path.insert(0, '/z/ultravioleta/dao/karmacadabra')

from test-seller.load_test import LoadTest

print("="*60)
print("Testing facilitator with verbose logging")
print("="*60)

# Run a single test
tester = LoadTest(num_requests=1, verbose=True)
tester.run()

print("\n" + "="*60)
print("Test completed. Now checking CloudWatch logs...")
print("="*60)
print("\nWait 5 seconds for logs to propagate...")
time.sleep(5)

print("\nTo view logs, run:")
print("  aws logs tail /ecs/facilitator-production --since 1m --follow --region us-east-2")
