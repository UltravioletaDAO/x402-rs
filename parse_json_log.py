#!/usr/bin/env python3
"""
Parse CloudWatch JSON log format
"""
import json
import re
from datetime import datetime

LOG_FILE = r"z:\ultravioleta\dao\facilitator\1735.log"

def strip_ansi(text):
    """Remove ANSI escape codes"""
    ansi_escape = re.compile(r'\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])')
    return ansi_escape.sub('', text)

def main():
    print("=" * 80)
    print("FACILITATOR LOG ANALYSIS - JSON CloudWatch Format")
    print("=" * 80)
    print()

    with open(LOG_FILE, 'r', encoding='utf-8') as f:
        data = json.load(f)

    events = data.get('events', [])
    print(f"Total events: {len(events)}")
    print()

    # Track different operations
    settle_requests = []
    send_raw_tx = []
    polling_events = []
    errors = []
    deserialization_success = []
    nonce_increments = []

    for event in events:
        msg = strip_ansi(event['message'])
        ts = event['timestamp']

        # POST /settle
        if 'POST /settle' in msg and 'post_settle' in msg:
            settle_requests.append({'timestamp': ts, 'message': msg[:200]})

        # eth_sendRawTransaction
        if 'sendRawTransaction' in msg:
            send_raw_tx.append({'timestamp': ts, 'message': msg[:300]})

        # Polling
        if 'eth_blockNumber' in msg and 'poller' in msg:
            polling_events.append({'timestamp': ts, 'message': msg[:150]})

        # Deserialization
        if 'Deserialization SUCCEEDED' in msg:
            deserialization_success.append({'timestamp': ts, 'message': msg[:200]})

        # Nonce increment
        if 'incrementing nonce' in msg:
            match = re.search(r'current_nonce=(\d+)', msg)
            if match:
                nonce_increments.append({'timestamp': ts, 'nonce': int(match.group(1))})

        # Errors
        if 'ERROR' in msg or 'error' in msg.lower():
            errors.append({'timestamp': ts, 'message': msg[:300]})

    # Print summaries
    print(f"POST /settle requests: {len(settle_requests)}")
    if settle_requests:
        first_ts = datetime.fromtimestamp(settle_requests[0]['timestamp'] / 1000)
        last_ts = datetime.fromtimestamp(settle_requests[-1]['timestamp'] / 1000)
        print(f"  First: {first_ts}")
        print(f"  Last: {last_ts}")

    print(f"\neth_sendRawTransaction calls: {len(send_raw_tx)}")
    if send_raw_tx:
        print("  Details:")
        for tx in send_raw_tx[:10]:  # First 10
            ts = datetime.fromtimestamp(tx['timestamp'] / 1000)
            print(f"    [{ts}] {tx['message']}")

    print(f"\nPolling events: {len(polling_events)}")
    print(f"Deserialization successes: {len(deserialization_success)}")
    print(f"Nonce increments: {len(nonce_increments)}")
    if nonce_increments:
        print(f"  Nonces: {sorted(set(n['nonce'] for n in nonce_increments))}")

    print(f"\nErrors found: {len(errors)}")
    if errors:
        print("  First 5 errors:")
        for err in errors[:5]:
            ts = datetime.fromtimestamp(err['timestamp'] / 1000)
            print(f"    [{ts}] {err['message'][:150]}")

    # Look for specific transaction hashes
    print(f"\n{'='*80}")
    print("SEARCHING FOR TRANSACTION HASHES")
    print(f"{'='*80}")

    tx_hashes = []
    for event in events:
        msg = strip_ansi(event['message'])
        # Find 0x + 64 hex chars (transaction hash)
        matches = re.findall(r'0x[0-9a-f]{64}', msg)
        for match in matches:
            if match not in tx_hashes:
                tx_hashes.append(match)
                ts = datetime.fromtimestamp(event['timestamp'] / 1000)
                print(f"  [{ts}] {match}")
                print(f"    Context: {msg[max(0, msg.index(match)-50):msg.index(match)+120]}")

    print(f"\n{'='*80}")
    print("ANALYSIS")
    print(f"{'='*80}")
    print(f"The log shows {len(settle_requests)} settlement requests")
    print(f"But only {len(send_raw_tx)} eth_sendRawTransaction calls")
    print(f"Difference: {len(settle_requests) - len(send_raw_tx)} requests didn't result in on-chain tx")
    print()
    print(f"This log appears to be from a POLLING operation (waiting for confirmations)")
    print(f"NOT from the initial settlement phase where transactions are submitted")

if __name__ == "__main__":
    main()
