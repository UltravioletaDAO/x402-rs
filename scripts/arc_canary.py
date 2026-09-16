#!/usr/bin/env python3
"""Read-only Arc preflight; --execute sends ONE micro-USDC between our EVM wallets.

Requires eth-account, eth-utils; boto3 only for --execute. Keys stay in memory.
Never retries an uncertain settle. Keep its nonce/hash to reconcile that payment.
"""
import argparse
from decimal import Decimal
import json
from pathlib import Path
import secrets
import time
import urllib.error
import urllib.request
from urllib.parse import urlsplit

from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import keccak

USDC = "0x3600000000000000000000000000000000000000"
NETWORKS = {
    "arc": (5042, "https://rpc.mainnet.arc.io", "evm_mainnets"),
    "arc-testnet": (5042002, "https://rpc.testnet.arc.io", "evm_testnets"),
}


def request(url, body=None, headers=None):
    # The production server gets the client IP from its reverse proxy. A private
    # SSH tunnel has no proxy, so supply the actual loopback client identity.
    local_headers = {"X-Real-IP": "127.0.0.1"} if urlsplit(url).hostname in ("localhost", "127.0.0.1") else {}
    req = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json",
                                          "User-Agent": "uvd-x402-facilitator", **local_headers, **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=45) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def rpc(url, method, params):
    status, body = request(url, {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    if status != 200 or "result" not in body:
        raise RuntimeError(f"RPC {method} failed (HTTP {status}); no payment was retried")
    return body["result"]


def balance(url, address):
    return int(rpc(url, "eth_call", [{"to": USDC, "data": "0x70a08231" + address[2:].zfill(64)}, "latest"]), 16)


def domain_separator(chain_id):
    return "0x" + keccak(
        keccak(text="EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
        + keccak(text="USDC") + keccak(text="2") + chain_id.to_bytes(32, "big")
        + bytes.fromhex(USDC[2:]).rjust(32, b"\0")
    ).hex()


def payment(network, chain_id, payer, recipient, key, nonce):
    now = int(time.time())
    auth = {"from": payer, "to": recipient, "value": 1, "validAfter": now - 10,
            "validBefore": now + 120, "nonce": nonce}
    typed = {
        "types": {
            "EIP712Domain": [{"name": n, "type": t} for n, t in
                             [("name", "string"), ("version", "string"), ("chainId", "uint256"),
                              ("verifyingContract", "address")]],
            "TransferWithAuthorization": [{"name": n, "type": t} for n, t in
                                          [("from", "address"), ("to", "address"), ("value", "uint256"),
                                           ("validAfter", "uint256"), ("validBefore", "uint256"), ("nonce", "bytes32")]],
        },
        "primaryType": "TransferWithAuthorization",
        "domain": {"name": "USDC", "version": "2", "chainId": chain_id, "verifyingContract": USDC},
        "message": auth,
    }
    signature = Account.sign_message(encode_typed_data(full_message=typed), key).signature
    wire = {k: str(v) if isinstance(v, int) else v for k, v in auth.items()}
    return {
        "x402Version": 1,
        "paymentPayload": {"x402Version": 1, "scheme": "exact", "network": network,
                           "payload": {"signature": "0x" + signature.hex(), "authorization": wire}},
        "paymentRequirements": {"scheme": "exact", "network": network, "maxAmountRequired": "1",
                                "resource": "https://example.com/arc-canary", "description": "Arc USDC canary",
                                "mimeType": "application/json", "payTo": recipient, "maxTimeoutSeconds": 120,
                                "asset": USDC, "extra": {"name": "USDC", "version": "2"}},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--network", choices=NETWORKS, required=True)
    parser.add_argument("--facilitator", default="https://facilitator.ultravioletadao.xyz")
    parser.add_argument("--execute", action="store_true", help="Transfer one micro-USDC to our other EVM wallet, plus gas")
    args = parser.parse_args()
    chain_id, url, category = NETWORKS[args.network]
    config = json.loads((Path(__file__).resolve().parents[1] / "config/supported_tokens.json").read_text(encoding="utf-8"))
    payer = config[category][args.network]["facilitatorWallet"]
    other = "arc-testnet" if args.network == "arc" else "arc"
    recipient = config[NETWORKS[other][2]][other]["facilitatorWallet"]
    if int(rpc(url, "eth_chainId", []), 16) != chain_id:
        raise RuntimeError("Wrong RPC network")
    block = rpc(url, "eth_getBlockByNumber", ["latest", False])
    if abs(int(time.time()) - int(block["timestamp"], 16)) > 120:
        raise RuntimeError("RPC block is stale; refusing to sign")
    call = lambda selector: rpc(url, "eth_call", [{"to": USDC, "data": selector}, block["number"]])
    if int(call("0x313ce567"), 16) != 6 or call("0x3644e515").lower() != domain_separator(chain_id):
        raise RuntimeError("USDC decimals or domain changed; refusing to sign")
    native = int(rpc(url, "eth_getBalance", [payer, "latest"]), 16)
    status, supported = request(args.facilitator.rstrip("/") + "/supported")
    served = status == 200 and any(k.get("network") in (args.network, f"eip155:{chain_id}")
                                 and k.get("scheme") == "exact" for k in supported.get("kinds", []))
    print(json.dumps({"network": args.network, "chain_id": chain_id, "block": int(block["number"], 16),
                      "domain_matches": True, "payer": payer, "recipient": recipient,
                      "native_usdc": str(Decimal(native) / Decimal(10**18)), "served": served,
                      "execute": args.execute}), flush=True)
    if not args.execute:
        return
    if not served or native < 10**17:
        raise RuntimeError("Canary requires the served network and at least 0.1 USDC in its signer")
    if int(rpc(url, "eth_gasPrice", []), 16) > 50_000_000_000:
        raise RuntimeError("Gas quote exceeds this canary's 50 gwei guard; review before retrying")
    import boto3
    env = "mainnet" if args.network == "arc" else "testnet"
    raw = boto3.client("secretsmanager", region_name="us-east-2").get_secret_value(
        SecretId=f"facilitator-evm-{env}-private-key")["SecretString"]
    key = json.loads(raw)["private_key"]
    if Account.from_key(key).address.lower() != payer.lower():
        raise RuntimeError("Configured signer differs from the documented canary wallet")
    nonce = "0x" + secrets.token_hex(32)
    body = payment(args.network, chain_id, payer, recipient, key, nonce)
    del key, raw
    base = args.facilitator.rstrip("/")
    status, verified = request(base + "/verify", body)
    if status != 200 or not verified.get("isValid"):
        raise RuntimeError(f"Verify refused the canary (HTTP {status}); no settle requested")
    before = balance(url, recipient)
    print(json.dumps({"settle_requested": True, "network": args.network, "nonce": nonce}), flush=True)
    status, settled = request(base + "/settle", body)
    tx = settled.get("transaction")
    print(json.dumps({"http": status, "success": settled.get("success"), "transaction": tx}), flush=True)
    if status != 200 or not settled.get("success") or not isinstance(tx, str):
        raise RuntimeError("Settle not confirmed; reconcile the nonce/hash, do not create another payment")
    receipt = rpc(url, "eth_getTransactionReceipt", [tx])
    if not receipt or int(receipt["status"], 16) != 1:
        raise RuntimeError("Successful receipt missing; reconcile the reported transaction")
    topic = "0x" + keccak(text="Transfer(address,address,uint256)").hex()
    matching = [log for log in receipt["logs"] if log["address"].lower() == USDC.lower()
                and len(log["topics"]) == 3 and log["topics"][0].lower() == topic
                and log["topics"][1][-40:].lower() == payer[2:].lower()
                and log["topics"][2][-40:].lower() == recipient[2:].lower()
                and int(log["data"], 16) == 1]
    if len(matching) != 1 or balance(url, recipient) - before != 1:
        raise RuntimeError("The receipt and recipient balance must prove exactly one micro-USDC")
    replay_status, replay = request(base + "/settle", body)
    if replay.get("success") and replay.get("transaction") != tx:
        raise RuntimeError("Replay returned a different transaction; investigate immediately")
    if balance(url, recipient) - before != 1:
        raise RuntimeError("Recipient balance changed after replay")
    cost = Decimal(int(receipt["gasUsed"], 16) * int(receipt["effectiveGasPrice"], 16)) / Decimal(10**18)
    print(json.dumps({"canary_passed": True, "network": args.network, "transaction": tx,
                      "usdc_atomic_units_received": 1, "gas_usdc": str(cost),
                      "replay_http": replay_status, "replay_success": replay.get("success")}), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # Do not dump exception internals: HTTP URLs or SDK errors can carry secrets.
        if isinstance(error, RuntimeError):
            raise SystemExit(str(error))
        raise SystemExit(f"Canary stopped: {type(error).__name__}; no automatic settle retry")
