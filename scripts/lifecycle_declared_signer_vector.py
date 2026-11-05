#!/usr/bin/env python3
"""Generate the shared policy vector for the escrow lifecycle `declared_signer` role.

Writes tests/fixtures/escrow/lifecycle-declared-signer-vector.json. Two parties load that file
byte for byte: the facilitator's Rust tests (src/payment_operator/lifecycle_vector.rs) and the
Execution Market conformance test that signs lifecycle orders. The expected verdicts are policy,
written here by hand; every digest and signature is computed by eth_account, an implementation
independent of the Rust code it checks, so the Rust side is never compared to itself.

Every key is derived from a public seed string (private key = keccak256(utf8(seed))). They are
TEST keys: never fund one, never declare one in the production table.

Deterministic: fixed seeds, RFC 6979 signatures, nonces derived from the case ids, a fixed JSON
layout. Every run writes the same bytes; their SHA-256 is pinned in
src/payment_operator/lifecycle_vector.rs (FIXTURE_SHA256), so both sides can compare it.

No value is written as 0x followed by 64 hex digits (the repository's pre-commit hook refuses
that shape): 32-byte values (nonce, digest) and the 65-byte signature are plain hex without the
0x prefix, and the salt is a decimal string.

Usage:
    python3 scripts/lifecycle_declared_signer_vector.py          # rewrite the fixture
    python3 scripts/lifecycle_declared_signer_vector.py --check  # exit 1 if it would change

Needs eth_account (pip install eth-account); nothing else, and no network.
"""

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

from eth_account import Account
from eth_account.messages import encode_typed_data
from eth_utils import keccak

FIXTURE = (
    Path(__file__).resolve().parent.parent
    / "tests"
    / "fixtures"
    / "escrow"
    / "lifecycle-declared-signer-vector.json"
)

VECTOR_VERSION = 1
# The `lifecycleSignersVersion` /supported publishes next to `lifecycleSigners`.
ANNOUNCEMENT_VERSION = 1
DOMAIN_NAME = "x402 escrow lifecycle"
DOMAIN_VERSION = "1"
NOW = 1793000000
MAX_DEADLINE_SECS = 900

SEEDS = {
    "declaredSigner": "x402-rs lifecycle vector v1 / declared signer D / TEST KEY, never fund",
    "otherKey": "x402-rs lifecycle vector v1 / other key E / TEST KEY, never fund",
    "payer": "x402-rs lifecycle vector v1 / payer / TEST KEY, never fund",
    "receiver": "x402-rs lifecycle vector v1 / receiver / TEST KEY, never fund",
    "owner": "x402-rs lifecycle vector v1 / operator owner / TEST KEY, never fund",
}

# PaymentOperators exactly as OperatorAddresses::for_network declares them (lowercase).
BASE_A = "0x271f9fa7f8907acf178ccfb470076d9129d8f0eb"
BASE_B = "0x030353642b936c9d4213cad7bcb0fb8a1489cbe5"
ARB_CELO = "0xc2377a9db1de2520bd6b2756ed012f4e82f7938e"  # Arbitrum, Celo, Avalanche, Optimism
ARC_OP = "0x0258472a1410ac3ad720f1bc83f22b3c0af1fd9d"
ETHEREUM_OP = "0x69b67962ffb7c5c7078ff348a87df604dfa8001b"
# paymentInfo.feeReceiver of every case but other-operator: never the operator, so a verifier that
# resolved the declared signer from feeReceiver instead of paymentInfo.operator fails the vector.
FEE_RECEIVER = "0xae07ceb6b395bc685a776a0b4c489e8d9ce9a6ad"

BASE = "eip155:8453"
ARBITRUM = "eip155:42161"
CELO = "eip155:42220"
ARC = "eip155:5042"
ANNOUNCED_NETWORKS = [BASE, ARBITRUM, CELO, ARC]

TOKENS = {
    BASE: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
    ARBITRUM: "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
    CELO: "0xceba9300f2b948710d2653dd7b07f33a8b32118c",
    ARC: "0x3600000000000000000000000000000000000000",
}

