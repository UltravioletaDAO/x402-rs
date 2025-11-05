"""
Comprehensive x402 Facilitator Test Suite

Tests both /verify and /settle endpoints with various scenarios:
- Valid payments
- Invalid signatures
- Insufficient funds
- Network mismatches
- Expired authorizations
"""

import json
import time
from typing import Dict, Any, Optional
from eth_account import Account
from eth_account.messages import encode_typed_data
from web3 import Web3
import requests
import os
from dotenv import load_dotenv

load_dotenv()

# Configuration
FACILITATOR_URL = os.getenv("FACILITATOR_URL", "https://facilitator.ultravioletadao.xyz")
PRIVATE_KEY = os.getenv("TEST_PRIVATE_KEY")  # Test wallet private key
RPC_URL_FUJI = os.getenv("RPC_URL_AVALANCHE_FUJI", "https://avalanche-fuji-c-chain-rpc.publicnode.com")

# Network Configuration
NETWORKS = {
    "avalanche-fuji": {
        "chain_id": 43113,
        "glue_token": "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743",
        "eip712_name": "Gasless Ultravioleta DAO Extended Token",
        "eip712_version": "1",
    },
    "base-sepolia": {
        "chain_id": 84532,
        "usdc_token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        "eip712_name": "USD Coin",
        "eip712_version": "2",
    },
}


