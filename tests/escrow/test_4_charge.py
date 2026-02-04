#!/usr/bin/env python3
"""
Test #4: CHARGE - Direct payment (no escrow hold)

CHAMBA SCENARIO: Instant payment for completed micro-task
=========================================================

In the PaymentOperator contract:
  - charge(paymentInfo, amount, tokenCollector, collectorData) -> calls ESCROW.charge()
  - CHARGE is an ALTERNATIVE to AUTHORIZE, NOT a follow-up
  - Funds go directly from payer -> receiver (no escrow hold)
  - Useful for instant payments where no escrow period is needed

Flow: CHARGE only (single step, funds transfer immediately)

Use cases:
  - Micro-tasks under $5 that don't need dispute protection
  - Repeat trusted workers with established track record
  - Time-sensitive payments where escrow delay is unacceptable
"""

import json
import os
import sys
import secrets
import time

from eth_account import Account
from web3 import Web3

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from test_escrow_with_correct_nonce import (
    NETWORK,
    compute_correct_nonce, sign_erc3009, get_private_key
)

# Load ABI from compiled contract
ABI_PATH = os.path.join(os.path.dirname(__file__), "..", "..", "abi", "PaymentOperator.json")
with open(ABI_PATH) as f:
    raw = json.load(f)
    OPERATOR_ABI = raw["abi"] if isinstance(raw, dict) else raw

RPC_URL = os.environ.get("RPC_URL_BASE", "https://mainnet.base.org")


def test_charge_flow():
    print("\n" + "=" * 60)
    print("CHAMBA: Direct instant payment (no escrow)")
    print("Flow: CHARGE (single step, funds go directly to receiver)")
    print("=" * 60 + "\n")

    private_key = get_private_key()
    account = Account.from_key(private_key)
    payer = account.address
    receiver = payer  # Self-payment for testing
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

    # Compute the nonce (same as authorize - based on PaymentInfo hash)
    nonce = compute_correct_nonce(NETWORK["chain_id"], NETWORK["escrow"], payment_info)

    # Sign ERC-3009 ReceiveWithAuthorization for the TokenCollector
    auth = {
        "from": payer,
        "to": NETWORK["token_collector"],
        "value": str(amount),
        "validAfter": "0",
        "validBefore": str(payment_info["preApprovalExpiry"]),
        "nonce": nonce,
    }
    signature = sign_erc3009(private_key, auth, NETWORK["chain_id"])
    collector_data = bytes.fromhex(signature[2:])

    print(f"   Payer: {payer}")
    print(f"   Amount: {amount / 1e6} USDC")
    print(f"   Salt: {salt[:20]}...")

    # Build PaymentInfo tuple (with payer)
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

    # charge(paymentInfo, amount, tokenCollector, collectorData)
    # TokenCollector collects USDC via ERC-3009, then funds go directly to receiver
    print("\n[Step 1] CHARGE - Direct payment via PaymentOperator...")
    tx = operator.functions.charge(
        payment_info_tuple,
        amount,
        Web3.to_checksum_address(NETWORK["token_collector"]),
        collector_data,
    ).build_transaction({
        "from": payer,
        "nonce": w3.eth.get_transaction_count(payer),
        "gas": 300000,
        "maxFeePerGas": w3.eth.gas_price * 2,
        "maxPriorityFeePerGas": w3.eth.gas_price,
    })

    signed_tx = w3.eth.account.sign_transaction(tx, private_key)
    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f"   TX: https://basescan.org/tx/{tx_hash.hex()}")

    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
    if receipt["status"] != 1:
        print(f"   [FAILED] Charge reverted! Gas: {receipt['gasUsed']}")
        return False

    print(f"   [OK] Direct payment completed! Gas: {receipt['gasUsed']}")
    print("\n" + "=" * 60)
    print("SUCCESS! CHARGE completed (single-step direct payment)")
    print("(No escrow hold - funds went directly to receiver)")
    print("=" * 60)
    return True


if __name__ == "__main__":
    success = test_charge_flow()
    sys.exit(0 if success else 1)