PAYMENT_INFO_TYPE = [
    {"name": "operator", "type": "address"},
    {"name": "payer", "type": "address"},
    {"name": "receiver", "type": "address"},
    {"name": "token", "type": "address"},
    {"name": "maxAmount", "type": "uint120"},
    {"name": "preApprovalExpiry", "type": "uint48"},
    {"name": "authorizationExpiry", "type": "uint48"},
    {"name": "refundExpiry", "type": "uint48"},
    {"name": "minFeeBps", "type": "uint16"},
    {"name": "maxFeeBps", "type": "uint16"},
    {"name": "feeReceiver", "type": "address"},
    {"name": "salt", "type": "uint256"},
]
ORDER_TYPE = [
    {"name": "action", "type": "string"},
    {"name": "amount", "type": "uint256"},
    {"name": "deadline", "type": "uint256"},
    {"name": "nonce", "type": "bytes32"},
    {"name": "paymentInfo", "type": "PaymentInfo"},
]


def key(name):
    return Account.from_key(keccak(text=SEEDS[name]))


def address(name):
    return key(name).address.lower()


def chain_id(caip2):
    return int(caip2.split(":", 1)[1])


def tables():
    d, e = address("declaredSigner"), address("otherKey")
    zero = "0x" + "00" * 20
    return {
        "declared": [
            {"network": BASE, "operator": BASE_A, "signer": d},
            {"network": ARBITRUM, "operator": ARB_CELO, "signer": d},
            {"network": ARC, "operator": ARC_OP, "signer": d},
        ],
        "empty": [],
        # D revoked and E declared in its place, by the same pull request.
        "rotated": [{"network": BASE, "operator": BASE_A, "signer": e}],
        # Each row grants nothing: an operator Base does not declare (Ethereum's), and a zero signer.
        "invalid": [
            {"network": BASE, "operator": ETHEREUM_OP, "signer": d},
            {"network": BASE, "operator": BASE_B, "signer": zero},
        ],
        # A rotation that kept the revoked row: the operator is ambiguous, so it has no signer.
        "ambiguous": [
            {"network": BASE, "operator": BASE_A, "signer": d},
            {"network": BASE, "operator": BASE_A, "signer": e},
        ],
    }


def announcement(rows, network):
    """What /supported announces on `network` for `rows`: the valid, unambiguous pairs."""
    payment_operators = {
        BASE: {BASE_A, BASE_B},
        ARBITRUM: {ARB_CELO},
        CELO: {ARB_CELO},
        ARC: {ARC_OP},
    }[network]
    zero = "0x" + "00" * 20
    seen, ambiguous = {}, set()
    for row in rows:
        if row["network"] != network:
            continue
        if row["operator"] not in payment_operators or row["signer"] == zero:
            continue
        if row["operator"] in seen:
            ambiguous.add(row["operator"])
        seen[row["operator"]] = row["signer"]
    signers = {op: s for op, s in sorted(seen.items()) if op not in ambiguous}
    return {"lifecycleSigners": signers, "lifecycleSignersVersion": ANNOUNCEMENT_VERSION}


def payment_info(
    network,
    operator,
    max_amount=1000000,
    authorization_expiry=NOW + 7200,
    fee_receiver=FEE_RECEIVER,
):
    return {
        "operator": operator,
        "payer": address("payer"),
        "receiver": address("receiver"),
        "token": TOKENS[network],
        "maxAmount": str(max_amount),
        "preApprovalExpiry": NOW + 3600,
        "authorizationExpiry": authorization_expiry,
        "refundExpiry": NOW + 30 * 86400,
        "minFeeBps": 0,
        "maxFeeBps": 1300,
        "feeReceiver": fee_receiver,
        "salt": "12345",
    }


