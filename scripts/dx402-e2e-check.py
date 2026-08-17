#!/usr/bin/env python3
"""End-to-end check of DX402 against a live facilitator.

Runs the full loop with real HTTP: seal a body, anchor it, fetch the ciphertext
back, decrypt it with the payer key, and confirm a *different* key cannot.

    PYTHONPATH=../uvd-x402-sdk-python/src python3 scripts/dx402-e2e-check.py

This is the acceptance test from the KarmaCadabra handoff, run for real. It uses
a throwaway key and a synthetic transaction hash: v0.1 of `/dx402/anchor` does
not verify that the payment exists on-chain, which is a real limitation worth
knowing about rather than a trick to make the test pass.

Exit code 0 = every check passed.
"""

import base64
import json
import os
import sys
import urllib.request

FACILITATOR = os.environ.get(
    "DX402_FACILITATOR", "https://facilitator.ultravioletadao.xyz"
)

try:
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

    from uvd_x402_sdk.dx402 import (
        _parse_sealed,
        _unseal,
        content_hash,
        payment_id,
        seal_evidence,
    )
except ImportError as exc:
    sys.exit(f"missing dependency: {exc}\nrun with PYTHONPATH pointing at the SDK src/")


def get(path: str):
    with urllib.request.urlopen(f"{FACILITATOR}{path}", timeout=30) as r:
        return r.status, r.read()


def post_json(path: str, payload: dict):
    req = urllib.request.Request(
        f"{FACILITATOR}{path}",
        data=json.dumps(payload).encode(),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


checks: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    checks.append((name, ok, detail))
    print(f"  {'PASS' if ok else 'FAIL'}  {name}" + (f"  — {detail}" if detail else ""))


print(f"DX402 end-to-end against {FACILITATOR}\n")

# --- 0. the extension is advertised ------------------------------------------
status, body = get("/supported")
exts = json.loads(body).get("extensions", [])
check("durable-evidence advertised in /supported", "durable-evidence" in exts, str(exts))

status, body = get("/dx402/stats")
stats = json.loads(body)
signer = stats.get("receiptSigner", "")
check("/dx402/stats responds", status == 200, f"signer={signer}")
anchored_before = stats.get("anchored", 0)

# --- 1. seal ------------------------------------------------------------------
PRIV = bytes.fromhex("11" * 32)
sk = ec.derive_private_key(int.from_bytes(PRIV, "big"), ec.SECP256K1())
payer_key = sk.public_key().public_bytes(Encoding.X962, PublicFormat.CompressedPoint)

BODY = json.dumps(
    {"message": "DX402 end-to-end check", "secret": "only-the-payer-should-read-this"}
).encode()

TX = "0x" + "ab" * 32
PID = payment_id("eip155:8453", TX)
blob = seal_evidence(BODY, payer_key, PID)
check("sealed the body", len(blob) > len(BODY), f"{len(blob)} bytes")

# --- 2. anchor, sending the ciphertext (no bucket of our own) -----------------
status, resp = post_json(
    "/dx402/anchor",
    {
        "paymentId": PID,
        "network": "base",
        "txHash": TX,
        "payer": "0x103040545AC5031A11E8C03dd11324C7333a13C7",
        "payee": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
        "sealed": base64.b64encode(blob).decode(),
        "backend": "s3",
        "contentHash": content_hash(BODY),
        "keyAlg": "ECIES-secp256k1",
        "mode": "direct",
        "retention": "90d",
    },
)
check("POST /dx402/anchor accepted", status == 201, str(resp)[:200])
if status != 201:
    sys.exit(1)

pointer = resp.get("pointer", "")
check("facilitator issued a pointer", bool(pointer), pointer)
check("receipt was signed", bool(resp.get("receipt")), (resp.get("receipt") or "")[:20] + "…")

# --- 3. look it up ------------------------------------------------------------
status, body = get(f"/dx402/evidence/{PID}")
ev = json.loads(body)
check("GET /dx402/evidence returns the record", status == 200)
check(
    "contentHash round-trips through the index",
    ev.get("contentHash") == content_hash(BODY),
)
check("mode is direct", ev.get("mode") == "direct", str(ev.get("mode")))

status, body = get(f"/dx402/receipt/{PID}")
check("GET /dx402/receipt returns a signed receipt", status == 200)

# --- 4. fetch the ciphertext back --------------------------------------------
status, fetched = get(f"/dx402/blob/{PID}")
check("GET /dx402/blob returns the sealed bytes", status == 200, f"{len(fetched)} bytes")
check("bytes came back verbatim", fetched == blob)

# --- 5. only the payer can read it -------------------------------------------
plaintext = _unseal(_parse_sealed(fetched), PRIV, PID.encode())
check("the payer decrypts it", plaintext == BODY)
check("contentHash matches the decrypted body", content_hash(plaintext) == ev["contentHash"])

other = bytes.fromhex("22" * 32)
try:
    _unseal(_parse_sealed(fetched), other, PID.encode())
    check("a different wallet CANNOT decrypt it", False, "it decrypted — privacy is broken")
except Exception:
    check("a different wallet CANNOT decrypt it", True)

# --- 6. unknown payments 404 rather than 500 ---------------------------------
try:
    get("/dx402/evidence/0xdoesnotexist")
    check("unknown payment returns 404", False, "got 200")
except urllib.error.HTTPError as e:
    check("unknown payment returns 404", e.code == 404, f"got {e.code}")

# --- 7. payments still work --------------------------------------------------
status, body = get("/health")
check("facilitator still healthy", json.loads(body).get("status") == "healthy")

failed = [c for c in checks if not c[1]]
print(f"\n{len(checks) - len(failed)}/{len(checks)} passed")
if failed:
    print("FAILED: " + ", ".join(c[0] for c in failed))
sys.exit(1 if failed else 0)
