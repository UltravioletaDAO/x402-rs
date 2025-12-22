#!/usr/bin/env python3
"""
EIP-3009 Verification Script

Verifies if a token contract supports EIP-3009 transferWithAuthorization,
which is required for x402 protocol integration.

Usage:
    python3 verify_eip3009.py --contract 0xCONTRACT_ADDRESS --rpc https://rpc-url

Examples:
    # Verify USDT0 on Arbitrum
    python3 verify_eip3009.py --contract 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 --rpc https://arb1.arbitrum.io/rpc

    # Verify USDC on Base
    python3 verify_eip3009.py --contract 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 --rpc https://mainnet.base.org
"""

import argparse
import json
import sys
from urllib.request import Request, urlopen
from urllib.error import URLError, HTTPError


def eth_call(rpc_url: str, to: str, data: str) -> dict:
    """Make an eth_call to the RPC endpoint."""
    payload = {
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{"to": to, "data": data}, "latest"],
        "id": 1
    }

    req = Request(
        rpc_url,
        data=json.dumps(payload).encode('utf-8'),
        headers={"Content-Type": "application/json"}
    )

    try:
        with urlopen(req, timeout=30) as response:
            return json.loads(response.read().decode('utf-8'))
    except (URLError, HTTPError) as e:
        return {"error": {"message": str(e)}}


def verify_eip3009(contract: str, rpc_url: str) -> dict:
    """
    Verify if contract supports EIP-3009 transferWithAuthorization.

    Returns a dict with:
        - supported: bool
        - reason: str
        - details: dict (name, decimals, etc.)
    """
    result = {
        "supported": False,
        "reason": "",
        "details": {}
    }

    # Function signature for transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)
    # Selector: 0xe3ee160e
    # We call with dummy parameters to check if function exists

    # Parameters (all zeros except minimal valid data):
    # from: 0x0000000000000000000000000000000000000001
    # to: 0x0000000000000000000000000000000000000002
    # value: 1000000 (0xf4240)
    # validAfter: 0
    # validBefore: 9999999999 (0x2540be3ff)
    # nonce: 0x00..00
    # signature: 65 bytes of zeros (0x00..00)

    calldata = (
        "0xe3ee160e"  # function selector
        "0000000000000000000000000000000000000000000000000000000000000001"  # from
        "0000000000000000000000000000000000000000000000000000000000000002"  # to
        "00000000000000000000000000000000000000000000000000000000000f4240"  # value
        "0000000000000000000000000000000000000000000000000000000000000000"  # validAfter
        "00000000000000000000000000000000000000000000000002540be3ff000000"  # validBefore
        "0000000000000000000000000000000000000000000000000000000000000000"  # nonce
        "00000000000000000000000000000000000000000000000000000000000000e0"  # offset to signature
        "0000000000000000000000000000000000000000000000000000000000000041"  # signature length (65)
        "0000000000000000000000000000000000000000000000000000000000000000"  # r
        "0000000000000000000000000000000000000000000000000000000000000000"  # s
        "0000000000000000000000000000000000000000000000000000000000000000"  # v (padded)
    )

    response = eth_call(rpc_url, contract, calldata)

    if "error" in response:
        error_msg = response["error"].get("message", "Unknown error")

        # Check for signature-related errors (means function EXISTS)
        signature_errors = [
            "invalid signature",
            "ECRecover",
            "SignatureChecker",
            "ECDSA",
            "FiatToken: invalid",
            "signature verification",
        ]

        for sig_err in signature_errors:
            if sig_err.lower() in error_msg.lower():
                result["supported"] = True
                result["reason"] = f"EIP-3009 supported (signature validation error: {error_msg})"
                break

        if not result["supported"]:
            # Check for function not found errors
            not_found_errors = [
                "execution reverted",
                "function selector",
                "not recognized",
                "invalid opcode",
            ]

            for not_found in not_found_errors:
                if not_found.lower() in error_msg.lower():
                    result["reason"] = f"EIP-3009 NOT supported (function not found: {error_msg})"
                    break

            if not result["reason"]:
                result["reason"] = f"Unknown error: {error_msg}"
    else:
        # Unexpected success (shouldn't happen with dummy params)
        result["reason"] = "Unexpected success response"

    return result


