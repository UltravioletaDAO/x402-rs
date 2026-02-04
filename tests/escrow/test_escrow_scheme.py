#!/usr/bin/env python3
"""
Comprehensive test script for x402r Escrow Scheme.

This script tests the escrow payment flow on the facilitator.
It handles ERC-3009 signing, nonce computation, and payload construction.

Usage:
    python test_escrow_scheme.py --private-key YOUR_PRIVATE_KEY [--network sepolia|mainnet]

Requirements:
    pip install web3 eth-account requests python-dotenv
"""

import argparse
import json
import os
import secrets
import sys
import time
from dataclasses import dataclass
from datetime import datetime
from typing import Optional

import boto3
import requests
from eth_abi import encode
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3

# ============================================================================
# AWS Secrets Manager
# ============================================================================

def get_secret_from_aws(secret_name: str, region: str = "us-east-2") -> str:
    """Retrieve a secret from AWS Secrets Manager."""
    client = boto3.client("secretsmanager", region_name=region)
    response = client.get_secret_value(SecretId=secret_name)
    return response["SecretString"]

# ============================================================================
# Configuration
# ============================================================================

FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"

# Network configurations
NETWORKS = {
    "mainnet": {
        "name": "Base Mainnet",
        "caip2": "eip155:8453",
        "chain_id": 8453,
        "usdc": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        "escrow": "0x320a3c35F131E5D2Fb36af56345726B298936037",
        "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
        "token_collector": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
        "rpc_url": "https://mainnet.base.org",
        # USDC on Base mainnet uses "USD Coin" as domain name
        "usdc_domain_name": "USD Coin",
        "usdc_domain_version": "2",
    },
    "sepolia": {
        "name": "Base Sepolia",
        "caip2": "eip155:84532",
        "chain_id": 84532,
        "usdc": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        "escrow": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
        "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
        "token_collector": "0xC80cd08d609673061597DE7fe54Af3978f10A825",
        "rpc_url": "https://sepolia.base.org",
        # USDC on Base Sepolia - check contract for actual name
        "usdc_domain_name": "USDC",
        "usdc_domain_version": "2",
    },
}

# Constants
MAX_UINT48 = 281474976710655  # Maximum value for uint48 (no expiry)
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

# ============================================================================
# Data Classes
# ============================================================================

@dataclass
class TestResult:
    """Result of a test case."""
    name: str
    passed: bool
    message: str
    response: Optional[dict] = None
    tx_hash: Optional[str] = None

# ============================================================================
# Utility Functions
# ============================================================================

def log(msg: str, level: str = "INFO"):
    """Print log message with timestamp."""
    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{timestamp}] [{level}] {msg}")


def generate_salt() -> str:
    """Generate random 32-byte salt as hex string."""
    return "0x" + secrets.token_hex(32)


def to_checksum(address: str) -> str:
    """Convert address to checksum format."""
    return Web3.to_checksum_address(address)


def get_usdc_balance(w3: Web3, address: str, usdc_address: str) -> int:
    """Get USDC balance for address (in raw units, 6 decimals)."""
    # Minimal ERC20 ABI for balanceOf
    abi = [{"constant": True, "inputs": [{"name": "_owner", "type": "address"}],
            "name": "balanceOf", "outputs": [{"name": "balance", "type": "uint256"}],
            "type": "function"}]
    contract = w3.eth.contract(address=to_checksum(usdc_address), abi=abi)
    return contract.functions.balanceOf(to_checksum(address)).call()


def format_usdc(amount: int) -> str:
    """Format USDC amount with 6 decimals."""
    return f"{amount / 1_000_000:.6f} USDC"

# ============================================================================
# Nonce Computation
# ============================================================================

