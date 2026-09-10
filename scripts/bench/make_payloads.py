#!/usr/bin/env python3
"""make_payloads.py -- genera payloads x402 firmados de verdad para el banco de carga.

Por que hace falta firmar: /verify y /settle rechazan una firma invalida antes de
llegar al RPC, asi que un payload de mentira mide el parser, no el camino de pago.
Aca se firma EIP-712 TransferWithAuthorization con una llave EFIMERA generada en el
momento; nunca toca ninguna cadena y no vale nada.

Red por defecto: base-sepolia (testnet). El dominio EIP-712 (name/version/chainId/
verifyingContract) sale de src/network.rs; si cambia alli hay que cambiarlo aca.

Uso:
  python3 scripts/bench/make_payloads.py --out-dir /tmp/bench --count 64
"""
import argparse
import json
import os
import secrets
import time

from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import to_checksum_address

# src/network.rs: USDC_BASE_SEPOLIA -- address, eip712.name, eip712.version
NETWORKS = {
    "base-sepolia": {
        "chain_id": 84532,
        "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        "eip712_name": "USDC",
        "eip712_version": "2",
    },
}

TYPES = {
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
}


def build(network: str, pay_to: str, amount: int, ttl: int):
    cfg = NETWORKS[network]
    pay_to = to_checksum_address(pay_to.lower())
    acct = Account.create()
    now = int(time.time())
    auth = {
        "from": acct.address,
        "to": pay_to,
        "value": amount,
        "validAfter": now - 600,
        "validBefore": now + ttl,
        "nonce": "0x" + secrets.token_hex(32),
    }
    domain = {
        "name": cfg["eip712_name"],
        "version": cfg["eip712_version"],
        "chainId": cfg["chain_id"],
        "verifyingContract": to_checksum_address(cfg["asset"].lower()),
    }
    msg = dict(auth)
    msg["nonce"] = bytes.fromhex(auth["nonce"][2:])
    signable = encode_typed_data(
        full_message={
            "types": TYPES,
            "primaryType": "TransferWithAuthorization",
            "domain": domain,
            "message": msg,
        }
    )
    signed = acct.sign_message(signable)
    # EIP-2: la firma de eth_account ya viene en forma canonica (s <= n/2).
    return {
        "x402Version": 1,
        "paymentPayload": {
            "x402Version": 1,
            "scheme": "exact",
            "network": network,
            "payload": {
                "signature": "0x" + signed.signature.hex().replace("0x", ""),
                "authorization": {
                    "from": auth["from"],
                    "to": auth["to"],
                    "value": str(auth["value"]),
                    "validAfter": str(auth["validAfter"]),
                    "validBefore": str(auth["validBefore"]),
                    "nonce": auth["nonce"],
                },
            },
        },
        "paymentRequirements": {
            "scheme": "exact",
            "network": network,
            "maxAmountRequired": str(amount),
            "resource": "http://127.0.0.1:3000/bench",
            "description": "capacity benchmark",
            "mimeType": "application/json",
            "payTo": pay_to,
            "maxTimeoutSeconds": 60,
            "asset": cfg["asset"],
            "extra": {"name": cfg["eip712_name"], "version": cfg["eip712_version"]},
        },
    }


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--network", default="base-sepolia", choices=sorted(NETWORKS))
    p.add_argument("--out-dir", required=True)
    p.add_argument("--count", type=int, default=64)
    p.add_argument("--amount", type=int, default=1000)
    p.add_argument("--ttl", type=int, default=3600)
    p.add_argument(
        "--pay-to",
        default="0x000000000000000000000000000000000000dEaD",
        help="destinatario; solo tiene que coincidir entre authorization.to y payTo",
    )
    a = p.parse_args()
    os.makedirs(a.out_dir, exist_ok=True)
    paths = []
    for i in range(a.count):
        body = build(a.network, a.pay_to, a.amount, a.ttl)
        path = os.path.join(a.out_dir, f"payment_{i:03d}.json")
        with open(path, "w") as fh:
            json.dump(body, fh)
        paths.append(path)
    print(json.dumps({"network": a.network, "count": len(paths), "files": paths}))


if __name__ == "__main__":
    main()
