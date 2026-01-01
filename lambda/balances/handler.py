"""
Lambda function to fetch wallet balances for all networks.
Uses concurrent requests and caches results for 60 seconds.

This Lambda is called via API Gateway and returns balances in JSON format.
Private RPC URLs (with API keys) are stored in AWS Secrets Manager.
Falls back to public RPCs if private ones fail.
"""

import json
import os
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import Any
import urllib.request
import urllib.error

import boto3
from botocore.exceptions import ClientError

# Cache for balances (TTL: 60 seconds)
_cache: dict[str, Any] = {}
_cache_timestamp: float = 0
CACHE_TTL_SECONDS = 60

# Cache for secrets (loaded once per Lambda cold start)
_secrets_cache: dict[str, str] = {}

# Mainnet secret name
MAINNET_SECRET_NAME = "facilitator-rpc-mainnet"


def get_secret(secret_name: str, key: str | None = None) -> str | None:
    """
    Retrieve a secret from AWS Secrets Manager.
    Results are cached for the lifetime of the Lambda container.
    """
    cache_key = f"{secret_name}:{key}" if key else secret_name

    if cache_key in _secrets_cache:
        return _secrets_cache[cache_key]

    try:
        client = boto3.client("secretsmanager")
        response = client.get_secret_value(SecretId=secret_name)
        secret_string = response.get("SecretString", "")

        if key:
            # Parse JSON and extract specific key
            secret_data = json.loads(secret_string)
            value = secret_data.get(key)
        else:
            value = secret_string

        if value:
            _secrets_cache[cache_key] = value
        return value
    except ClientError as e:
        print(f"Error retrieving secret {secret_name}: {e}")
        return None
    except json.JSONDecodeError as e:
        print(f"Error parsing secret JSON {secret_name}: {e}")
        return None


def get_private_rpc(network_key: str) -> str | None:
    """Get private RPC URL from Secrets Manager."""
    return get_secret(MAINNET_SECRET_NAME, network_key)


# Wallet addresses
MAINNET_ADDRESS = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
TESTNET_ADDRESS = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"

# Solana addresses
SOLANA_MAINNET_ADDRESS = "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq"
SOLANA_TESTNET_ADDRESS = "6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv"

# Sui addresses
SUI_MAINNET_ADDRESS = "0xe7bbf2b13f7d72714760aa16e024fa1b35a978793f9893d0568a4fbf356a764a"
SUI_TESTNET_ADDRESS = "0xabbd16a2fab2a502c9cfe835195a6fc7d70bfc27cffb40b8b286b52a97006e67"

# NEAR addresses
NEAR_MAINNET_ADDRESS = "uvd-facilitator.near"
NEAR_TESTNET_ADDRESS = "uvd-facilitator.testnet"

# Stellar addresses
STELLAR_MAINNET_ADDRESS = "GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB"
STELLAR_TESTNET_ADDRESS = "GBBFZMLUJEZVI32EN4XA2KPP445XIBTMTRBLYWFIL556RDTHS2OWFQ2Z"

# Algorand addresses
ALGORAND_MAINNET_ADDRESS = "KIMS5H6QLCUDL65L5UBTOXDPWLMTS7N3AAC3I6B2NCONEI5QIVK7LH2C2I"
ALGORAND_TESTNET_ADDRESS = "5DPPDQNYUPCTXRZWRYSF3WPYU6RKAUR25F3YG4EKXQRHV5AUAI62H5GXL4"


# Public RPC fallbacks (no API keys)
PUBLIC_RPCS = {
    "base": "https://mainnet.base.org",
    "avalanche": "https://avalanche-c-chain-rpc.publicnode.com",
    "polygon": "https://polygon.drpc.org",
    "optimism": "https://mainnet.optimism.io",
    "celo": "https://rpc.celocolombia.org",
    "hyperevm": "https://rpc.hyperliquid.xyz/evm",
    "ethereum": "https://ethereum-rpc.publicnode.com",
    "arbitrum": "https://arb1.arbitrum.io/rpc",
    "unichain": "https://unichain-rpc.publicnode.com",
    "solana": "https://api.mainnet-beta.solana.com",
    "near": "https://free.rpc.fastnear.com",
}