def compute_escrow_nonce(
    chain_id: int,
    escrow_address: str,
    payment_info: dict,
) -> str:
    """
    Compute the nonce for ERC-3009 authorization.

    The nonce is: keccak256(abi.encode(chainId, escrow, paymentInfoWithZeroPayer))

    paymentInfo uses payer=0x0 for payer-agnostic nonce.

    IMPORTANT: Uses abi.encode() (padded to 32 bytes), NOT abi.encodePacked()!
    The PaymentInfo struct must be encoded as a tuple.
    """
    # PaymentInfo struct components (order matters!)
    # struct PaymentInfo {
    #     address operator;
    #     address payer;           // <-- Use ZERO_ADDRESS
    #     address receiver;
    #     address token;
    #     uint120 maxAmount;
    #     uint48 preApprovalExpiry;
    #     uint48 authorizationExpiry;
    #     uint48 refundExpiry;
    #     uint16 minFeeBps;
    #     uint16 maxFeeBps;
    #     address feeReceiver;
    #     uint256 salt;
    # }

    # Convert salt from hex string to int
    salt = payment_info["salt"]
    if isinstance(salt, str):
        salt = int(salt, 16) if salt.startswith("0x") else int(salt)

    # Build the PaymentInfo tuple with payer = 0x0
    payment_info_tuple = (
        to_checksum(payment_info["operator"]),
        ZERO_ADDRESS,  # payer = 0 for payer-agnostic
        to_checksum(payment_info["receiver"]),
        to_checksum(payment_info["token"]),
        int(payment_info["maxAmount"]),
        int(payment_info["preApprovalExpiry"]),
        int(payment_info["authorizationExpiry"]),
        int(payment_info["refundExpiry"]),
        int(payment_info["minFeeBps"]),
        int(payment_info["maxFeeBps"]),
        to_checksum(payment_info["feeReceiver"]),
        salt,
    )

    # ABI encode: (uint256 chainId, address escrow, PaymentInfo paymentInfo)
    # PaymentInfo is a tuple type
    encoded = encode(
        [
            "uint256",  # chainId
            "address",  # escrow
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",  # PaymentInfo tuple
        ],
        [
            chain_id,
            to_checksum(escrow_address),
            payment_info_tuple,
        ]
    )

    # keccak256 of the encoded data
    nonce_hash = Web3.keccak(encoded)

    # Return with 0x prefix for proper hex encoding
    return "0x" + nonce_hash.hex()

# ============================================================================
# ERC-3009 Signing
# ============================================================================

def sign_erc3009_authorization(
    private_key: str,
    authorization: dict,
    chain_id: int,
    usdc_address: str,
    domain_name: str,
    domain_version: str,
) -> str:
    """
    Sign ERC-3009 TransferWithAuthorization using EIP-712.

    Returns the signature as a hex string.
    """
    # EIP-712 domain
    domain = {
        "name": domain_name,
        "version": domain_version,
        "chainId": chain_id,
        "verifyingContract": to_checksum(usdc_address),
    }

    # EIP-712 types
    types = {
        "TransferWithAuthorization": [
            {"name": "from", "type": "address"},
            {"name": "to", "type": "address"},
            {"name": "value", "type": "uint256"},
            {"name": "validAfter", "type": "uint256"},
            {"name": "validBefore", "type": "uint256"},
            {"name": "nonce", "type": "bytes32"},
        ],
    }

    # Message to sign
    message = {
        "from": to_checksum(authorization["from"]),
        "to": to_checksum(authorization["to"]),
        "value": int(authorization["value"]),
        "validAfter": int(authorization["validAfter"]),
        "validBefore": int(authorization["validBefore"]),
        "nonce": authorization["nonce"] if authorization["nonce"].startswith("0x") else "0x" + authorization["nonce"],
    }

    # Sign using eth_account
    signable = encode_typed_data(
        domain_data=domain,
        message_types=types,
        message_data=message,
    )

    account = Account.from_key(private_key)
    signed = account.sign_message(signable)

    return signed.signature.hex()

# ============================================================================
# Payload Construction
# ============================================================================

