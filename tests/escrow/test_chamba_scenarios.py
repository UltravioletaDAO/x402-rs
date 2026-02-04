#!/usr/bin/env python3
"""
Chamba Integration Test: Simulate all payment scenarios from Chamba's perspective.

This test simulates what Chamba would do when an AI agent posts a task,
using the Advanced Escrow (PaymentOperator) system.

Tests 4 Chamba payment scenarios on Base Mainnet:
1. Standard task (AUTHORIZE -> RELEASE)
2. Cancelled task (AUTHORIZE -> REFUND IN ESCROW)
3. Micro-task instant payment (CHARGE)
4. Quality dispute (AUTHORIZE -> RELEASE -> REFUND POST ESCROW attempt)

Note: Partial release (AUTHORIZE -> partial RELEASE + REFUND) is implemented
in the SDK and Chamba integration but not tested here to minimize gas costs.
"""

import json
import os
import sys
import secrets
import time
import requests

from eth_account import Account
from web3 import Web3

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from test_escrow_with_correct_nonce import (
    NETWORK, FACILITATOR_URL,
    compute_correct_nonce, sign_erc3009, get_private_key
)

# Load ABI
ABI_PATH = os.path.join(os.path.dirname(__file__), "..", "..", "abi", "PaymentOperator.json")
with open(ABI_PATH) as f:
    raw = json.load(f)
    OPERATOR_ABI = raw["abi"] if isinstance(raw, dict) else raw

RPC_URL = os.environ.get("RPC_URL_BASE", "https://mainnet.base.org")

# Chamba fee configuration
PLATFORM_FEE_BPS = 800  # 8%


def build_payment_info(receiver, amount, salt, tier="standard"):
    """Build PaymentInfo based on Chamba task tier."""
    now = int(time.time())

    tiers = {
        "micro":      {"pre": 3600,   "auth": 7200,    "refund": 86400},
        "standard":   {"pre": 7200,   "auth": 86400,   "refund": 604800},
        "premium":    {"pre": 14400,  "auth": 172800,  "refund": 1209600},
        "enterprise": {"pre": 86400,  "auth": 604800,  "refund": 2592000},
    }
    t = tiers.get(tier, tiers["standard"])

    return {
        "operator": NETWORK["operator"],
        "receiver": receiver,
        "token": NETWORK["usdc"],
        "maxAmount": str(amount),
        "preApprovalExpiry": now + t["pre"],
        "authorizationExpiry": now + t["auth"],
        "refundExpiry": now + t["refund"],
        "minFeeBps": 0,
        "maxFeeBps": PLATFORM_FEE_BPS,
        "feeReceiver": NETWORK["operator"],
        "salt": salt,
    }


def authorize_via_facilitator(private_key, payer, receiver, amount, payment_info):
    """AUTHORIZE: Lock funds in escrow via facilitator."""
    nonce = compute_correct_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)
    auth = {
        "from": payer,
        "to": NETWORK["token_collector"],
        "value": str(amount),
        "validAfter": "0",
        "validBefore": str(payment_info["preApprovalExpiry"]),
        "nonce": nonce,
    }
    signature = sign_erc3009(private_key, auth, NETWORK["chain_id"])
    payload = {
        "x402Version": 2,
        "scheme": "escrow",
        "payload": {
            "authorization": auth,
            "signature": signature,
            "paymentInfo": payment_info,
        },
        "paymentRequirements": {
            "scheme": "escrow",
            "network": f"eip155:{NETWORK['chain_id']}",
            "maxAmountRequired": str(amount),
            "asset": NETWORK["usdc"],
            "payTo": receiver,
            "extra": {
                "escrowAddress": NETWORK["escrow"],
                "operatorAddress": NETWORK["operator"],
                "tokenCollector": NETWORK["token_collector"],
            },
        },
    }
    response = requests.post(f"{FACILITATOR_URL}/settle", json=payload, timeout=120)
    response.raise_for_status()
    return response.json()


def build_payment_tuple(payment_info, payer, salt):
    """Build the on-chain PaymentInfo tuple."""
    return (
        Web3.to_checksum_address(NETWORK["operator"]),
        Web3.to_checksum_address(payer),
        Web3.to_checksum_address(payment_info["receiver"]),
        Web3.to_checksum_address(NETWORK["usdc"]),
        int(payment_info["maxAmount"]),
        payment_info["preApprovalExpiry"],
        payment_info["authorizationExpiry"],
        payment_info["refundExpiry"],
        0, PLATFORM_FEE_BPS,
        Web3.to_checksum_address(NETWORK["operator"]),
        int(salt, 16),
    )


def send_tx(w3, func_call, payer, private_key, gas=250000):
    """Build, sign, and send a transaction."""
    tx = func_call.build_transaction({
        "from": payer,
        "nonce": w3.eth.get_transaction_count(payer),
        "gas": gas,
        "maxFeePerGas": w3.eth.gas_price * 2,
        "maxPriorityFeePerGas": w3.eth.gas_price,
    })
    signed = w3.eth.account.sign_transaction(tx, private_key)
    tx_hash = w3.eth.send_raw_transaction(signed.raw_transaction)
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
    return receipt, tx_hash


