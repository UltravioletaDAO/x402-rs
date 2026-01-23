#!/usr/bin/env python3
"""
Network Research Script for x402-rs Facilitator

Usage:
    python research_network.py --network scroll
    python research_network.py --network scroll --mainnet-rpc https://rpc.scroll.io
"""

import argparse
import json
import subprocess
import sys
import os
from pathlib import Path

# Facilitator wallet addresses
MAINNET_WALLET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
TESTNET_WALLET = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"

# Common USDC addresses by network (from Circle's official deployments)
KNOWN_USDC = {
    "scroll": {
        "mainnet": "0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4",
        "testnet": None,  # Check Circle docs
    },
    "linea": {
        "mainnet": "0x176211869cA2b568f2A7D4EE941E073a821EE1ff",  # Bridged
        "testnet": None,
    },
    "blast": {
        "mainnet": "0x4300000000000000000000000000000000000003",  # USDB (native)
        "testnet": None,
    },
    "zksync": {
        "mainnet": "0x1d17CBcF0D6D143135aE902365D2E5e2A16538D4",
        "testnet": "0x0faF6df7054946141266420b43783387A78d82A9",
    },
    "mantle": {
        "mainnet": "0x09Bc4E0D864854c6aFB6eB9A9cdF58aC190D0dF9",
        "testnet": None,
    },
    "mode": {
        "mainnet": "0xd988097fb8612cc24eeC14542bC03424c656005f",
        "testnet": None,
    },
    "taiko": {
        "mainnet": "0x07d83526730c7438048D55A4fc0b850e2aaB6f0b",
        "testnet": None,
    },
}

# Chain IDs from chainlist.org
KNOWN_CHAIN_IDS = {
    "scroll": {"mainnet": 534352, "testnet": 534351},
    "linea": {"mainnet": 59144, "testnet": 59141},
    "blast": {"mainnet": 81457, "testnet": 168587773},
    "zksync": {"mainnet": 324, "testnet": 300},
    "mantle": {"mainnet": 5000, "testnet": 5003},
    "mode": {"mainnet": 34443, "testnet": 919},
    "taiko": {"mainnet": 167000, "testnet": 167009},
    "abstract": {"mainnet": 2741, "testnet": 11124},
    "ink": {"mainnet": 57073, "testnet": 763373},
    "worldchain": {"mainnet": 480, "testnet": 4801},
    "soneium": {"mainnet": 1868, "testnet": 1946},
}

# Public RPCs
KNOWN_RPCS = {
    "scroll": {
        "mainnet": "https://rpc.scroll.io",
        "testnet": "https://sepolia-rpc.scroll.io",
    },
    "linea": {
        "mainnet": "https://rpc.linea.build",
        "testnet": "https://rpc.sepolia.linea.build",
    },
    "blast": {
        "mainnet": "https://rpc.blast.io",
        "testnet": "https://sepolia.blast.io",
    },
    "zksync": {
        "mainnet": "https://mainnet.era.zksync.io",
        "testnet": "https://sepolia.era.zksync.dev",
    },
    "mantle": {
        "mainnet": "https://rpc.mantle.xyz",
        "testnet": "https://rpc.sepolia.mantle.xyz",
    },
    "mode": {
        "mainnet": "https://mainnet.mode.network",
        "testnet": "https://sepolia.mode.network",
    },
    "taiko": {
        "mainnet": "https://rpc.mainnet.taiko.xyz",
        "testnet": "https://rpc.hekla.taiko.xyz",
    },
}

# Block explorers
KNOWN_EXPLORERS = {
    "scroll": {
        "mainnet": "https://scrollscan.com",
        "testnet": "https://sepolia.scrollscan.com",
    },
    "linea": {
        "mainnet": "https://lineascan.build",
        "testnet": "https://sepolia.lineascan.build",
    },
    "blast": {
        "mainnet": "https://blastscan.io",
        "testnet": "https://sepolia.blastscan.io",
    },
    "zksync": {
        "mainnet": "https://explorer.zksync.io",
        "testnet": "https://sepolia.explorer.zksync.io",
    },
}

# Brand colors (hex)
KNOWN_COLORS = {
    "scroll": "#FFEEDA",
    "linea": "#61DFFF",
    "blast": "#FCFC03",
    "zksync": "#8C8DFC",
    "mantle": "#000000",
    "mode": "#DFFE00",
    "taiko": "#E81899",
}


def get_balance(rpc_url: str, address: str) -> float:
    """Get native token balance via JSON-RPC."""
    try:
        payload = {
            "jsonrpc": "2.0",
            "method": "eth_getBalance",
            "params": [address, "latest"],
            "id": 1
        }
        result = subprocess.run(
            ["curl", "-s", "-X", "POST", rpc_url,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(payload)],
            capture_output=True, text=True, timeout=10
        )
        data = json.loads(result.stdout)
        if "result" in data:
            return int(data["result"], 16) / 1e18
    except Exception as e:
        print(f"  Error getting balance: {e}")
    return 0.0


