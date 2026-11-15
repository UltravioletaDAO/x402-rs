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
from urllib.parse import urlsplit

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


def rpc_host(rpc_url: str) -> str:
    """The host of an RPC URL, for a log line: a private one carries its key in the
    path (QuickNode, Alchemy), so the path never reaches the log (src/redact.rs)."""
    try:
        return urlsplit(rpc_url).hostname or "<rpc>"
    except ValueError:
        return "<rpc>"


def rpc_error(error: Exception) -> str:
    """What a log line says about a failed RPC read: the exception's type, and the
    status of an HTTP error -- never its text. urllib and http.client quote the URL,
    or a piece of it, in their messages (`unknown url type: '...'`, `nonnumeric
    port: '...'`, `URL can't contain control characters. '...'`), key included."""
    if isinstance(error, urllib.error.HTTPError):
        return f"{type(error).__name__} {error.code}"
    return type(error).__name__


# Wallet addresses
MAINNET_ADDRESS = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
TESTNET_ADDRESS = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"

# Solana addresses
SOLANA_MAINNET_ADDRESS = "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq"
SOLANA_TESTNET_ADDRESS = "6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv"

# Sui addresses
SUI_MAINNET_ADDRESS = "0xe7bbf2b13f7d72714760aa16e024fa1b35a978793f9893d0568a4fbf356a764a"
SUI_TESTNET_ADDRESS = "0xabbd16a2fab2a502c9cfe835195a6fc7d70bfc27cffb40b8b286b52a97006e67"

# Sui gRPC endpoints when RPC_URL_SUI / RPC_URL_SUI_TESTNET are unset: the
# facilitator's defaults (DEFAULT_ENDPOINTS_* in src/chain/sui/rpc.rs, whose
# test every_configured_sui_endpoint_list_is_the_default_one holds the two equal).
SUI_GRPC_DEFAULTS = {
    "sui-mainnet": "https://fullnode.mainnet.sui.io:443,https://sui-grpc.publicnode.com:443",
    "sui-testnet": "https://fullnode.testnet.sui.io:443,https://sui-testnet-grpc.publicnode.com:443",
}
# GraphQL, tried after every gRPC endpoint: Mysten's, the only public Sui
# GraphQL that answered on 2026-11-15.
SUI_GRAPHQL_URLS = {
    "sui-mainnet": ["https://graphql.mainnet.sui.io/graphql"],
    "sui-testnet": ["https://graphql.testnet.sui.io/graphql"],
}
SUI_COIN_TYPE = "0x2::sui::SUI"


def sui_grpc_endpoints(env_var: str, network: str) -> list[str]:
    """The gRPC endpoints `env_var` names (comma-separated; trimmed, repeats
    dropped, anything but an http(s) URL with a host skipped), or the defaults
    when it names none. The same reading as `endpoint_list` in rpc.rs."""
    urls: list[str] = []
    for entry in (os.environ.get(env_var) or "").split(","):
        entry = entry.strip()
        try:
            parts = urlsplit(entry)
        except ValueError:
            continue
        if parts.scheme.lower() not in ("http", "https") or not parts.hostname:
            continue
        if entry.rstrip("/").lower() not in [u.rstrip("/").lower() for u in urls]:
            urls.append(entry)
    return urls or SUI_GRPC_DEFAULTS[network].split(",")

# NEAR addresses
NEAR_MAINNET_ADDRESS = "uvd-facilitator.near"
NEAR_TESTNET_ADDRESS = "uvd-facilitator.testnet"

# Stellar addresses
STELLAR_MAINNET_ADDRESS = "GCHPGXJT2WFFRFCA5TV4G4E3PMMXLNIDUH27PKDYA4QJ2XGYZWGFZNHB"
STELLAR_TESTNET_ADDRESS = "GBBFZMLUJEZVI32EN4XA2KPP445XIBTMTRBLYWFIL556RDTHS2OWFQ2Z"

# Algorand addresses
ALGORAND_MAINNET_ADDRESS = "KIMS5H6QLCUDL65L5UBTOXDPWLMTS7N3AAC3I6B2NCONEI5QIVK7LH2C2I"
ALGORAND_TESTNET_ADDRESS = "5DPPDQNYUPCTXRZWRYSF3WPYU6RKAUR25F3YG4EKXQRHV5AUAI62H5GXL4"