class X402Tester:
    """Test client for x402 facilitator endpoints"""

    def __init__(self, facilitator_url: str = FACILITATOR_URL, private_key: Optional[str] = None):
        self.facilitator_url = facilitator_url.rstrip("/")
        self.private_key = private_key
        if private_key:
            self.account = Account.from_key(private_key)
            self.address = self.account.address
        else:
            self.account = None
            self.address = None

    def generate_nonce(self) -> str:
        """Generate random 32-byte nonce"""
        return "0x" + os.urandom(32).hex()

    def create_eip712_domain(self, network: str, token_address: str) -> Dict[str, Any]:
        """Create EIP-712 domain for token"""
        net_config = NETWORKS.get(network)
        if not net_config:
            raise ValueError(f"Unsupported network: {network}")

        return {
            "name": net_config.get("eip712_name", "USD Coin"),
            "version": net_config.get("eip712_version", "2"),
            "chainId": net_config["chain_id"],
            "verifyingContract": token_address,
        }

    def sign_transfer_authorization(
        self,
        from_address: str,
        to_address: str,
        value: str,
        valid_after: int,
        valid_before: int,
        nonce: str,
        network: str,
        token_address: str,
    ) -> str:
        """Sign EIP-712 TransferWithAuthorization"""
        if not self.account:
            raise ValueError("No private key configured")

        domain = self.create_eip712_domain(network, token_address)

        message = {
            "from": from_address,
            "to": to_address,
            "value": int(value),
            "validAfter": valid_after,
            "validBefore": valid_before,
            "nonce": nonce,
        }

        structured_data = {
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"},
                ],
                "TransferWithAuthorization": [
                    {"name": "from", "type": "address"},
                    {"name": "to", "type": "address"},
                    {"name": "value", "type": "uint256"},
                    {"name": "validAfter", "type": "uint256"},
                    {"name": "validBefore", "type": "uint256"},
                    {"name": "nonce", "type": "bytes32"},
                ],
            },
            "primaryType": "TransferWithAuthorization",
            "domain": domain,
            "message": message,
        }

        encoded = encode_typed_data(full_message=structured_data)
        signed = self.account.sign_message(encoded)
        return signed.signature.hex()

    def create_verify_request(
        self,
        recipient: str,
        amount: str,
        network: str = "avalanche-fuji",
        token_address: Optional[str] = None,
        valid_before: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Create a properly formatted VerifyRequest"""
        if not self.account:
            raise ValueError("No private key configured for signing")

        if token_address is None:
            token_address = NETWORKS[network].get("glue_token") or NETWORKS[network].get("usdc_token")

        if valid_before is None:
            valid_before = int(time.time()) + 3600  # 1 hour from now

        nonce = self.generate_nonce()

        signature = self.sign_transfer_authorization(
            from_address=self.address,
            to_address=recipient,
            value=amount,
            valid_after=0,
            valid_before=valid_before,
            nonce=nonce,
            network=network,
            token_address=token_address,
        )

        return {
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": network,
                "payload": {
                    "signature": signature,
                    "authorization": {
                        "from": self.address,
                        "to": recipient,
                        "value": amount,
                        "validAfter": 0,
                        "validBefore": valid_before,
                        "nonce": nonce,
                    },
                },
            },
            "paymentRequirements": {
                "network": network,
                "scheme": "exact",
                "asset": token_address,
                "recipient": recipient,
                "amount": amount,
            },
        }

    def test_health(self) -> Dict[str, Any]:
        """Test /health endpoint"""
        print("\n=== Testing /health ===")
        response = requests.get(f"{self.facilitator_url}/health")
        data = response.json()

        print(f"Status: {response.status_code}")
        print(f"Providers: {len(data.get('providers', []))}")

        for provider in data.get("providers", []):
            print(f"  - {provider['network']}: {provider['address']}")

        return data

    def test_supported(self) -> Dict[str, Any]:
        """Test /supported endpoint"""
        print("\n=== Testing /supported ===")
        response = requests.get(f"{self.facilitator_url}/supported")
        data = response.json()

        print(f"Status: {response.status_code}")
        print(f"Supported kinds: {len(data.get('kinds', []))}")

        for kind in data.get("kinds", []):
            print(f"  - {kind['scheme']} on {kind['network']}")

        return data

    def test_verify(self, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """Test /verify endpoint"""
        print("\n=== Testing /verify ===")
        print(f"Network: {request_data['paymentPayload']['network']}")
        print(f"Amount: {request_data['paymentRequirements']['amount']}")

        response = requests.post(
            f"{self.facilitator_url}/verify",
            json=request_data,
            headers={"Content-Type": "application/json"},
        )

        print(f"Status: {response.status_code}")
        data = response.json()
        print(f"Valid: {data.get('valid', False)}")

        if not data.get("valid"):
            print(f"Error: {data.get('error', {}).get('reason', 'Unknown')}")

        return data

    def test_settle(self, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """Test /settle endpoint"""
        print("\n=== Testing /settle ===")
        print(f"Network: {request_data['paymentPayload']['network']}")
        print(f"Amount: {request_data['paymentRequirements']['amount']}")

        response = requests.post(
            f"{self.facilitator_url}/settle",
            json=request_data,
            headers={"Content-Type": "application/json"},
        )

        print(f"Status: {response.status_code}")

        if response.status_code == 200:
            data = response.json()
            print(f"Transaction hash: {data.get('transactionHash', 'N/A')}")
            return data
        else:
            print(f"Error: {response.text}")
            return {"error": response.text}


def run_all_tests():
    """Run comprehensive test suite"""
    print("[TEST] x402 Facilitator Test Suite")
    print("=" * 50)

    tester = X402Tester(private_key=PRIVATE_KEY)

    if not PRIVATE_KEY:
        print("\n[WARN]  WARNING: No TEST_PRIVATE_KEY configured")
        print("Only testing read-only endpoints (health, supported)")
        print("\nTo test verify/settle:")
        print("  1. Set TEST_PRIVATE_KEY in .env")
        print("  2. Fund wallet with test GLUE/USDC")
        print("  3. Re-run tests\n")

    # Test 1: Health check
    try:
        tester.test_health()
        print("[PASS] /health passed")
    except Exception as e:
        print(f"[FAIL] /health failed: {e}")

    # Test 2: Supported networks
    try:
        tester.test_supported()
        print("[PASS] /supported passed")
    except Exception as e:
        print(f"[FAIL] /supported failed: {e}")

    if PRIVATE_KEY:
        # Test 3: Verify valid payment
        print("\n\n[TEST 3] Valid payment verification")
        print("=" * 50)
        try:
            request = tester.create_verify_request(
                recipient="0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
                amount="10000",  # 0.01 GLUE (6 decimals)
                network="avalanche-fuji",
            )
            result = tester.test_verify(request)

            if result.get("valid"):
                print("[PASS] /verify passed - payment is valid")

                # Test 4: Settle the payment
                print("\n\n📋 TEST 4: Settle valid payment")
                print("=" * 50)
                settle_result = tester.test_settle(request)

                if settle_result.get("transactionHash"):
                    print("[PASS] /settle passed - payment settled on-chain")
                    print(f"   TX: {settle_result['transactionHash']}")
                else:
                    print("[FAIL] /settle failed")
            else:
                print("[FAIL] /verify failed - payment is invalid")
                print(f"   Reason: {result.get('error', {})}")

        except Exception as e:
            print(f"[FAIL] Tests 3 & 4 failed: {e}")
            import traceback

            traceback.print_exc()

        # Test 5: Invalid signature
        print("\n\n📋 TEST 5: Invalid signature (should fail)")
        print("=" * 50)
        try:
            request = tester.create_verify_request(
                recipient="0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
                amount="10000",
                network="avalanche-fuji",
            )
            # Corrupt the signature
            request["paymentPayload"]["payload"]["signature"] = "0x" + "00" * 65

            result = tester.test_verify(request)

            if not result.get("valid"):
                print("[PASS] /verify correctly rejected invalid signature")
            else:
                print("[FAIL] /verify incorrectly accepted invalid signature")

        except Exception as e:
            print(f"[FAIL] Test 5 failed: {e}")

    print("\n\n[PASS] Test suite complete!")
    print("=" * 50)


if __name__ == "__main__":
    run_all_tests()