# ============================================================
# SCENARIO TESTS
# ============================================================


def test_scenario_1_standard_task():
    """
    Scenario 1: Standard task - Agent posts, worker completes, agent approves.
    Flow: AUTHORIZE -> RELEASE
    """
    print("\n" + "=" * 70)
    print("SCENARIO 1: Standard Task (AUTHORIZE -> RELEASE)")
    print("Agent: TravelBot | Worker: Maria | Bounty: $0.01 USDC")
    print("Task: 'Verify if Cafe Velvet is open'")
    print("=" * 70)

    pk = get_private_key()
    account = Account.from_key(pk)
    payer = account.address
    receiver = payer  # Self for testing
    w3 = Web3(Web3.HTTPProvider(RPC_URL))

    amount = 10000
    salt = "0x" + secrets.token_hex(32)
    pi = build_payment_info(receiver, amount, salt, tier="micro")

    # Step 1: Agent publishes task -> AUTHORIZE
    print("\n  [1/2] Agent publishes task, locking bounty...")
    result = authorize_via_facilitator(pk, payer, receiver, amount, pi)
    if not result.get("success"):
        print(f"  FAILED: {result.get('errorReason')}")
        return False
    print(f"  OK - Funds locked. TX: {result['transaction'][:20]}...")

    time.sleep(5)

    # Step 2: Worker completes, agent approves -> RELEASE
    print("  [2/2] Worker verified cafe is open, agent approves...")
    operator = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["operator"]), abi=OPERATOR_ABI
    )
    pt = build_payment_tuple(pi, payer, salt)
    receipt, tx_hash = send_tx(w3, operator.functions.release(pt, amount), payer, pk)
    if receipt["status"] != 1:
        print(f"  FAILED: Release reverted. Gas: {receipt['gasUsed']}")
        return False
    print(f"  OK - Worker paid! Gas: {receipt['gasUsed']}")
    print("  RESULT: Maria received $0.01 USDC for verifying cafe")
    return True


def test_scenario_2_cancelled_task():
    """
    Scenario 2: Cancelled task - Agent cancels before completion.
    Flow: AUTHORIZE -> REFUND IN ESCROW
    """
    print("\n" + "=" * 70)
    print("SCENARIO 2: Cancelled Task (AUTHORIZE -> REFUND IN ESCROW)")
    print("Agent: EventBot | Bounty: $0.01 USDC")
    print("Task: 'Photograph concert' - Concert cancelled due to rain")
    print("=" * 70)

    pk = get_private_key()
    account = Account.from_key(pk)
    payer = account.address
    receiver = payer
    w3 = Web3(Web3.HTTPProvider(RPC_URL))

    amount = 10000
    salt = "0x" + secrets.token_hex(32)
    pi = build_payment_info(receiver, amount, salt, tier="standard")

    # Step 1: AUTHORIZE
    print("\n  [1/2] Agent posts task, locking bounty...")
    result = authorize_via_facilitator(pk, payer, receiver, amount, pi)
    if not result.get("success"):
        print(f"  FAILED: {result.get('errorReason')}")
        return False
    print(f"  OK - Funds locked. TX: {result['transaction'][:20]}...")

    time.sleep(5)

    # Step 2: Concert cancelled -> REFUND IN ESCROW
    print("  [2/2] Concert cancelled! Agent refunds from escrow...")
    operator = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["operator"]), abi=OPERATOR_ABI
    )
    pt = build_payment_tuple(pi, payer, salt)
    receipt, tx_hash = send_tx(w3, operator.functions.refundInEscrow(pt, amount), payer, pk)
    if receipt["status"] != 1:
        print(f"  FAILED: RefundInEscrow reverted. Gas: {receipt['gasUsed']}")
        return False
    print(f"  OK - Agent refunded! Gas: {receipt['gasUsed']}")
    print("  RESULT: Agent got $0.01 USDC back, will re-post when weather clears")
    return True