def build_escrow_payload(
    payer_address: str,
    receiver_address: str,
    amount_usdc: float,
    network_config: dict,
    private_key: str,
    valid_before: Optional[int] = None,
    salt: Optional[str] = None,
) -> dict:
    """
    Build complete escrow scheme payload for the facilitator.

    Args:
        payer_address: Address of the payer (signer)
        receiver_address: Address that receives the payment
        amount_usdc: Amount in USDC (e.g., 1.0 for 1 USDC)
        network_config: Network configuration dict
        private_key: Payer's private key for signing
        valid_before: Optional expiry timestamp (default: 1 hour from now)
        salt: Optional salt (default: random)

    Returns:
        Complete payload dict ready to send to facilitator
    """
    # Convert amount to raw units (6 decimals)
    amount_raw = int(amount_usdc * 1_000_000)

    # Generate salt if not provided
    if salt is None:
        salt = generate_salt()

    # Set validBefore (default: 1 hour from now)
    if valid_before is None:
        valid_before = int(time.time()) + 3600

    # Build paymentInfo
    payment_info = {
        "operator": network_config["operator"],
        "receiver": receiver_address,
        "token": network_config["usdc"],
        "maxAmount": str(amount_raw),
        "preApprovalExpiry": MAX_UINT48,
        "authorizationExpiry": MAX_UINT48,
        "refundExpiry": MAX_UINT48,
        "minFeeBps": 0,
        "maxFeeBps": 100,  # 1% max fee
        "feeReceiver": network_config["operator"],
        "salt": salt,
    }

    # Compute nonce
    nonce = compute_escrow_nonce(
        chain_id=network_config["chain_id"],
        escrow_address=network_config["escrow"],
        payment_info=payment_info,
    )

    # Build authorization
    authorization = {
        "from": payer_address,
        "to": network_config["token_collector"],
        "value": str(amount_raw),
        "validAfter": "0",
        "validBefore": str(valid_before),
        "nonce": nonce,
    }

    # Sign the authorization
    signature = sign_erc3009_authorization(
        private_key=private_key,
        authorization=authorization,
        chain_id=network_config["chain_id"],
        usdc_address=network_config["usdc"],
        domain_name=network_config["usdc_domain_name"],
        domain_version=network_config["usdc_domain_version"],
    )

    # Build complete payload
    payload = {
        "x402Version": 2,
        "scheme": "escrow",
        "payload": {
            "authorization": authorization,
            "signature": "0x" + signature if not signature.startswith("0x") else signature,
            "paymentInfo": payment_info,
        },
        "paymentRequirements": {
            "scheme": "escrow",
            "network": network_config["caip2"],
            "maxAmountRequired": str(amount_raw),
            "asset": network_config["usdc"],
            "payTo": receiver_address,
            "extra": {
                "escrowAddress": network_config["escrow"],
                "operatorAddress": network_config["operator"],
                "tokenCollector": network_config["token_collector"],
            },
        },
    }

    return payload

# ============================================================================
# Facilitator API
# ============================================================================

def send_settle_request(payload: dict) -> dict:
    """Send settle request to facilitator."""
    url = f"{FACILITATOR_URL}/settle"

    log(f"Sending settle request to {url}")
    log(f"Payload size: {len(json.dumps(payload))} bytes")

    response = requests.post(
        url,
        json=payload,
        headers={"Content-Type": "application/json"},
        timeout=60,
    )

    log(f"Response status: {response.status_code}")

    try:
        return response.json()
    except:
        return {"error": response.text, "status_code": response.status_code}


def check_facilitator_health() -> bool:
    """Check if facilitator is healthy."""
    try:
        response = requests.get(f"{FACILITATOR_URL}/health", timeout=10)
        return response.status_code == 200
    except Exception as e:
        log(f"Health check failed: {e}", "ERROR")
        return False

# ============================================================================
# Test Cases
# ============================================================================

def test_health_check() -> TestResult:
    """Test: Facilitator health check."""
    log("Testing facilitator health...")
    if check_facilitator_health():
        return TestResult("health_check", True, "Facilitator is healthy")
    else:
        return TestResult("health_check", False, "Facilitator is not healthy")


def test_basic_authorize(
    private_key: str,
    network_config: dict,
    receiver_address: str,
    amount_usdc: float = 0.01,
) -> TestResult:
    """Test: Basic authorize flow (happy path)."""
    test_name = f"basic_authorize_{amount_usdc}_usdc"
    log(f"Testing basic authorize with {amount_usdc} USDC...")

    try:
        account = Account.from_key(private_key)
        payer_address = account.address

        payload = build_escrow_payload(
            payer_address=payer_address,
            receiver_address=receiver_address,
            amount_usdc=amount_usdc,
            network_config=network_config,
            private_key=private_key,
        )

        log(f"Payer: {payer_address}")
        log(f"Receiver: {receiver_address}")
        log(f"Amount: {amount_usdc} USDC")

        # Debug output
        log(f"[DEBUG] Nonce: {payload['payload']['authorization']['nonce']}")
        log(f"[DEBUG] To (tokenCollector): {payload['payload']['authorization']['to']}")
        log(f"[DEBUG] Operator: {payload['payload']['paymentInfo']['operator']}")
        log(f"[DEBUG] Salt: {payload['payload']['paymentInfo']['salt']}")
        log(f"[DEBUG] validBefore: {payload['payload']['authorization']['validBefore']}")

        response = send_settle_request(payload)

        if response.get("success"):
            tx_hash = response.get("transaction")
            log(f"SUCCESS! Transaction: {tx_hash}", "SUCCESS")
            return TestResult(test_name, True, "Authorization successful", response, tx_hash)
        else:
            error = response.get("errorReason") or response.get("error") or str(response)
            log(f"FAILED: {error}", "ERROR")
            # Show full response for debugging
            log(f"[DEBUG] Full response: {json.dumps(response, indent=2)}", "DEBUG")
            return TestResult(test_name, False, f"Authorization failed: {error}", response)

    except Exception as e:
        log(f"EXCEPTION: {e}", "ERROR")
        import traceback
        log(f"[DEBUG] Traceback: {traceback.format_exc()}", "DEBUG")
        return TestResult(test_name, False, f"Exception: {e}")