def sign(case):
    signed_over = case["signedOver"]
    pi = case["paymentInfo"]
    message = {
        "action": case["action"],
        "amount": int(signed_over["amount"]),
        "deadline": case["deadline"],
        "nonce": bytes.fromhex(case["nonce"]),
        "paymentInfo": {
            **pi,
            "maxAmount": int(pi["maxAmount"]),
            "salt": int(pi["salt"]),
        },
    }
    typed = {
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
            ],
            "LifecycleOrder": ORDER_TYPE,
            "PaymentInfo": PAYMENT_INFO_TYPE,
        },
        "primaryType": "LifecycleOrder",
        "domain": {
            "name": DOMAIN_NAME,
            "version": DOMAIN_VERSION,
            "chainId": signed_over["chainId"],
        },
        "message": message,
    }
    signable = encode_typed_data(full_message=typed)
    signed = Account.sign_message(signable, key(case["signer"]).key)
    if Account.recover_message(signable, signature=signed.signature).lower() != address(case["signer"]):
        raise SystemExit(f"{case['id']}: the signature does not recover to its signer")
    return signed.message_hash.hex().removeprefix("0x"), signed.signature.hex().removeprefix("0x")


def case(
    case_id,
    table,
    network,
    action,
    operator,
    signer,
    expected,
    policy,
    em_delegate,
    owner_rpc="up",
    amount=1000000,
    max_amount=1000000,
    authorization_expiry=NOW + 7200,
    deadline=NOW + 60,
    signed_chain=None,
    signed_amount=None,
    claimed_signer=None,
    fee_receiver=FEE_RECEIVER,
    note="",
):
    c = {
        "id": case_id,
        "note": note,
        "table": table,
        "network": network,
        "action": action,
        "paymentInfo": payment_info(
            network, operator, max_amount, authorization_expiry, fee_receiver
        ),
        "amount": str(amount),
        "deadline": deadline,
        "nonce": keccak(text=f"x402-rs lifecycle vector v1 / nonce / {case_id}").hex(),
        "signer": signer,
        "claimedSigner": claimed_signer or signer,
        "signedOver": {
            "chainId": signed_chain or chain_id(network),
            "amount": str(signed_amount if signed_amount is not None else amount),
        },
        "ownerRpc": owner_rpc,
        "policy": policy,
        "expected": expected,
        "emDelegate": em_delegate,
    }
    c["digest"], c["signature"] = sign(c)
    return c


def ok(role):
    return {"verdict": "ok", "role": role}


def verdict(name):
    return {"verdict": name}


DECLARED = ok("declared_signer")
NEEDS_OWNER = {"step": "needs_owner"}
UNAUTHORIZED = [verdict("unauthorized_role")]


