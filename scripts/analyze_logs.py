#!/usr/bin/env python3
"""
Analyze facilitator logs in chunks to find patterns
"""
import json
import re
from collections import defaultdict

LOG_FILE = r"z:\ultravioleta\dao\facilitator\1735.log"
CHUNK_SIZE = 150  # Lines per chunk

def extract_timestamp(line):
    """Extract timestamp from log line"""
    match = re.search(r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)', line)
    return match.group(1) if match else None

def extract_error_message(line):
    """Extract error message from log"""
    # Look for error patterns
    if 'ErrorResp' in line:
        match = re.search(r'ErrorResp\(ErrorPayload \{([^}]+)', line)
        return match.group(1) if match else "ErrorResp"
    if 'Invalid request' in line:
        return "Invalid request"
    if 'Contract call failed' in line:
        return "Contract call failed"
    return None

def analyze_chunk(lines, chunk_num):
    """Analyze a chunk of log lines"""
    results = {
        'chunk': chunk_num,
        'line_range': f"{chunk_num * CHUNK_SIZE}-{(chunk_num + 1) * CHUNK_SIZE}",
        'timestamps': [],
        'post_settle_requests': 0,
        'eth_sendRawTransaction': 0,
        'poller_events': 0,
        'errors': [],
        'nonce_increments': [],
        'tx_hashes': [],
    }

    for line in lines:
        # Extract timestamp
        ts = extract_timestamp(line)
        if ts and ts not in results['timestamps']:
            results['timestamps'].append(ts)

        # Count POST /settle requests
        if 'POST /settle' in line and 'post_settle' in line:
            results['post_settle_requests'] += 1

        # Find eth_sendRawTransaction (actual transaction submission)
        if 'eth_sendRawTransaction' in line:
            results['eth_sendRawTransaction'] += 1
            # Try to extract tx hash from response
            tx_match = re.search(r'0x[0-9a-f]{64}', line)
            if tx_match:
                results['tx_hashes'].append(tx_match.group(0))

        # Count polling events
        if 'poller' in line and 'eth_blockNumber' in line:
            results['poller_events'] += 1

        # Find nonce increments
        if 'incrementing nonce' in line:
            nonce_match = re.search(r'current_nonce=(\d+)', line)
            if nonce_match:
                results['nonce_increments'].append(int(nonce_match.group(1)))

        # Extract errors
        error = extract_error_message(line)
        if error:
            results['errors'].append(error)

    return results

def main():
    print("=" * 80)
    print("FACILITATOR LOG ANALYSIS - CHUNKED")
    print("=" * 80)
    print()

    all_results = []

    with open(LOG_FILE, 'r', encoding='utf-8') as f:
        chunk_num = 0
        while True:
            lines = [f.readline() for _ in range(CHUNK_SIZE)]
            lines = [l for l in lines if l]  # Remove empty lines

            if not lines:
                break

            results = analyze_chunk(lines, chunk_num)
            all_results.append(results)
            chunk_num += 1

    # Print summary for each chunk
    print(f"Total chunks analyzed: {len(all_results)}")
    print()

    for result in all_results:
        if result['post_settle_requests'] > 0 or result['eth_sendRawTransaction'] > 0:
            print(f"\n{'='*80}")
            print(f"CHUNK {result['chunk']} (lines {result['line_range']})")
            print(f"{'='*80}")
            print(f"  Timestamps: {result['timestamps'][0] if result['timestamps'] else 'None'} ... {result['timestamps'][-1] if len(result['timestamps']) > 1 else ''}")
            print(f"  POST /settle requests: {result['post_settle_requests']}")
            print(f"  eth_sendRawTransaction calls: {result['eth_sendRawTransaction']}")
            print(f"  Poller events (waiting for confirmation): {result['poller_events']}")
            print(f"  Nonce increments: {result['nonce_increments']}")
            print(f"  Transaction hashes found: {len(result['tx_hashes'])}")
            if result['tx_hashes']:
                for tx in result['tx_hashes']:
                    print(f"    - {tx}")
            if result['errors']:
                print(f"  Errors: {len(result['errors'])}")
                for err in set(result['errors']):
                    print(f"    - {err}")

    # Global summary
    print(f"\n{'='*80}")
    print("GLOBAL SUMMARY")
    print(f"{'='*80}")

    total_settle_requests = sum(r['post_settle_requests'] for r in all_results)
    total_tx_submissions = sum(r['eth_sendRawTransaction'] for r in all_results)
    total_polling = sum(r['poller_events'] for r in all_results)
    all_nonces = [n for r in all_results for n in r['nonce_increments']]
    all_tx_hashes = [tx for r in all_results for tx in r['tx_hashes']]

    print(f"Total POST /settle requests: {total_settle_requests}")
    print(f"Total eth_sendRawTransaction: {total_tx_submissions}")
    print(f"Total polling events: {total_polling}")
    print(f"Nonces used: {sorted(set(all_nonces))}")
    print(f"Transaction hashes: {len(all_tx_hashes)}")
    for tx in all_tx_hashes:
        print(f"  - https://basescan.org/tx/{tx}")

    # Compare with BaseScan info
    print(f"\n{'='*80}")
    print("BASESCAN CORRELATION")
    print(f"{'='*80}")
    print("From your screenshot, facilitator wallet had transactions around block 37621104")
    print("One transaction has ERROR icon: 0x2030cc306c...")
    print()
    print("Expected from load test: 30 requests")
    print(f"Actual settlements attempted (from logs): {total_settle_requests}")
    print(f"Actual transactions submitted on-chain: {total_tx_submissions}")
    print()

    if total_polling > 0:
        avg_polls_per_tx = total_polling / max(total_tx_submissions, 1)
        print(f"Average polling events per transaction: {avg_polls_per_tx:.1f}")
        print(f"Estimated wait time per tx: {avg_polls_per_tx * 7:.0f}s (7s poll interval)")

if __name__ == "__main__":
    main()