def test_expired_authorization(
    private_key: str,
    network_config: dict,
    receiver_address: str,
) -> TestResult:
    """Test: Expired authorization should fail."""
    test_name = "expired_authorization"
    log("Testing expired authorization...")

    try:
        account = Account.from_key(private_key)
        payer_address = account.address

        # Use a past timestamp
        expired_time = int(time.time()) - 3600  # 1 hour ago

        payload = build_escrow_payload(
            payer_address=payer_address,
            receiver_address=receiver_address,
            amount_usdc=0.01,
            network_config=network_config,
            private_key=private_key,
            valid_before=expired_time,
        )

        response = send_settle_request(payload)

        # This should fail
        if not response.get("success"):
            log("Correctly rejected expired authorization", "SUCCESS")
            return TestResult(test_name, True, "Expired authorization correctly rejected", response)
        else:
            log("UNEXPECTED: Expired authorization was accepted", "ERROR")
            return TestResult(test_name, False, "Expired authorization should have been rejected", response)

    except Exception as e:
        log(f"EXCEPTION: {e}", "ERROR")
        return TestResult(test_name, False, f"Exception: {e}")


def test_wrong_scheme(
    private_key: str,
    network_config: dict,
    receiver_address: str,
) -> TestResult:
    """Test: Wrong scheme should fail."""
    test_name = "wrong_scheme"
    log("Testing wrong scheme...")

    try:
        account = Account.from_key(private_key)

        payload = build_escrow_payload(
            payer_address=account.address,
            receiver_address=receiver_address,
            amount_usdc=0.01,
            network_config=network_config,
            private_key=private_key,
        )

        # Change scheme to something invalid
        payload["scheme"] = "invalid_scheme"

        response = send_settle_request(payload)

        # This should fail or not route to escrow
        if not response.get("success"):
            log("Correctly rejected wrong scheme", "SUCCESS")
            return TestResult(test_name, True, "Wrong scheme correctly rejected", response)
        else:
            log("UNEXPECTED: Wrong scheme was accepted", "ERROR")
            return TestResult(test_name, False, "Wrong scheme should have been rejected", response)

    except Exception as e:
        log(f"EXCEPTION: {e}", "ERROR")
        return TestResult(test_name, False, f"Exception: {e}")


def test_balance_check(
    private_key: str,
    network_config: dict,
) -> TestResult:
    """Test: Check USDC balance of payer."""
    test_name = "balance_check"
    log("Checking USDC balance...")

    try:
        account = Account.from_key(private_key)
        payer_address = account.address

        w3 = Web3(Web3.HTTPProvider(network_config["rpc_url"]))
        balance = get_usdc_balance(w3, payer_address, network_config["usdc"])

        log(f"Payer: {payer_address}")
        log(f"USDC Balance: {format_usdc(balance)}")

        if balance > 0:
            return TestResult(test_name, True, f"Balance: {format_usdc(balance)}")
        else:
            return TestResult(test_name, False, "No USDC balance - cannot run payment tests")

    except Exception as e:
        log(f"EXCEPTION: {e}", "ERROR")
        return TestResult(test_name, False, f"Exception: {e}")

# ============================================================================
# Main Test Runner
# ============================================================================