def test_scenario_3_instant_payment():
    """
    Scenario 3: Instant micro-task payment.
    Flow: CHARGE (direct, no escrow)
    """
    print("\n" + "=" * 70)
    print("SCENARIO 3: Instant Payment (CHARGE)")
    print("Agent: LogisticsBot | Worker: Juan (95% rep) | Bounty: $0.01 USDC")
    print("Task: 'Quick delivery across town' - Trusted worker, instant pay")
    print("=" * 70)

    pk = get_private_key()
    account = Account.from_key(pk)
    payer = account.address
    receiver = payer
    w3 = Web3(Web3.HTTPProvider(RPC_URL))

    amount = 10000
    salt = "0x" + secrets.token_hex(32)
    pi = build_payment_info(receiver, amount, salt, tier="micro")

    # Compute nonce and sign ERC-3009
    nonce = compute_correct_nonce(NETWORK["chain_id"], NETWORK["escrow"], pi)
    auth = {
        "from": payer,
        "to": NETWORK["token_collector"],
        "value": str(amount),
        "validAfter": "0",
        "validBefore": str(pi["preApprovalExpiry"]),
        "nonce": nonce,
    }
    signature = sign_erc3009(pk, auth, NETWORK["chain_id"])
    collector_data = bytes.fromhex(signature[2:])

    # Single step: CHARGE
    print("\n  [1/1] Agent pays Juan directly (trusted worker)...")
    operator = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["operator"]), abi=OPERATOR_ABI
    )
    pt = build_payment_tuple(pi, payer, salt)
    receipt, tx_hash = send_tx(
        w3,
        operator.functions.charge(
            pt, amount,
            Web3.to_checksum_address(NETWORK["token_collector"]),
            collector_data
        ),
        payer, pk, gas=300000
    )
    if receipt["status"] != 1:
        print(f"  FAILED: Charge reverted. Gas: {receipt['gasUsed']}")
        return False
    print(f"  OK - Instant payment! Gas: {receipt['gasUsed']}")
    print("  RESULT: Juan received $0.01 USDC instantly, no escrow delay")
    return True


def test_scenario_4_full_lifecycle():
    """
    Scenario 4: Full lifecycle - authorize, release, then dispute attempt.
    Flow: AUTHORIZE -> RELEASE -> (REFUND POST ESCROW attempt)
    """
    print("\n" + "=" * 70)
    print("SCENARIO 4: Full Lifecycle with Dispute")
    print("Agent: ShopifyBot | Worker: Diego | Bounty: $0.01 USDC")
    print("Task: 'Product photos' - Worker paid, then quality issue found")
    print("=" * 70)

    pk = get_private_key()
    account = Account.from_key(pk)
    payer = account.address
    receiver = payer
    w3 = Web3(Web3.HTTPProvider(RPC_URL))

    amount = 10000
    salt = "0x" + secrets.token_hex(32)
    pi = build_payment_info(receiver, amount, salt, tier="standard")

    # Step 1: AUTHORIZE
    print("\n  [1/3] Agent posts task, locking bounty...")
    result = authorize_via_facilitator(pk, payer, receiver, amount, pi)
    if not result.get("success"):
        print(f"  FAILED: {result.get('errorReason')}")
        return False
    print(f"  OK - Funds locked. TX: {result['transaction'][:20]}...")

    time.sleep(5)

    # Step 2: RELEASE
    print("  [2/3] Worker submits photos, agent approves...")
    operator = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["operator"]), abi=OPERATOR_ABI
    )
    pt = build_payment_tuple(pi, payer, salt)
    receipt, _ = send_tx(w3, operator.functions.release(pt, amount), payer, pk)
    if receipt["status"] != 1:
        print(f"  FAILED: Release reverted. Gas: {receipt['gasUsed']}")
        return False
    print(f"  OK - Worker paid! Gas: {receipt['gasUsed']}")

    time.sleep(5)

    # Step 3: REFUND POST ESCROW (attempt)
    print("  [3/3] Agent discovers quality issue, initiates dispute...")
    ZERO = "0x0000000000000000000000000000000000000000"
    try:
        receipt, _ = send_tx(
            w3,
            operator.functions.refundPostEscrow(
                pt, amount,
                Web3.to_checksum_address(ZERO), b""
            ),
            payer, pk
        )
        if receipt["status"] == 1:
            print(f"  OK - RefundPostEscrow succeeded! Gas: {receipt['gasUsed']}")
        else:
            print(f"  INFO - Reverted as expected (needs RefundRequest). Gas: {receipt['gasUsed']}")
    except Exception:
        print("  INFO - RefundPostEscrow reverted (expected without RefundRequest)")
        print("  In production: arbitration panel reviews dispute")

    print("  RESULT: Steps 1-2 worked. Step 3 needs RefundRequest contract.")
    return True


def main():
    print("""
+======================================================================+
|                                                                      |
|           CHAMBA x ADVANCED ESCROW: INTEGRATION TESTS                |
|                                                                      |
|   Simulating real Chamba payment flows on Base Mainnet               |
|                                                                      |
+======================================================================+
    """)

    scenarios = [
        ("Scenario 1: Standard Task", test_scenario_1_standard_task),
        ("Scenario 2: Cancelled Task", test_scenario_2_cancelled_task),
        ("Scenario 3: Instant Payment", test_scenario_3_instant_payment),
        ("Scenario 4: Full Lifecycle", test_scenario_4_full_lifecycle),
    ]

    results = []
    for name, test_fn in scenarios:
        try:
            success = test_fn()
        except Exception as e:
            print(f"  ERROR: {e}")
            success = False
        results.append((name, success))
        time.sleep(5)

    # Summary
    print("\n" + "=" * 70)
    print("                    CHAMBA INTEGRATION SUMMARY")
    print("=" * 70)
    passed = 0
    for name, success in results:
        status = "PASS" if success else "FAIL"
        print(f"  [{status}] {name}")
        if success:
            passed += 1
    print("=" * 70)
    print(f"  Total: {passed}/{len(results)} scenarios passed")
    print("=" * 70)

    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