def cases():
    rel, ref = "release", "refundInEscrow"
    return [
        case("declared-release", "declared", BASE, rel, BASE_A, "declaredSigner", [DECLARED],
             DECLARED, True, note="D declared for Base/A releases; the owner is never read"),
        case("declared-refund-before-expiry", "declared", BASE, ref, BASE_A, "declaredSigner",
             [DECLARED], DECLARED, True,
             note="D refunds before authorizationExpiry, which the payer may not"),
        case("declared-release-owner-rpc-down", "declared", BASE, rel, BASE_A, "declaredSigner",
             [DECLARED], DECLARED, True, owner_rpc="down",
             note="same as declared-release with the owner RPC failing: a local check"),
        case("declared-refund-owner-rpc-down", "declared", BASE, ref, BASE_A, "declaredSigner",
             [DECLARED], DECLARED, True, owner_rpc="down"),
        case("declared-arc-refund-capturable", "declared", ARC, ref, ARC_OP, "declaredSigner",
             [DECLARED], DECLARED, True, owner_rpc="down", amount=70000, max_amount=100000,
             note="Arc refundInEscrow signed over the capturable amount (70000) that is submitted"),
        case("other-operator", "declared", BASE, rel, BASE_B, "declaredSigner", UNAUTHORIZED,
             NEEDS_OWNER, False, fee_receiver=BASE_A,
             note="D is declared for Base/A only; this order is for Base/B (its feeReceiver is A)"),
        case("other-chain", "declared", CELO, rel, ARB_CELO, "declaredSigner", UNAUTHORIZED,
             NEEDS_OWNER, False,
             note="same operator address as the Arbitrum declaration, on Celo, which declares none"),
        case("other-key", "declared", BASE, rel, BASE_A, "otherKey", UNAUTHORIZED, NEEDS_OWNER,
             False, note="E signs for Base/A, where D is the declared signer"),
        case("declaration-absent", "empty", BASE, rel, BASE_A, "declaredSigner", UNAUTHORIZED,
             NEEDS_OWNER, False),
        case("declaration-revoked", "rotated", BASE, rel, BASE_A, "declaredSigner", UNAUTHORIZED,
             NEEDS_OWNER, False, note="D was replaced by E"),
        case("rotated-key", "rotated", BASE, ref, BASE_A, "otherKey", [DECLARED], DECLARED, True,
             owner_rpc="down", note="E, declared in D's place, is admitted"),
        case("declaration-invalid-foreign-operator", "invalid", BASE, rel, ETHEREUM_OP,
             "declaredSigner", UNAUTHORIZED, NEEDS_OWNER, False,
             note="the row names an operator Base does not declare"),
        case("declaration-ambiguous", "ambiguous", BASE, rel, BASE_A, "declaredSigner",
             UNAUTHORIZED, NEEDS_OWNER, False, note="two rows for Base/A grant neither key: not D"),
        case("declaration-ambiguous-other-row", "ambiguous", BASE, rel, BASE_A, "otherKey",
             UNAUTHORIZED, NEEDS_OWNER, False, note="nor E, the key of the second row"),
        case("bad-signature-other-chain", "declared", BASE, rel, BASE_A, "declaredSigner",
             [verdict("bad_signature")], verdict("bad_signature"), None,
             signed_chain=chain_id(ARBITRUM), note="D signed the order for chainId 42161"),
        case("bad-signature-other-amount", "declared", BASE, rel, BASE_A, "declaredSigner",
             [verdict("bad_signature")], verdict("bad_signature"), None, amount=999999,
             signed_amount=1000000, note="D signed 1000000; 999999 is submitted"),
        case("bad-signature-arc-capturable", "declared", ARC, ref, ARC_OP, "declaredSigner",
             [verdict("bad_signature")], verdict("bad_signature"), None, amount=70000,
             max_amount=100000, signed_amount=100000,
             note="D signed maxAmount (100000); the capturable 70000 is submitted"),
        case("forged-claim", "declared", BASE, rel, BASE_A, "otherKey",
             [verdict("bad_signature")], verdict("bad_signature"), None,
             claimed_signer="declaredSigner", note="E signs and claims to be D"),
        case("replayed", "declared", BASE, rel, BASE_A, "declaredSigner",
             [DECLARED, verdict("replayed")], DECLARED, True,
             note="submitted twice with the same nonce"),
        case("expired", "declared", BASE, rel, BASE_A, "declaredSigner", [verdict("expired")],
             verdict("expired"), None, deadline=NOW - 1),
        case("deadline-too-far", "declared", BASE, rel, BASE_A, "declaredSigner",
             [verdict("deadline_too_far")], verdict("deadline_too_far"), None,
             deadline=NOW + MAX_DEADLINE_SECS + 1),
        case("payer-release", "declared", BASE, rel, BASE_A, "payer", [ok("payer")], ok("payer"),
             None, note="the roles that existed before declared_signer, unchanged"),
        case("receiver-refund", "declared", BASE, ref, BASE_A, "receiver", [ok("receiver")],
             ok("receiver"), None),
        case("payer-refund-at-expiry", "declared", BASE, ref, BASE_A, "payer",
             [ok("payer_after_expiry")], ok("payer_after_expiry"), None, authorization_expiry=NOW,
             note="the payer refunds in the very second authorizationExpiry is reached"),
        case("receiver-release", "declared", BASE, rel, BASE_A, "receiver", UNAUTHORIZED,
             NEEDS_OWNER, None, note="the receiver still may not release"),
        case("owner-release", "declared", BASE, rel, BASE_A, "owner", [ok("operator_owner")],
             NEEDS_OWNER, None, note="the operator owner is still read from the chain"),
    ]