def run_all_tests(
    private_key: str,
    network: str,
    receiver_address: str,
    skip_real_payments: bool = False,
) -> list[TestResult]:
    """Run all test cases."""
    network_config = NETWORKS[network]
    results = []

    log("=" * 60)
    log(f"x402r Escrow Scheme Test Suite")
    log(f"Network: {network_config['name']} ({network_config['caip2']})")
    log(f"Facilitator: {FACILITATOR_URL}")
    log("=" * 60)

    # Test 1: Health check
    results.append(test_health_check())

    # Test 2: Balance check
    balance_result = test_balance_check(private_key, network_config)
    results.append(balance_result)

    if not balance_result.passed:
        log("Skipping payment tests due to zero balance", "WARN")
        return results

    # Test 3: Wrong scheme (doesn't cost money)
    results.append(test_wrong_scheme(private_key, network_config, receiver_address))

    # Test 4: Expired authorization (doesn't cost money - should fail before tx)
    results.append(test_expired_authorization(private_key, network_config, receiver_address))

    if not skip_real_payments:
        # Test 5: Basic authorize (costs USDC!)
        log("=" * 60)
        log("REAL PAYMENT TESTS (will transfer USDC to escrow)")
        log("=" * 60)
        results.append(test_basic_authorize(private_key, network_config, receiver_address, 0.01))
    else:
        log("Skipping real payment tests (--skip-real-payments)", "WARN")

    return results


def print_results(results: list[TestResult]):
    """Print test results summary."""
    log("=" * 60)
    log("TEST RESULTS SUMMARY")
    log("=" * 60)

    passed = sum(1 for r in results if r.passed)
    total = len(results)

    for r in results:
        status = "PASS" if r.passed else "FAIL"
        print(f"  [{status}] {r.name}: {r.message}")
        if r.tx_hash:
            print(f"         TX: {r.tx_hash}")

    log(f"Results: {passed}/{total} tests passed")

    if passed == total:
        log("All tests passed!", "SUCCESS")
    else:
        log(f"{total - passed} tests failed", "ERROR")

# ============================================================================
# CLI
# ============================================================================

def main():
    parser = argparse.ArgumentParser(
        description="Test x402r Escrow Scheme on the facilitator",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Run all tests on Base Mainnet using AWS Secrets Manager (recommended)
    python test_escrow_scheme.py --secret-name lighthouse-buyer-tester --network mainnet

    # Run all tests on Base Sepolia (testnet)
    python test_escrow_scheme.py --secret-name lighthouse-buyer-tester --network sepolia

    # Skip real payment tests (only run validation tests)
    python test_escrow_scheme.py --secret-name lighthouse-buyer-tester --skip-real-payments

    # Specify a custom receiver address
    python test_escrow_scheme.py --secret-name lighthouse-buyer-tester --receiver 0x...
        """
    )

    parser.add_argument(
        "--secret-name",
        default="lighthouse-buyer-tester",
        help="AWS Secrets Manager secret name containing private key (default: lighthouse-buyer-tester)",
    )
    parser.add_argument(
        "--aws-region",
        default="us-east-2",
        help="AWS region for Secrets Manager (default: us-east-2)",
    )
    parser.add_argument(
        "--network",
        choices=["sepolia", "mainnet"],
        default="sepolia",
        help="Network to test on (default: sepolia)",
    )
    parser.add_argument(
        "--receiver",
        help="Receiver address (default: same as payer)",
    )
    parser.add_argument(
        "--skip-real-payments",
        action="store_true",
        help="Skip tests that make real payments (only run validation tests)",
    )
    args = parser.parse_args()

    # Get private key from AWS Secrets Manager
    log(f"Fetching private key from AWS Secrets Manager: {args.secret_name}")
    try:
        private_key = get_secret_from_aws(args.secret_name, args.aws_region)
        log("Private key retrieved successfully")
    except Exception as e:
        log(f"Failed to get secret from AWS: {e}", "ERROR")
        sys.exit(1)

    # Get payer address from private key
    account = Account.from_key(private_key)
    payer_address = account.address

    # Set receiver (default to payer for self-payment test)
    receiver_address = args.receiver or payer_address

    log(f"Payer address: {payer_address}")
    log(f"Receiver address: {receiver_address}")

    # Run tests
    results = run_all_tests(
        private_key=private_key,
        network=args.network,
        receiver_address=receiver_address,
        skip_real_payments=args.skip_real_payments,
    )

    # Print results
    print_results(results)

    # Exit with appropriate code
    all_passed = all(r.passed for r in results)
    sys.exit(0 if all_passed else 1)


if __name__ == "__main__":
    main()