def get_token_info(contract: str, rpc_url: str) -> dict:
    """Get token name, decimals, and version."""
    info = {}

    # Get name
    # name() selector: 0x06fdde03
    response = eth_call(rpc_url, contract, "0x06fdde03")
    if "result" in response and response["result"]:
        try:
            # Decode string from ABI encoding
            hex_data = response["result"][2:]  # Remove 0x
            if len(hex_data) >= 128:  # Offset + length + data
                offset = int(hex_data[0:64], 16) * 2
                length = int(hex_data[64:128], 16)
                name_hex = hex_data[128:128 + length * 2]
                info["name"] = bytes.fromhex(name_hex).decode('utf-8', errors='replace')
        except Exception:
            info["name"] = "Unable to decode"

    # Get decimals
    # decimals() selector: 0x313ce567
    response = eth_call(rpc_url, contract, "0x313ce567")
    if "result" in response and response["result"]:
        try:
            info["decimals"] = int(response["result"], 16)
        except Exception:
            info["decimals"] = "Unable to decode"

    # Get version (may not exist on all contracts)
    # version() selector: 0x54fd4d50
    response = eth_call(rpc_url, contract, "0x54fd4d50")
    if "result" in response and response["result"] and len(response["result"]) > 2:
        try:
            hex_data = response["result"][2:]
            if len(hex_data) >= 128:
                offset = int(hex_data[0:64], 16) * 2
                length = int(hex_data[64:128], 16)
                version_hex = hex_data[128:128 + length * 2]
                info["version"] = bytes.fromhex(version_hex).decode('utf-8', errors='replace')
        except Exception:
            info["version"] = "Not exposed"
    else:
        info["version"] = "Not exposed"

    # Get DOMAIN_SEPARATOR
    # DOMAIN_SEPARATOR() selector: 0x3644e515
    response = eth_call(rpc_url, contract, "0x3644e515")
    if "result" in response and response["result"] and len(response["result"]) > 2:
        info["domain_separator"] = response["result"]
    else:
        info["domain_separator"] = "Not found"

    return info


def main():
    parser = argparse.ArgumentParser(
        description="Verify EIP-3009 support for a token contract",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Verify USDT0 on Arbitrum (should pass)
  python3 verify_eip3009.py --contract 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 --rpc https://arb1.arbitrum.io/rpc

  # Verify legacy USDT on Ethereum (should fail)
  python3 verify_eip3009.py --contract 0xdAC17F958D2ee523a2206206994597C13D831ec7 --rpc https://eth.llamarpc.com

  # Verify USDC on Base (should pass)
  python3 verify_eip3009.py --contract 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 --rpc https://mainnet.base.org
        """
    )
    parser.add_argument("--contract", required=True, help="Token contract address")
    parser.add_argument("--rpc", required=True, help="RPC endpoint URL")
    parser.add_argument("--json", action="store_true", help="Output as JSON")

    args = parser.parse_args()

    # Normalize contract address
    contract = args.contract.lower()
    if not contract.startswith("0x"):
        contract = "0x" + contract

    print(f"Verifying EIP-3009 support for {contract}")
    print(f"RPC: {args.rpc}")
    print("-" * 60)

    # Get token info first
    info = get_token_info(contract, args.rpc)

    if not args.json:
        print(f"Token Name: {info.get('name', 'Unknown')}")
        print(f"Decimals: {info.get('decimals', 'Unknown')}")
        print(f"Version: {info.get('version', 'Unknown')}")
        print(f"Domain Separator: {info.get('domain_separator', 'Unknown')[:20]}..." if info.get('domain_separator') else "Domain Separator: None")
        print("-" * 60)

    # Verify EIP-3009 support
    result = verify_eip3009(contract, args.rpc)
    result["details"] = info

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        if result["supported"]:
            print("[PASS] EIP-3009 SUPPORTED")
            print(f"Reason: {result['reason']}")
            print("")
            print("This token can be integrated with x402!")
            print("")
            print("Next steps:")
            print("1. Verify EIP-712 name matches the token name above")
            print("2. Determine EIP-712 version (try '1' or '2')")
            print("3. Follow the stablecoin-addition skill workflow")
        else:
            print("[FAIL] EIP-3009 NOT SUPPORTED")
            print(f"Reason: {result['reason']}")
            print("")
            print("This token CANNOT be integrated with x402.")
            print("It likely only implements EIP-2612 (permit).")

    # Exit with appropriate code
    sys.exit(0 if result["supported"] else 1)


if __name__ == "__main__":
    main()
