#!/usr/bin/env python3
"""
deploy_operator_skale.py -- Deploy a PaymentOperator on SKALE Base (chain 1187947933)

Replicates what x402r-sdk's deployMarketplaceOperator() does, step by step,
using individual transactions (no Multicall3 batching -- safer for SKALE).

SKALE Base: legacy tx only, gas price from RPC, no EIP-1559.

Usage:
    export DEPLOYER_PRIVATE_KEY="0x..."
    python3 scripts/deploy_operator_skale.py [--dry-run]
"""

import argparse
import json
import os
import sys

try:
    from web3 import Web3
except ImportError:
    print("Error: web3 not installed. Run: pip install web3")
    sys.exit(1)

# ===========================================================================
# Configuration
# ===========================================================================

RPC_URL = "https://skale-base.skalenodes.com/v1/base"
CHAIN_ID = 1187947933

FEE_RECIPIENT = Web3.to_checksum_address("0x103040545AC5031A11E8C03dd11324C7333a13C7")
ESCROW_PERIOD_SECONDS = 7 * 24 * 3600  # 604800 = 7 days
AUTHORIZED_CODEHASH = b'\x00' * 32

ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

# CREATE3 unified factory addresses (Shanghai redeploy 2026-03-26)
FACTORIES = {
    "paymentOperator":        "0xA13AD07eD53BFF6c4e9e6478C3A8FFA4D096B5A3",
    "escrowPeriod":           "0xCf84F213d6e1b2E2dc0DbCBd7d81FaAC305d4E96",
    "refundRequest":          "0x7996b1E7B5B28AF85093dcE3AE73b128133D3715",
    "refundRequestEvidence":  "0xa454D7e0D521176c998309E4E6828156870EDf4B",
    "staticAddressCondition": "0xf9739BB422C93A9705cC636BA9D35B97F721e782",
    "orCondition":            "0xefaD31Ab2a17092Bb4350C84324D59C80CeBB9F4",
}

SINGLETONS = {
    "receiver":  "0xd14242a812F9C7C81869F01867453e571cacEaba",
}

PROTOCOL = {
    "usdcTvlLimit": "0x6CAcA05D19312d28787e93ad4249508ED11198be",
}

# ===========================================================================
# Minimal ABIs
# ===========================================================================

def make_factory_abi(deploy_inputs, deploy_output="address"):
    return [
        {"type": "function", "name": "deploy", "inputs": deploy_inputs,
         "outputs": [{"name": "", "type": deploy_output}], "stateMutability": "nonpayable"},
        {"type": "function", "name": "getDeployed", "inputs": deploy_inputs,
         "outputs": [{"name": "", "type": "address"}], "stateMutability": "view"},
        {"type": "function", "name": "computeAddress", "inputs": deploy_inputs,
         "outputs": [{"name": "", "type": "address"}], "stateMutability": "view"},
    ]

ESCROW_PERIOD_ABI = make_factory_abi([
    {"name": "escrowPeriod", "type": "uint256"},
    {"name": "authorizedCodehash", "type": "bytes32"},
])

REFUND_REQUEST_ABI = make_factory_abi([{"name": "arbiter", "type": "address"}])
REFUND_EVIDENCE_ABI = make_factory_abi([{"name": "refundRequest", "type": "address"}])
SAC_ABI = make_factory_abi([{"name": "designatedAddress", "type": "address"}])
OR_CONDITION_ABI = make_factory_abi([{"name": "_conditions", "type": "address[]"}])

CONFIG_COMPONENTS = [
    {"name": f, "type": "address"} for f in [
        "feeRecipient", "feeCalculator", "authorizeCondition", "authorizeRecorder",
        "chargeCondition", "chargeRecorder", "releaseCondition", "releaseRecorder",
        "refundInEscrowCondition", "refundInEscrowRecorder",
        "refundPostEscrowCondition", "refundPostEscrowRecorder",
    ]
]

OPERATOR_FACTORY_ABI = [
    {"type": "function", "name": "deployOperator",
     "inputs": [{"name": "config", "type": "tuple", "components": CONFIG_COMPONENTS}],
     "outputs": [{"name": "", "type": "address"}], "stateMutability": "nonpayable"},
    {"type": "function", "name": "getOperator",
     "inputs": [{"name": "config", "type": "tuple", "components": CONFIG_COMPONENTS}],
     "outputs": [{"name": "", "type": "address"}], "stateMutability": "view"},
    {"type": "function", "name": "computeAddress",
     "inputs": [{"name": "config", "type": "tuple", "components": CONFIG_COMPONENTS}],
     "outputs": [{"name": "", "type": "address"}], "stateMutability": "view"},
]