def build():
    doc = {
        "vector": "x402-rs escrow lifecycle declared_signer policy",
        "vectorVersion": VECTOR_VERSION,
        "testOnly": (
            "Every key in this file is derived from a public seed string. They are test keys:"
            " never fund one, never declare one in the production table."
        ),
        "generator": "scripts/lifecycle_declared_signer_vector.py (eth_account)",
        "encoding": {
            "keyDerivation": "privateKey = keccak256(utf8(seed)); address = its secp256k1 address",
            "addresses": "lowercase, 0x-prefixed, 40 hex digits (what /supported emits)",
            "nonce": "32 bytes as 64 hex digits, no 0x prefix",
            "digest": "EIP-712 digest of what was SIGNED (signedOver), 64 hex digits, no 0x prefix",
            "signature": "65 bytes r||s||v (v = 27 or 28) as 130 hex digits, no 0x prefix",
            "amounts": "decimal strings; salt likewise (uint256)",
        },
        "caseFields": {
            "table": "the declarations in force, a key of `tables`",
            "network": "where the order is submitted (CAIP-2)",
            "amount": "the amount submitted; signedOver holds what was signed",
            "signer": "the key that signs; claimedSigner is what lifecycleAuth.signer says",
            "ownerRpc": (
                "up: the operator owner getter (FEE_RECIPIENT, FEE_RECEIVER on Arc) answers"
                " keys.owner; down: every owner read fails"
            ),
            "policy": (
                "the chain-free verdict ({verdict, role}) or {step: needs_owner} when only the"
                " operator-owner rule is left"
            ),
            "expected": "the verdict of each submission, in order, through the facilitator's gate",
            "emDelegate": (
                "true: the signer must be admitted as declared_signer (Execution Market signs"
                " with it); false: the declared role must not be granted; null: not a"
                " declared-role decision"
            ),
        },
        "eip712": {
            "domain": {"name": DOMAIN_NAME, "version": DOMAIN_VERSION, "chainId": "signedOver.chainId"},
            "primaryType": "LifecycleOrder",
            "types": {"LifecycleOrder": ORDER_TYPE, "PaymentInfo": PAYMENT_INFO_TYPE},
        },
        "announcement": {
            "where": (
                "GET /supported of the facilitator that settles, every x402r (EVM) kind with"
                " scheme \"escrow\" on the network, under its CAIP-2 id and under its v1 name:"
                " extra.lifecycleSigners and extra.lifecycleSignersVersion"
            ),
            "lifecycleSigners": (
                "object, operator -> declared signer, every declared operator of that network;"
                " {} when none is declared. Every escrow kind of a network carries the same map"
            ),
            "lifecycleSignersVersion": ANNOUNCEMENT_VERSION,
            "absent": "both fields absent: a facilitator older than the announcement (no delegate)",
            "otherKinds": (
                "absent from every other kind: commerce, exact, upto, and the NEAR escrow kind"
            ),
            "readerRule": (
                "no lifecycleSignersVersion, a version other than 1, a network with no escrow kind,"
                " or an operator missing from the map: no declared signer. A permission is never"
                " inferred from an absence, and never read from a payment requirement, POST"
                " /accepts or a request payload"
            ),
        },
        "now": NOW,
        "maxDeadlineSecs": MAX_DEADLINE_SECS,
        "keys": {
            name: {"seed": seed, "address": address(name)} for name, seed in SEEDS.items()
        },
        "tables": tables(),
        "announcements": {
            name: {net: announcement(rows, net) for net in ANNOUNCED_NETWORKS}
            for name, rows in tables().items()
        },
        "cases": cases(),
    }
    return json.dumps(doc, indent=2, ensure_ascii=True) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="exit 1 if the fixture would change")
    args = parser.parse_args()
    text = build()
    if re.search(r"0x[0-9a-fA-F]{64}", text):
        raise SystemExit("a value came out as 0x + 64 hex digits; the pre-commit hook refuses it")
    if args.check:
        current = FIXTURE.read_text(encoding="utf-8") if FIXTURE.exists() else ""
        if current.replace("\r\n", "\n") != text:
            print(f"{FIXTURE} is out of date; run without --check", file=sys.stderr)
            return 1
        print(f"{FIXTURE} is up to date")
        return 0
    FIXTURE.write_bytes(text.encode("utf-8"))
    print(f"wrote {FIXTURE} sha256={hashlib.sha256(text.encode('utf-8')).hexdigest()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