def get_network_configs() -> dict[str, dict]:
    """
    Build network configurations.
    Private RPC URLs (with API keys) are loaded from Secrets Manager first,
    with fallback to environment variables and then public RPCs.
    """
    # Load private RPCs from Secrets Manager for mainnets
    private_rpcs = {}
    for network_key in ["base", "avalanche", "polygon", "optimism", "celo",
                        "hyperevm", "ethereum", "arbitrum", "unichain", "solana", "near"]:
        private_rpc = get_private_rpc(network_key)
        if private_rpc:
            private_rpcs[network_key] = private_rpc
            print(f"Loaded private RPC for {network_key}")
        else:
            print(f"No private RPC for {network_key}, will use public")

    return {
        # EVM Mainnets - with private RPC priority
        "avalanche-mainnet": {
            "rpcs": [
                private_rpcs.get("avalanche"),
                os.environ.get("RPC_URL_AVALANCHE"),
                PUBLIC_RPCS["avalanche"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "base-mainnet": {
            "rpcs": [
                private_rpcs.get("base"),
                os.environ.get("RPC_URL_BASE"),
                PUBLIC_RPCS["base"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "celo-mainnet": {
            "rpcs": [
                private_rpcs.get("celo"),
                os.environ.get("RPC_URL_CELO"),
                PUBLIC_RPCS["celo"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "hyperevm-mainnet": {
            "rpcs": [
                private_rpcs.get("hyperevm"),
                os.environ.get("RPC_URL_HYPEREVM"),
                PUBLIC_RPCS["hyperevm"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "polygon-mainnet": {
            "rpcs": [
                private_rpcs.get("polygon"),
                os.environ.get("RPC_URL_POLYGON"),
                PUBLIC_RPCS["polygon"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "optimism-mainnet": {
            "rpcs": [
                private_rpcs.get("optimism"),
                os.environ.get("RPC_URL_OPTIMISM"),
                PUBLIC_RPCS["optimism"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "ethereum-mainnet": {
            "rpcs": [
                private_rpcs.get("ethereum"),
                os.environ.get("RPC_URL_ETHEREUM"),
                PUBLIC_RPCS["ethereum"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "arbitrum-mainnet": {
            "rpcs": [
                private_rpcs.get("arbitrum"),
                os.environ.get("RPC_URL_ARBITRUM"),
                PUBLIC_RPCS["arbitrum"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "unichain-mainnet": {
            "rpcs": [
                private_rpcs.get("unichain"),
                os.environ.get("RPC_URL_UNICHAIN"),
                PUBLIC_RPCS["unichain"],
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "monad-mainnet": {
            "rpcs": [
                os.environ.get("RPC_URL_MONAD"),
                "https://rpc.monad.xyz",
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "bsc-mainnet": {
            "rpcs": [
                os.environ.get("RPC_URL_BSC"),
                "https://bsc-dataseed.binance.org/",
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        # EVM Testnets - public RPCs only
        "avalanche-testnet": {
            "rpcs": ["https://avalanche-fuji-c-chain-rpc.publicnode.com"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "base-testnet": {
            "rpcs": ["https://sepolia.base.org"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "celo-testnet": {
            "rpcs": ["https://rpc.ankr.com/celo_sepolia"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "polygon-testnet": {
            "rpcs": ["https://rpc-amoy.polygon.technology"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "optimism-testnet": {
            "rpcs": ["https://sepolia.optimism.io"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "ethereum-testnet": {
            "rpcs": ["https://ethereum-sepolia-rpc.publicnode.com"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "arbitrum-testnet": {
            "rpcs": ["https://arbitrum-sepolia-rpc.publicnode.com"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "unichain-testnet": {
            "rpcs": ["https://unichain-sepolia.drpc.org"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        "hyperevm-testnet": {
            "rpcs": ["https://rpc.hyperliquid-testnet.xyz/evm"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        # Solana - with private RPC priority
        "solana-mainnet": {
            "rpcs": [
                private_rpcs.get("solana"),
                os.environ.get("RPC_URL_SOLANA"),
                PUBLIC_RPCS["solana"],
            ],
            "address": SOLANA_MAINNET_ADDRESS,
            "type": "solana"
        },
        "solana-devnet": {
            "rpcs": ["https://api.devnet.solana.com"],
            "address": SOLANA_TESTNET_ADDRESS,
            "type": "solana"
        },
        # Fogo (Solana-based)
        "fogo-mainnet": {
            "rpcs": ["https://rpc.fogo.nightly.app/"],
            "address": SOLANA_MAINNET_ADDRESS,
            "type": "solana"
        },
        "fogo-testnet": {
            "rpcs": ["https://testnet.fogo.io/"],
            "address": SOLANA_TESTNET_ADDRESS,
            "type": "solana"
        },
        # Sui
        "sui-mainnet": {
            "rpcs": [
                os.environ.get("RPC_URL_SUI"),
                "https://fullnode.mainnet.sui.io:443",
            ],
            "address": SUI_MAINNET_ADDRESS,
            "type": "sui"
        },
        "sui-testnet": {
            "rpcs": ["https://fullnode.testnet.sui.io:443"],
            "address": SUI_TESTNET_ADDRESS,
            "type": "sui"
        },
        # NEAR - with private RPC priority
        "near-mainnet": {
            "rpcs": [
                private_rpcs.get("near"),
                "https://free.rpc.fastnear.com",
                "https://near.lava.build",
                "https://near.drpc.org",
            ],
            "address": NEAR_MAINNET_ADDRESS,
            "type": "near"
        },
        "near-testnet": {
            "rpcs": [
                "https://test.rpc.fastnear.com",
                "https://rpc.testnet.fastnear.com",
                "https://near-testnet.drpc.org",
            ],
            "address": NEAR_TESTNET_ADDRESS,
            "type": "near"
        },
        # Stellar
        "stellar-mainnet": {
            "api": f"https://horizon.stellar.org/accounts/{STELLAR_MAINNET_ADDRESS}",
            "address": STELLAR_MAINNET_ADDRESS,
            "type": "stellar"
        },
        "stellar-testnet": {
            "api": f"https://horizon-testnet.stellar.org/accounts/{STELLAR_TESTNET_ADDRESS}",
            "address": STELLAR_TESTNET_ADDRESS,
            "type": "stellar"
        },
        # Algorand
        "algorand-mainnet": {
            "api": f"https://mainnet-api.algonode.cloud/v2/accounts/{ALGORAND_MAINNET_ADDRESS}",
            "address": ALGORAND_MAINNET_ADDRESS,
            "type": "algorand"
        },
        "algorand-testnet": {
            "api": f"https://testnet-api.algonode.cloud/v2/accounts/{ALGORAND_TESTNET_ADDRESS}",
            "address": ALGORAND_TESTNET_ADDRESS,
            "type": "algorand"
        },
    }


def fetch_json(url: str, data: bytes | None = None, timeout: float = 10) -> dict:
    """Make an HTTP request and return JSON response."""
    headers = {"Content-Type": "application/json"}
    req = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return json.loads(response.read().decode())


def fetch_evm_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for an EVM network with RPC fallback."""
    rpcs = [r for r in config.get("rpcs", []) if r]  # Filter out None values

    for rpc_url in rpcs:
        try:
            payload = json.dumps({
                "jsonrpc": "2.0",
                "method": "eth_getBalance",
                "params": [config["address"], "latest"],
                "id": 1
            }).encode()

            data = fetch_json(rpc_url, payload, timeout=8)
            if "result" in data:
                balance_wei = int(data["result"], 16)
                balance_eth = balance_wei / 1e18
                return network, f"{balance_eth:.4f}"
        except Exception as e:
            print(f"Error fetching {network} from {rpc_url[:50]}...: {e}")
            continue

    return network, None


def fetch_solana_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for Solana network with RPC fallback."""
    rpcs = [r for r in config.get("rpcs", []) if r]  # Filter out None values

    for rpc_url in rpcs:
        try:
            payload = json.dumps({
                "jsonrpc": "2.0",
                "method": "getBalance",
                "params": [config["address"]],
                "id": 1
            }).encode()

            data = fetch_json(rpc_url, payload, timeout=8)
            if "result" in data and "value" in data["result"]:
                balance_lamports = data["result"]["value"]
                balance_sol = balance_lamports / 1e9
                return network, f"{balance_sol:.4f}"
        except Exception as e:
            print(f"Error fetching {network} from {rpc_url[:50]}...: {e}")
            continue

    return network, None


def fetch_sui_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for Sui network with RPC fallback."""
    rpcs = [r for r in config.get("rpcs", []) if r]  # Filter out None values

    for rpc_url in rpcs:
        try:
            payload = json.dumps({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_getAllBalances",
                "params": [config["address"]]
            }).encode()

            data = fetch_json(rpc_url, payload, timeout=8)
            if "result" in data and isinstance(data["result"], list):
                sui_balance = next(
                    (b for b in data["result"] if b.get("coinType") == "0x2::sui::SUI"),
                    None
                )
                if sui_balance:
                    balance_mist = int(sui_balance["totalBalance"])
                    balance_sui = balance_mist / 1e9
                    return network, f"{balance_sui:.4f}"
        except Exception as e:
            print(f"Error fetching {network} from {rpc_url[:50]}...: {e}")
            continue

    return network, None


def fetch_near_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for NEAR network with RPC rotation."""
    rpcs = [r for r in config.get("rpcs", []) if r]  # Filter out None values

    for rpc_url in rpcs:
        try:
            payload = json.dumps({
                "jsonrpc": "2.0",
                "id": "balance",
                "method": "query",
                "params": {
                    "request_type": "view_account",
                    "finality": "final",
                    "account_id": config["address"]
                }
            }).encode()

            data = fetch_json(rpc_url, payload, timeout=5)
            if "result" in data and "amount" in data["result"]:
                balance_yocto = int(data["result"]["amount"])
                balance_near = balance_yocto / 1e24
                return network, f"{balance_near:.4f}"
        except Exception:
            continue
    return network, None


def fetch_stellar_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for Stellar network."""
    try:
        data = fetch_json(config["api"], timeout=8)
        native_balance = next(
            (b for b in data.get("balances", []) if b.get("asset_type") == "native"),
            None
        )
        if native_balance:
            balance_xlm = float(native_balance["balance"])
            return network, f"{balance_xlm:.4f}"
    except Exception as e:
        print(f"Error fetching {network}: {e}")
    return network, None


def fetch_algorand_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for Algorand network."""
    try:
        data = fetch_json(config["api"], timeout=8)
        if "amount" in data:
            balance_algo = data["amount"] / 1e6
            return network, f"{balance_algo:.4f}"
    except Exception as e:
        print(f"Error fetching {network}: {e}")
    return network, None


def fetch_all_balances() -> dict[str, str | None]:
    """Fetch balances for all networks concurrently."""
    global _cache, _cache_timestamp

    # Check cache
    now = time.time()
    if _cache and (now - _cache_timestamp) < CACHE_TTL_SECONDS:
        return _cache

    networks = get_network_configs()
    balances = {}

    # Use ThreadPoolExecutor for concurrent requests
    with ThreadPoolExecutor(max_workers=20) as executor:
        futures = []

        for network, config in networks.items():
            network_type = config.get("type", "evm")

            if network_type == "evm":
                futures.append(executor.submit(fetch_evm_balance, network, config))
            elif network_type == "solana":
                futures.append(executor.submit(fetch_solana_balance, network, config))
            elif network_type == "sui":
                futures.append(executor.submit(fetch_sui_balance, network, config))
            elif network_type == "near":
                futures.append(executor.submit(fetch_near_balance, network, config))
            elif network_type == "stellar":
                futures.append(executor.submit(fetch_stellar_balance, network, config))
            elif network_type == "algorand":
                futures.append(executor.submit(fetch_algorand_balance, network, config))

        for future in as_completed(futures):
            network, balance = future.result()
            balances[network] = balance

    # Update cache
    _cache = balances
    _cache_timestamp = now

    return balances


def lambda_handler(event: dict, context: Any) -> dict:
    """
    AWS Lambda handler.
    Returns all wallet balances as JSON.
    """
    try:
        balances = fetch_all_balances()

        return {
            "statusCode": 200,
            "headers": {
                "Content-Type": "application/json",
                "Access-Control-Allow-Origin": "*",
                "Access-Control-Allow-Methods": "GET, OPTIONS",
                "Access-Control-Allow-Headers": "Content-Type",
                "Cache-Control": "public, max-age=60"
            },
            "body": json.dumps({
                "balances": balances,
                "cached_at": int(_cache_timestamp),
                "ttl_seconds": CACHE_TTL_SECONDS
            })
        }
    except Exception as e:
        print(f"Error in lambda_handler: {e}")
        return {
            "statusCode": 500,
            "headers": {
                "Content-Type": "application/json",
                "Access-Control-Allow-Origin": "*"
            },
            "body": json.dumps({"error": str(e)})
        }