def verify_eip3009(rpc_url: str, contract: str) -> bool:
    """Verify if contract supports EIP-3009 transferWithAuthorization."""
    try:
        payload = {
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{
                "to": contract,
                "data": "0xe3ee160e"  # transferWithAuthorization selector
                        "0000000000000000000000000000000000000000000000000000000000000001"
                        "0000000000000000000000000000000000000000000000000000000000000002"
                        "00000000000000000000000000000000000000000000000000000000000f4240"
                        "0000000000000000000000000000000000000000000000000000000000000000"
                        "000000000000000000000000000000000000000000000000000000ffffffffff"
                        "0000000000000000000000000000000000000000000000000000000000000000"
                        "00000000000000000000000000000000000000000000000000000000000000e0"
                        "0000000000000000000000000000000000000000000000000000000000000041"
                        "0000000000000000000000000000000000000000000000000000000000000000"
                        "0000000000000000000000000000000000000000000000000000000000000000"
                        "0000000000000000000000000000000000000000000000000000000000000000"
            }, "latest"],
            "id": 1
        }
        result = subprocess.run(
            ["curl", "-s", "-X", "POST", rpc_url,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(payload)],
            capture_output=True, text=True, timeout=10
        )
        data = json.loads(result.stdout)
        error = data.get("error", {}).get("message", "")
        # If we get a signature error, the function EXISTS
        if "signature" in error.lower() or "invalid" in error.lower():
            return True
        # If we get generic revert, function doesn't exist
        return False
    except Exception as e:
        print(f"  Error verifying EIP-3009: {e}")
    return False


def get_token_metadata(rpc_url: str, contract: str) -> dict:
    """Get EIP-712 metadata from token contract."""
    metadata = {"name": None, "version": None, "decimals": None}

    # Get name
    try:
        payload = {
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{"to": contract, "data": "0x06fdde03"}, "latest"],  # name()
            "id": 1
        }
        result = subprocess.run(
            ["curl", "-s", "-X", "POST", rpc_url,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(payload)],
            capture_output=True, text=True, timeout=10
        )
        data = json.loads(result.stdout)
        if "result" in data and len(data["result"]) > 2:
            # Decode string from ABI encoding
            hex_data = data["result"][2:]  # Remove 0x
            if len(hex_data) >= 128:
                # Skip offset (32 bytes) and length (32 bytes)
                length = int(hex_data[64:128], 16)
                name_hex = hex_data[128:128 + length * 2]
                metadata["name"] = bytes.fromhex(name_hex).decode('utf-8').strip('\x00')
    except:
        pass

    # Get version
    try:
        payload = {
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{"to": contract, "data": "0x54fd4d50"}, "latest"],  # version()
            "id": 1
        }
        result = subprocess.run(
            ["curl", "-s", "-X", "POST", rpc_url,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(payload)],
            capture_output=True, text=True, timeout=10
        )
        data = json.loads(result.stdout)
        if "result" in data and len(data["result"]) > 2:
            hex_data = data["result"][2:]
            if len(hex_data) >= 128:
                length = int(hex_data[64:128], 16)
                version_hex = hex_data[128:128 + length * 2]
                metadata["version"] = bytes.fromhex(version_hex).decode('utf-8').strip('\x00')
    except:
        pass

    # Get decimals
    try:
        payload = {
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{"to": contract, "data": "0x313ce567"}, "latest"],  # decimals()
            "id": 1
        }
        result = subprocess.run(
            ["curl", "-s", "-X", "POST", rpc_url,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(payload)],
            capture_output=True, text=True, timeout=10
        )
        data = json.loads(result.stdout)
        if "result" in data:
            metadata["decimals"] = int(data["result"], 16)
    except:
        pass

    return metadata


