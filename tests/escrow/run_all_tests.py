#!/usr/bin/env python3
"""
Run all 5 escrow lifecycle tests.

ESCROW LIFECYCLE FLOWS (PaymentOperator contract):
1. AUTHORIZE          - Lock funds in escrow (via facilitator)
2. RELEASE            - Capture funds from escrow to receiver (after authorize)
3. REFUND IN ESCROW   - Return escrowed funds to payer (cancel, after authorize)
4. CHARGE             - Direct payment, no escrow hold (standalone)
5. REFUND POST ESCROW - Dispute resolution after release (complex)

Contract function mapping:
  operator.authorize()        -> escrow.authorize()   (lock funds)
  operator.release()          -> escrow.capture()      (release to receiver)
  operator.refundInEscrow()   -> escrow.partialVoid()  (return to payer)
  operator.charge()           -> escrow.charge()       (direct payment)
  operator.refundPostEscrow() -> escrow.refund()       (dispute refund)

Each test uses 0.01 USDC on Base Mainnet.
Total cost: ~0.05 USDC + gas fees (~$0.01-0.02 per test)
"""

import subprocess
import sys
import os
import time

TESTS = [
    ("test_escrow_with_correct_nonce.py", "1. AUTHORIZE", "Lock funds in escrow (via facilitator)"),
    ("test_2_release.py", "2. RELEASE", "Capture escrowed funds to receiver"),
    ("test_3_refund_in_escrow.py", "3. REFUND IN ESCROW", "Return escrowed funds to payer"),
    ("test_4_charge.py", "4. CHARGE", "Direct payment (no escrow hold)"),
    ("test_5_refund_post_escrow.py", "5. REFUND POST ESCROW", "Dispute after release"),
]


def run_test(script_name, title, description):
    """Run a single test and return success status."""
    print(f"\n{'=' * 70}")
    print(f"  {title}: {description}")
    print(f"  Script: {script_name}")
    print(f"{'=' * 70}\n")

    script_path = os.path.join(os.path.dirname(__file__), script_name)

    try:
        result = subprocess.run(
            [sys.executable, script_path],
            capture_output=False,
            timeout=180,  # 3 minute timeout per test
        )
        return result.returncode == 0
    except subprocess.TimeoutExpired:
        print(f"[TIMEOUT] Test took longer than 180 seconds")
        return False
    except Exception as e:
        print(f"[ERROR] Failed to run test: {e}")
        return False


def main():
    print("""
+======================================================================+
|                                                                      |
|           x402r ADVANCED ESCROW - FULL LIFECYCLE TESTS               |
|                                                                      |
|   Testing all 5 escrow flows on Base Mainnet via PaymentOperator     |
|                                                                      |
|   CHAMBA USE CASES:                                                  |
|   1. AUTHORIZE          -> Agent posts task, locks bounty            |
|   2. RELEASE            -> Worker completes, agent approves (capture)|
|   3. REFUND IN ESCROW   -> Agent cancels task (return to payer)      |
|   4. CHARGE             -> Direct instant payment (no escrow)        |
|   5. REFUND POST ESCROW -> Quality dispute after release             |
|                                                                      |
|   Contract: operator.release() -> escrow.capture() (NOT charge!)     |
|                                                                      |
+======================================================================+
    """)

    results = []
    start_time = time.time()

    for script, title, desc in TESTS:
        success = run_test(script, title, desc)
        results.append((title, success))

        # Delay between tests to avoid RPC/nonce timing issues
        time.sleep(5)

    elapsed = time.time() - start_time

    # Summary
    print("\n" + "=" * 70)
    print("                         TEST SUMMARY")
    print("=" * 70)

    passed = 0
    for title, success in results:
        status = "[PASS]" if success else "[FAIL]"
        print(f"  {status} {title}")
        if success:
            passed += 1

    print("=" * 70)
    print(f"  Total: {passed}/{len(results)} tests passed")
    print(f"  Time: {elapsed:.1f} seconds")
    print("=" * 70)

    # Contract function reference
    print("""
+======================================================================+
|                     ESCROW FUNCTION REFERENCE                        |
+======================================================================+
|                                                                      |
|  PaymentOperator functions -> AuthCaptureEscrow functions:           |
|  +-----------------------+---------------------------+               |
|  | operator.authorize()  | -> escrow.authorize()     | Lock funds   |
|  | operator.release()    | -> escrow.capture()       | Pay receiver |
|  | operator.refundInEscrow() | -> escrow.partialVoid()| Refund payer|
|  | operator.charge()     | -> escrow.charge()        | Direct pay   |
|  | operator.refundPostEscrow()| -> escrow.refund()   | Dispute      |
|  +-----------------------+---------------------------+               |
|                                                                      |
|  TIMING PARAMETERS (PaymentInfo):                                    |
|  +-- preApprovalExpiry:    How long worker has to accept task        |
|  +-- authorizationExpiry:  How long to complete + approve            |
|  +-- refundExpiry:         Dispute window after release              |
|                                                                      |
+======================================================================+
    """)

    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
