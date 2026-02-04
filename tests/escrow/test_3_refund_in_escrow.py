#!/usr/bin/env python3
"""
Test #3: REFUND IN ESCROW - Return escrowed funds to payer (cancel task)

CHAMBA SCENARIO: Agent cancels task, funds return from escrow
=============================================================

In the PaymentOperator contract:
  - refundInEscrow(paymentInfo, uint120 amount) -> calls ESCROW.partialVoid()
  - Funds move from TokenStore (escrow) -> payer (agent gets refund)

Flow: AUTHORIZE (via facilitator) -> REFUND IN ESCROW (on-chain)

Use cases:
  - No workers accepted the task
  - Agent made a mistake in task description
  - External conditions changed (weather, event cancelled)
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

# Load ABI from compiled contract
ABI_PATH = os.path.join(os.path.dirname(__file__), "..", "..", "abi", "PaymentOperator.json")
with open(ABI_PATH) as f:
    raw = json.load(f)
    OPERATOR_ABI = raw["abi"] if isinstance(raw, dict) else raw

RPC_URL = os.environ.get("RPC_URL_BASE", "https://mainnet.base.org")


def authorize_via_facilitator(private_key, payer, receiver, amount, payment_info):
    """Use facilitator to AUTHORIZE funds into escrow."""
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
    return response.json()


def test_refund_in_escrow_flow():
    print("\n" + "=" * 60)
    print("CHAMBA: Agent cancels task, refunding from escrow")
    print("Flow: AUTHORIZE (facilitator) -> REFUND IN ESCROW (on-chain)")
    print("=" * 60 + "\n")

    private_key = get_private_key()
    account = Account.from_key(private_key)
    payer = account.address
    receiver = payer
    w3 = Web3(Web3.HTTPProvider(RPC_URL))

    amount = 10000  # 0.01 USDC
    salt = "0x" + secrets.token_hex(32)
    now = int(time.time())

    payment_info = {
        "operator": NETWORK["operator"],
        "receiver": receiver,
        "token": NETWORK["usdc"],
        "maxAmount": str(amount),
        "preApprovalExpiry": now + 3600,
        "authorizationExpiry": now + 86400,
        "refundExpiry": now + 604800,
        "minFeeBps": 0,
        "maxFeeBps": 100,
        "feeReceiver": NETWORK["operator"],
        "salt": salt,
    }

    # Step 1: AUTHORIZE via facilitator
    print("[Step 1] AUTHORIZE via facilitator...")
    result = authorize_via_facilitator(private_key, payer, receiver, amount, payment_info)
    if not result.get("success"):
        print(f"   [FAILED] {result.get('errorReason')}")
        return False
    print(f"   [OK] TX: {result.get('transaction')}")

    time.sleep(5)

    # Step 2: REFUND IN ESCROW (return funds to payer)
    print("\n[Step 2] REFUND IN ESCROW (return to payer)...")

    payment_info_tuple = (
        Web3.to_checksum_address(NETWORK["operator"]),
        Web3.to_checksum_address(payer),
        Web3.to_checksum_address(receiver),
        Web3.to_checksum_address(NETWORK["usdc"]),
        amount,
        payment_info["preApprovalExpiry"],
        payment_info["authorizationExpiry"],
        payment_info["refundExpiry"],
        0, 100,
        Web3.to_checksum_address(NETWORK["operator"]),
        int(salt, 16),
    )

    operator = w3.eth.contract(
        address=Web3.to_checksum_address(NETWORK["operator"]),
        abi=OPERATOR_ABI
    )

    # refundInEscrow(paymentInfo, uint120 amount) - returns escrowed funds to payer
    tx = operator.functions.refundInEscrow(
        payment_info_tuple,
        amount,
    ).build_transaction({
        "from": payer,
        "nonce": w3.eth.get_transaction_count(payer),
        "gas": 250000,
        "maxFeePerGas": w3.eth.gas_price * 2,
        "maxPriorityFeePerGas": w3.eth.gas_price,
    })

    signed_tx = w3.eth.account.sign_transaction(tx, private_key)
    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f"   TX: https://basescan.org/tx/{tx_hash.hex()}")

    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
    if receipt["status"] != 1:
        print(f"   [FAILED] RefundInEscrow reverted! Gas: {receipt['gasUsed']}")
        return False

    print(f"   [OK] Funds returned to agent! Gas: {receipt['gasUsed']}")
    print("\n" + "=" * 60)
    print("SUCCESS! AUTHORIZE -> REFUND IN ESCROW completed")
    print("(Funds returned from escrow to payer)")
    print("=" * 60)
    return True


if __name__ == "__main__":
    success = test_refund_in_escrow_flow()
    sys.exit(0 if success else 1)