def check_logo_exists(network: str) -> bool:
    """Check if logo file exists in static directory."""
    # Try multiple possible locations
    paths = [
        Path(f"static/{network}.png"),
        Path(f"../static/{network}.png"),
        Path(f"/mnt/z/ultravioleta/dao/x402-rs/static/{network}.png"),
    ]
    for p in paths:
        if p.exists():
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description="Research network for x402-rs facilitator")
    parser.add_argument("--network", required=True, help="Network name (e.g., scroll, linea)")
    parser.add_argument("--mainnet-rpc", help="Override mainnet RPC URL")
    parser.add_argument("--testnet-rpc", help="Override testnet RPC URL")
    parser.add_argument("--mainnet-usdc", help="Override mainnet USDC address")
    parser.add_argument("--testnet-usdc", help="Override testnet USDC address")
    parser.add_argument("--json", action="store_true", help="Output as JSON")
    args = parser.parse_args()

    network = args.network.lower()

    # Gather data
    result = {
        "network": network,
        "chain_ids": KNOWN_CHAIN_IDS.get(network, {"mainnet": None, "testnet": None}),
        "rpcs": {
            "mainnet": args.mainnet_rpc or KNOWN_RPCS.get(network, {}).get("mainnet"),
            "testnet": args.testnet_rpc or KNOWN_RPCS.get(network, {}).get("testnet"),
        },
        "usdc": {
            "mainnet": args.mainnet_usdc or KNOWN_USDC.get(network, {}).get("mainnet"),
            "testnet": args.testnet_usdc or KNOWN_USDC.get(network, {}).get("testnet"),
        },
        "explorers": KNOWN_EXPLORERS.get(network, {"mainnet": None, "testnet": None}),
        "brand_color": KNOWN_COLORS.get(network),
        "logo_exists": check_logo_exists(network),
        "wallets": {
            "mainnet": {"address": MAINNET_WALLET, "balance": 0.0},
            "testnet": {"address": TESTNET_WALLET, "balance": 0.0},
        },
        "eip3009": {"mainnet": False, "testnet": False},
        "eip712": {"mainnet": {}, "testnet": {}},
        "prerequisites_met": False,
    }

    if not args.json:
        print(f"\n{'='*60}")
        print(f"Network Research: {network.upper()}")
        print(f"{'='*60}\n")

    # Check RPCs and balances
    for env in ["mainnet", "testnet"]:
        rpc = result["rpcs"][env]
        if rpc:
            if not args.json:
                print(f"[{env.upper()}]")
                print(f"  RPC: {rpc}")

            # Get wallet balance
            wallet = MAINNET_WALLET if env == "mainnet" else TESTNET_WALLET
            balance = get_balance(rpc, wallet)
            result["wallets"][env]["balance"] = balance
            if not args.json:
                status = "FUNDED" if balance > 0 else "NOT FUNDED"
                print(f"  Wallet: {wallet}")
                print(f"  Balance: {balance:.6f} ({status})")

            # Check USDC
            usdc = result["usdc"][env]
            if usdc:
                if not args.json:
                    print(f"  USDC: {usdc}")

                # Verify EIP-3009
                eip3009 = verify_eip3009(rpc, usdc)
                result["eip3009"][env] = eip3009
                if not args.json:
                    status = "SUPPORTED" if eip3009 else "NOT SUPPORTED"
                    print(f"  EIP-3009: {status}")

                # Get metadata
                if eip3009:
                    metadata = get_token_metadata(rpc, usdc)
                    result["eip712"][env] = metadata
                    if not args.json:
                        print(f"  EIP-712 Name: {metadata.get('name', 'Unknown')}")
                        print(f"  EIP-712 Version: {metadata.get('version', 'Unknown')}")
                        print(f"  Decimals: {metadata.get('decimals', 'Unknown')}")
            else:
                if not args.json:
                    print(f"  USDC: NOT FOUND - Need to research")

            if not args.json:
                print()

    # Check prerequisites
    logo_ok = result["logo_exists"]
    mainnet_funded = result["wallets"]["mainnet"]["balance"] > 0.001
    testnet_funded = result["wallets"]["testnet"]["balance"] > 0
    eip3009_ok = result["eip3009"]["mainnet"] or result["usdc"]["mainnet"] is None

    result["prerequisites_met"] = logo_ok and mainnet_funded and testnet_funded and eip3009_ok

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(f"{'='*60}")
        print("PREREQUISITES CHECK")
        print(f"{'='*60}")
        print(f"  Logo (static/{network}.png): {'EXISTS' if logo_ok else 'MISSING'}")
        print(f"  Mainnet wallet funded: {'YES' if mainnet_funded else 'NO'}")
        print(f"  Testnet wallet funded: {'YES' if testnet_funded else 'NO'}")
        print(f"  EIP-3009 verified: {'YES' if eip3009_ok else 'NO/UNKNOWN'}")
        print()
        if result["prerequisites_met"]:
            print("ALL PREREQUISITES MET - Ready to implement!")
        else:
            print("MISSING PREREQUISITES - See above for details")
        print()

        # Output summary for Claude
        print(f"{'='*60}")
        print("SUMMARY FOR IMPLEMENTATION")
        print(f"{'='*60}")
        print(f"Network: {network.title()}")
        print(f"Chain IDs: {result['chain_ids']['mainnet']} (mainnet), {result['chain_ids']['testnet']} (testnet)")
        if result["usdc"]["mainnet"]:
            print(f"USDC Mainnet: {result['usdc']['mainnet']}")
        if result["usdc"]["testnet"]:
            print(f"USDC Testnet: {result['usdc']['testnet']}")
        if result["eip712"]["mainnet"]:
            print(f"EIP-712: name=\"{result['eip712']['mainnet'].get('name')}\", version=\"{result['eip712']['mainnet'].get('version')}\"")
        if result["brand_color"]:
            print(f"Brand Color: {result['brand_color']}")
        print()


if __name__ == "__main__":
    main()