# ===========================================================================
# Helpers
# ===========================================================================

def send_tx(w3, account, contract_addr, data, label):
    nonce = w3.eth.get_transaction_count(account.address)
    gas_price = w3.eth.gas_price
    tx = {
        "from": account.address,
        "to": Web3.to_checksum_address(contract_addr),
        "data": data,
        "nonce": nonce,
        "chainId": CHAIN_ID,
        "gasPrice": gas_price,
    }
    try:
        gas = w3.eth.estimate_gas(tx)
        tx["gas"] = int(gas * 1.3)
    except Exception as e:
        print(f"  [WARN] Gas estimate failed for {label}: {e}")
        tx["gas"] = 8_000_000

    signed = account.sign_transaction(tx)
    tx_hash = w3.eth.send_raw_transaction(signed.raw_transaction)
    print(f"  [{label}] tx: {tx_hash.hex()}")
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
    if receipt["status"] != 1:
        print(f"  [FAIL] {label} REVERTED!")
        sys.exit(1)
    print(f"  [{label}] confirmed block {receipt['blockNumber']} gas {receipt['gasUsed']}")
    return receipt


def deploy_via_factory(w3, account, factory, deploy_args, label, dry_run=False):
    existing = factory.functions.getDeployed(*deploy_args).call()
    if existing != ZERO_ADDRESS:
        print(f"  [{label}] already deployed: {existing}")
        return Web3.to_checksum_address(existing)

    predicted = factory.functions.computeAddress(*deploy_args).call()
    print(f"  [{label}] predicted: {predicted}")

    if dry_run:
        return Web3.to_checksum_address(predicted)

    tx_data = factory.functions.deploy(*deploy_args).build_transaction({
        "from": account.address, "chainId": CHAIN_ID,
        "gasPrice": w3.eth.gas_price,
        "nonce": w3.eth.get_transaction_count(account.address),
    })
    send_tx(w3, account, factory.address, tx_data["data"], label)

    deployed = factory.functions.getDeployed(*deploy_args).call()
    if deployed == ZERO_ADDRESS:
        print(f"  [FAIL] {label} verification failed!")
        sys.exit(1)
    print(f"  [{label}] deployed: {deployed}")
    return Web3.to_checksum_address(deployed)