# XRPL addresses (native XRP Ledger, classic r... addresses; NOT the EVM sidechain)
XRPL_MAINNET_ADDRESS = "rfADKkVXBNqK3z72tVSS3LVzAR3psYkonp"
XRPL_TESTNET_ADDRESS = "rGhTioKAFHe75KgVnQtacRiKFuPv28Wbwk"


# Public RPC fallbacks (no API keys)
PUBLIC_RPCS = {
    "base": "https://mainnet.base.org",
    "avalanche": "https://avalanche-c-chain-rpc.publicnode.com",
    "polygon": "https://polygon.drpc.org",
    "optimism": "https://mainnet.optimism.io",
    "celo": "https://celo-rpc.quickapi.com",
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
    with fallback to environment variables and then public RPCs. A key with no
    entry in PUBLIC_RPCS (robinhood) has no public fallback: without its private
    RPC the network is not measured (a null balance).
    """
    # Load private RPCs from Secrets Manager for mainnets
    private_rpcs = {}
    for network_key in ["base", "avalanche", "polygon", "optimism", "celo",
                        "hyperevm", "ethereum", "arbitrum", "unichain", "robinhood",
                        "solana", "near"]:
        private_rpc = get_private_rpc(network_key)
        if private_rpc:
            private_rpcs[network_key] = private_rpc
            print(f"Loaded private RPC for {network_key}")
        elif network_key in PUBLIC_RPCS:
            print(f"No private RPC for {network_key}, will use public")
        else:
            print(f"No private RPC for {network_key}, not measured (no public fallback)")

    configs = {
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
        # Sui. JSON-RPC is gone from every public Sui node (2026-11-14:
        # fullnode.*.sui.io answer -32601, publicnode -32000), so there is no
        # JSON-RPC URL here at all. The balance is read over gRPC-web from the
        # facilitator's own endpoint list (RPC_URL_SUI / RPC_URL_SUI_TESTNET,
        # comma-separated), then over GraphQL. See fetch_sui_balance.
        "sui-mainnet": {
            "grpc": sui_grpc_endpoints("RPC_URL_SUI", "sui-mainnet"),
            "graphql": SUI_GRAPHQL_URLS["sui-mainnet"],
            "address": SUI_MAINNET_ADDRESS,
            "type": "sui"
        },
        "sui-testnet": {
            "grpc": sui_grpc_endpoints("RPC_URL_SUI_TESTNET", "sui-testnet"),
            "graphql": SUI_GRAPHQL_URLS["sui-testnet"],
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
        # SKALE Base (gasless L3, native token is CREDIT)
        "skale-mainnet": {
            "rpcs": ["https://skale-base.skalenodes.com/v1/base"],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "skale-testnet": {
            "rpcs": ["https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        # Scroll
        "scroll-mainnet": {
            "rpcs": ["https://scroll.drpc.org", "https://1rpc.io/scroll", "https://rpc.scroll.io"],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        # Robinhood Chain (Arbitrum Orbit L2). Mainnet reads only the `robinhood` key
        # of facilitator-rpc-mainnet (QuickNode) or RPC_URL_ROBINHOOD, never the
        # public endpoint (owner's rule): without them, or with the private one down,
        # its balance is null -- not measured. We have no private testnet one.
        "robinhood-mainnet": {
            "rpcs": [
                private_rpcs.get("robinhood"),
                os.environ.get("RPC_URL_ROBINHOOD"),
            ],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "robinhood-testnet": {
            "rpcs": ["https://rpc.testnet.chain.robinhood.com"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        # HashKey Chain (OP Stack L2; gas is HSK; official keyless RPCs)
        "hashkey-mainnet": {
            "rpcs": ["https://mainnet.hsk.xyz"],
            "address": MAINNET_ADDRESS,
            "type": "evm"
        },
        "hashkey-testnet": {
            "rpcs": ["https://testnet.hsk.xyz"],
            "address": TESTNET_ADDRESS,
            "type": "evm"
        },
        # XRPL (native XRP Ledger; public JSON-RPC, balance in drops, 1 XRP = 1e6 drops)
        "xrpl-mainnet": {
            "rpcs": ["https://s1.ripple.com:51234/", "https://xrplcluster.com/"],
            "address": XRPL_MAINNET_ADDRESS,
            "type": "xrpl"
        },
        "xrpl-testnet": {
            "rpcs": ["https://s.altnet.rippletest.net:51234/"],
            "address": XRPL_TESTNET_ADDRESS,
            "type": "xrpl"
        },
    }

    # One native USDC balance (18 decimals). Its ERC-20 balance is a view of
    # the same funds, not a second balance to add. Monitor only enabled networks.
    for name, env, address, chain_id in (
        ("arc-mainnet", "RPC_URL_ARC", MAINNET_ADDRESS, 5042),
        ("arc-testnet", "RPC_URL_ARC_TESTNET", TESTNET_ADDRESS, 5042002),
    ):
        rpc = os.environ.get(env)
        if rpc:
            configs[name] = {"rpcs": [rpc], "address": address, "type": "evm", "chain_id": chain_id}
    for suffix, name, mirror in (
        ("TESTNET", "hedera-testnet", "https://testnet.mirrornode.hedera.com"),
        ("MAINNET", "hedera-mainnet", "https://mainnet-public.mirrornode.hedera.com"),
    ):
        account = os.environ.get(f"HEDERA_ACCOUNT_ID_{suffix}")
        if account:
            configs[name] = {"address": account, "mirror": mirror, "type": "hedera"}
    return configs


def fetch_json(url: str, data: bytes | None = None, timeout: float = 10) -> dict:
    """Make an HTTP request and return JSON response.

    The User-Agent is not cosmetic. Providers behind Cloudflare bot protection
    reject urllib's default "Python-urllib/3.x" outright -- celo-rpc.quickapi.com
    answers it with HTTP 403 "error code: 1010" while serving the identical
    request fine otherwise, which silently nulled the Celo mainnet balance.
    """
    headers = {
        "Content-Type": "application/json",
        "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36",
        "Accept": "application/json",
    }
    req = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return json.loads(response.read().decode())


def fetch_evm_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for an EVM network with RPC fallback."""
    rpcs = [r for r in config.get("rpcs", []) if r]  # Filter out None values

    for rpc_url in rpcs:
        try:
            if "chain_id" in config:
                identity = fetch_json(rpc_url, json.dumps({
                    "jsonrpc": "2.0", "method": "eth_chainId", "params": [], "id": 1,
                }).encode())
                if int(identity.get("result", "0x0"), 16) != config["chain_id"]:
                    continue
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
            print(f"Error fetching {network} from {rpc_host(rpc_url)}: {rpc_error(e)}")
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
            print(f"Error fetching {network} from {rpc_host(rpc_url)}: {rpc_error(e)}")
            continue

    return network, None


class SuiReadError(Exception):
    """A Sui endpoint answered without a balance (a gRPC status, a GraphQL error)."""


def _pb_varint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _pb_string(field: int, value: str) -> bytes:
    raw = value.encode()
    return _pb_varint((field << 3) | 2) + _pb_varint(len(raw)) + raw


def _pb_fields(buf: bytes) -> dict[int, list]:
    """Decode a flat protobuf message: {field: [value, ...]} (int for varints,
    bytes for length-delimited fields; fixed-width fields are skipped)."""
    fields: dict[int, list] = {}
    i = 0

    def varint() -> int:
        nonlocal i
        result, shift = 0, 0
        while True:
            if i >= len(buf):
                raise SuiReadError("truncated protobuf")
            byte = buf[i]
            i += 1
            result |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return result

    while i < len(buf):
        key = varint()
        field, wire = key >> 3, key & 7
        if wire == 0:
            value = varint()
        elif wire == 2:
            length = varint()
            if i + length > len(buf):
                raise SuiReadError("truncated protobuf")
            value = buf[i:i + length]
            i += length
        elif wire in (1, 5):
            i += 8 if wire == 1 else 4
            continue
        else:
            raise SuiReadError(f"protobuf wire type {wire}")
        fields.setdefault(field, []).append(value)
    return fields


def _grpc_web_frames(body: bytes) -> tuple[bytes | None, dict[str, str]]:
    """Split a gRPC-web body into its (last) message and its trailers."""
    message, trailers = None, {}
    i = 0
    while i + 5 <= len(body):
        flag = body[i]
        length = int.from_bytes(body[i + 1:i + 5], "big")
        payload = body[i + 5:i + 5 + length]
        i += 5 + length
        if flag & 0x80:
            for line in payload.decode(errors="replace").splitlines():
                key, _, value = line.partition(":")
                trailers[key.strip().lower()] = value.strip()
        else:
            message = payload
    return message, trailers


def sui_grpc_web_balance(base_url: str, owner: str, coin_type: str = SUI_COIN_TYPE,
                         timeout: float = 8) -> int:
    """`StateService.GetBalance` over gRPC-web (HTTP/1.1, which urllib speaks;
    plain gRPC needs HTTP/2). The total balance, in the coin's smallest unit."""
    message = _pb_string(1, owner) + _pb_string(2, coin_type)
    body = b"\x00" + len(message).to_bytes(4, "big") + message
    req = urllib.request.Request(
        base_url.rstrip("/") + "/sui.rpc.v2.StateService/GetBalance",
        data=body,
        headers={
            "Content-Type": "application/grpc-web+proto",
            "Accept": "application/grpc-web+proto",
            "X-Grpc-Web": "1",
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36",
        },
    )
    with urllib.request.urlopen(req, timeout=timeout) as response:
        header_status = response.headers.get("grpc-status")
        payload = response.read()
    data, trailers = _grpc_web_frames(payload)
    status = trailers.get("grpc-status", header_status)
    if status != "0":
        raise SuiReadError(f"grpc-status {status}")
    balance = _pb_fields(data or b"").get(1)
    if not balance:
        raise SuiReadError("answer without a balance")
    total = _pb_fields(balance[0]).get(3)
    if not total:
        raise SuiReadError("balance without a total")
    return int(total[0])


SUI_GRAPHQL_BALANCE = (
    "query($a: SuiAddress!) { address(address: $a) "
    "{ balance(coinType: \"0x2::sui::SUI\") { totalBalance } } }"
)


def sui_graphql_balance(url: str, owner: str, timeout: float = 8) -> int:
    """`Address.balance` over GraphQL: the total SUI balance, in MIST."""
    payload = json.dumps({"query": SUI_GRAPHQL_BALANCE, "variables": {"a": owner}}).encode()
    data = fetch_json(url, payload, timeout=timeout)
    if data.get("errors"):
        raise SuiReadError("graphql errors")
    address = (data.get("data") or {}).get("address")
    if address is None:
        raise SuiReadError("answer without an address")
    balance = address.get("balance")
    # An address that never held SUI has no balance row.
    return int(balance["totalBalance"]) if balance else 0


def fetch_sui_balance(network: str, config: dict) -> tuple[str, str | None]:
    """SUI (gas) balance: gRPC-web over each of the facilitator's endpoints,
    then GraphQL, first answer wins. No JSON-RPC: no public Sui node serves it.

    Only Mysten's fullnodes speak gRPC-web; publicnode answers 415 to it
    (measured 2026-11-15) and is tried anyway, so a list that names it first
    still reads. GraphQL is Mysten's too: no other public Sui GraphQL answered.
    """
    attempts = [(url, sui_grpc_web_balance) for url in config.get("grpc", [])]
    attempts += [(url, sui_graphql_balance) for url in config.get("graphql", [])]
    for url, read in attempts:
        try:
            balance_mist = read(url, config["address"])
            return network, f"{balance_mist / 1e9:.4f}"
        except Exception as e:
            print(f"Error fetching {network} from {rpc_host(url)}: {rpc_error(e)}")
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


def fetch_xrpl_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Fetch balance for XRPL (native XRP Ledger) via account_info JSON-RPC.

    Balance is returned in drops (1 XRP = 1e6 drops). Uses RPC fallback.
    """
    rpcs = [r for r in config.get("rpcs", []) if r]  # Filter out None values

    for rpc_url in rpcs:
        try:
            payload = json.dumps({
                "method": "account_info",
                "params": [{
                    "account": config["address"],
                    "ledger_index": "validated"
                }]
            }).encode()

            data = fetch_json(rpc_url, payload, timeout=8)
            result = data.get("result", {})
            account_data = result.get("account_data", {})
            if "Balance" in account_data:
                balance_drops = int(account_data["Balance"])
                balance_xrp = balance_drops / 1e6
                return network, f"{balance_xrp:.4f}"
        except Exception as e:
            print(f"Error fetching {network} from {rpc_host(rpc_url)}: {rpc_error(e)}")
            continue

    return network, None


def fetch_hedera_balance(network: str, config: dict) -> tuple[str, str | None]:
    """Native HBAR: numeric account ID, Mirror tinybars, exactly 8 decimals."""
    import re
    from decimal import Decimal
    account = config["address"]
    if not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.[1-9][0-9]*", account):
        return network, None
    try:
        data = fetch_json(f"{config['mirror']}/api/v1/accounts/{account}?transactions=false", timeout=8)
        balance = data.get("balance", {}).get("balance")
        if data.get("account") != account or data.get("deleted") is not False or type(balance) is not int or balance < 0:
            return network, None
        return network, f"{Decimal(balance) / Decimal(100000000):.8f}"
    except Exception:
        print(f"Native Hedera balance unavailable: {network}")
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
            elif network_type == "hedera":
                futures.append(executor.submit(fetch_hedera_balance, network, config))
            elif network_type == "xrpl":
                futures.append(executor.submit(fetch_xrpl_balance, network, config))

        for future in as_completed(futures):
            network, balance = future.result()
            balances[network] = balance

    # Update cache
    _cache = balances
    _cache_timestamp = now

    return balances


METRIC_NAMESPACE = "Facilitator/Chains"

# The chains an alarm reads Facilitator/Chains for: the keys of local.monitored_chains in
# terraform/environments/production/alerts.tf (evm_fee_cap_gwei, hedera-mainnet and
# hand_set_floors). arc-mainnet and hedera-mainnet stay listed even while alerts.tf
# switches their alarms off: two spare series are cheaper than an alarm that is turned
# back on and finds no data. Every other chain is still read and still returned to the
# landing page; it is only not published, because a custom metric no alarm reads is
# paid every month for nothing (COSTO-X402 B7: 85 series, 27 networks without any
# alarm). tests/scripts/test_chain_metrics_balances.py fails if this drifts.
MONITORED_CHAINS = frozenset({
    "arbitrum-mainnet",
    "arc-mainnet",
    "avalanche-mainnet",
    "base-mainnet",
    "celo-mainnet",
    "ethereum-mainnet",
    "hedera-mainnet",
    "monad-mainnet",
    "optimism-mainnet",
    "polygon-mainnet",
    "sui-mainnet",
    "solana-mainnet",
    "stellar-mainnet",
    "near-mainnet",
    "algorand-mainnet",
    "xrpl-mainnet",
})


def publish_chain_metrics(balances: dict[str, str | None]) -> None:
    """
    Publish per-chain health and balance to CloudWatch.

    A null balance already means "we could not read this chain" -- an unreachable
    RPC, a deprecated endpoint, a dead node all land here. That signal existed all
    along and went only to the landing page, where nobody watched it: Celo ran dry
    and the Sui RPC went away, both plainly visible in this readout, and neither
    announced itself. Emitting it as a metric is what lets an alarm see it.

    Never raises. This runs after the balances are already in hand, and a metrics
    problem must not turn a working balance response into a 500 -- the same rule
    the facilitator applies to its own transaction store.
    """
    try:
        import boto3

        cw = boto3.client("cloudwatch")
        data = []

        for network, balance in balances.items():
            if network not in MONITORED_CHAINS:
                continue
            healthy = balance is not None
            data.append({
                "MetricName": "ChainRpcHealthy",
                "Dimensions": [{"Name": "Chain", "Value": network}],
                "Value": 1.0 if healthy else 0.0,
                "Unit": "None",
            })
            if healthy:
                try:
                    data.append({
                        "MetricName": "ChainNativeBalance",
                        "Dimensions": [{"Name": "Chain", "Value": network}],
                        "Value": float(balance),
                        "Unit": "None",
                    })
                except (TypeError, ValueError):
                    # Readable but not a number: report it as reachable and skip
                    # the balance rather than guessing a value an alarm would act on.
                    print(f"[WARN] non-numeric balance for {network}: {balance!r}")

        # PutMetricData caps at 1000 metrics per call; we send ~32. Chunk anyway so
        # adding chains never silently truncates the tail.
        for i in range(0, len(data), 1000):
            cw.put_metric_data(Namespace=METRIC_NAMESPACE, MetricData=data[i:i + 1000])

        unreachable = [n for n, b in balances.items() if b is None]
        print(f"[OK] published {len(data)} metrics; unreachable={unreachable or 'none'}")
    except Exception as e:  # noqa: BLE001 - never fail the request over metrics
        print(f"[WARN] could not publish chain metrics: {type(e).__name__}: {e}")


def lambda_handler(event: dict, context: Any) -> dict:
    """
    AWS Lambda handler.
    Returns all wallet balances as JSON, and publishes per-chain metrics.
    """
    try:
        balances = fetch_all_balances()
        publish_chain_metrics(balances)

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