# ===========================================================================
# Main
# ===========================================================================

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--arbiter", type=str, default=None)
    args = parser.parse_args()

    arbiter = Web3.to_checksum_address(args.arbiter) if args.arbiter else FEE_RECIPIENT

    print("=" * 60)
    print("PaymentOperator Deployment -- SKALE Base")
    print("=" * 60)
    print(f"  Fee Recipient:  {FEE_RECIPIENT}")
    print(f"  Arbiter:        {arbiter}")
    print(f"  Escrow Period:  {ESCROW_PERIOD_SECONDS}s (7 days)")
    print()

    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    assert w3.is_connected(), "Cannot connect to RPC"
    print(f"[OK] Connected (block {w3.eth.block_number})")

    private_key = os.environ.get("DEPLOYER_PRIVATE_KEY")
    if not private_key and not args.dry_run:
        print("[FAIL] Set DEPLOYER_PRIVATE_KEY")
        sys.exit(1)

    account = w3.eth.account.from_key(private_key) if private_key else None
    if account:
        bal = w3.eth.get_balance(account.address)
        print(f"[OK] Deployer: {account.address} ({w3.from_wei(bal, 'ether')} CREDIT)")

    # Verify factories exist
    print("\n[1] Verifying factories...")
    for name, addr in FACTORIES.items():
        code = w3.eth.get_code(Web3.to_checksum_address(addr))
        assert len(code) > 0, f"{name} has no bytecode at {addr}"
        print(f"  {name}: OK ({len(code)} bytes)")

    # Contracts
    ep_factory = w3.eth.contract(address=Web3.to_checksum_address(FACTORIES["escrowPeriod"]), abi=ESCROW_PERIOD_ABI)
    rr_factory = w3.eth.contract(address=Web3.to_checksum_address(FACTORIES["refundRequest"]), abi=REFUND_REQUEST_ABI)
    re_factory = w3.eth.contract(address=Web3.to_checksum_address(FACTORIES["refundRequestEvidence"]), abi=REFUND_EVIDENCE_ABI)
    sac_factory = w3.eth.contract(address=Web3.to_checksum_address(FACTORIES["staticAddressCondition"]), abi=SAC_ABI)
    or_factory = w3.eth.contract(address=Web3.to_checksum_address(FACTORIES["orCondition"]), abi=OR_CONDITION_ABI)
    op_factory = w3.eth.contract(address=Web3.to_checksum_address(FACTORIES["paymentOperator"]), abi=OPERATOR_FACTORY_ABI)

    # Step 1: EscrowPeriod
    print("\n[2] EscrowPeriod...")
    ep_addr = deploy_via_factory(w3, account, ep_factory,
        [ESCROW_PERIOD_SECONDS, AUTHORIZED_CODEHASH], "EscrowPeriod", args.dry_run)

    # Step 2: RefundRequest
    print("\n[3] RefundRequest...")
    rr_addr = deploy_via_factory(w3, account, rr_factory,
        [arbiter], "RefundRequest", args.dry_run)

    # Step 3: RefundRequestEvidence
    print("\n[4] RefundRequestEvidence...")
    re_addr = deploy_via_factory(w3, account, re_factory,
        [rr_addr], "RefundEvidence", args.dry_run)

    # Step 4: StaticAddressCondition(arbiter)
    print("\n[5] StaticAddressCondition(arbiter)...")
    sac_addr = deploy_via_factory(w3, account, sac_factory,
        [arbiter], "SAC(arbiter)", args.dry_run)

    # Step 5: OrCondition([receiver, SAC(arbiter)])
    print("\n[6] OrCondition([receiver, SAC(arbiter)])...")
    receiver = Web3.to_checksum_address(SINGLETONS["receiver"])
    or_addr = deploy_via_factory(w3, account, or_factory,
        [[receiver, sac_addr]], "OrCondition", args.dry_run)

    # Step 6: PaymentOperator
    print("\n[7] PaymentOperator...")
    config = (
        Web3.to_checksum_address(FEE_RECIPIENT),
        Web3.to_checksum_address(ZERO_ADDRESS),           # feeCalculator (none)
        Web3.to_checksum_address(PROTOCOL["usdcTvlLimit"]),  # authorizeCondition
        Web3.to_checksum_address(ep_addr),                # authorizeRecorder
        Web3.to_checksum_address(ZERO_ADDRESS),           # chargeCondition
        Web3.to_checksum_address(ZERO_ADDRESS),           # chargeRecorder
        Web3.to_checksum_address(ep_addr),                # releaseCondition
        Web3.to_checksum_address(ZERO_ADDRESS),           # releaseRecorder
        Web3.to_checksum_address(or_addr),                # refundInEscrowCondition
        Web3.to_checksum_address(rr_addr),                # refundInEscrowRecorder
        Web3.to_checksum_address(receiver),               # refundPostEscrowCondition
        Web3.to_checksum_address(ZERO_ADDRESS),           # refundPostEscrowRecorder
    )

    existing = op_factory.functions.getOperator(config).call()
    if existing != ZERO_ADDRESS:
        print(f"  [OK] Already deployed: {existing}")
        operator_addr = existing
    elif args.dry_run:
        predicted = op_factory.functions.computeAddress(config).call()
        print(f"  [DRY RUN] Predicted: {predicted}")
        operator_addr = predicted
    else:
        tx_data = op_factory.functions.deployOperator(config).build_transaction({
            "from": account.address, "chainId": CHAIN_ID,
            "gasPrice": w3.eth.gas_price,
            "nonce": w3.eth.get_transaction_count(account.address),
        })
        send_tx(w3, account, op_factory.address, tx_data["data"], "Operator")
        operator_addr = op_factory.functions.getOperator(config).call()
        assert operator_addr != ZERO_ADDRESS, "Operator verification failed"
        print(f"  [OK] Operator: {operator_addr}")

    print()
    print("=" * 60)
    print("DONE")
    print("=" * 60)
    print(f"  PaymentOperator: {operator_addr}")
    print(f"  EscrowPeriod:    {ep_addr}")
    print(f"  RefundRequest:   {rr_addr}")
    print()
    print(f"  Add to addresses.rs:")
    print(f'    address!("{operator_addr[2:]}"),')
    print("=" * 60)


if __name__ == "__main__":
    main()
