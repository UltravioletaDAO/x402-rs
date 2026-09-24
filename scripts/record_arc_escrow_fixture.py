#!/usr/bin/env python3
"""record_arc_escrow_fixture.py -- record what Arc answers about its escrow set.

Writes tests/fixtures/escrow/arc-generation-d-chain.json, which the Rust tests
`every_announced_arc_address_has_code` and
`arc_operator_address_matches_recorded_compute_address` read. Nothing in that
file is typed by hand: every value is a JSON-RPC answer, and every address
asked about is read out of src/payment_operator/addresses.rs, so the fixture
covers exactly what the code declares.

Per network (Arc 5042, Arc testnet 5042002), pinned to one block:
  * eth_chainId, eth_blockNumber
  * eth_getCode of every `canonical_v1` address, of every PaymentOperator the
    code lists for the network, and of the CREATE3 set (expected empty there)
  * eth_call ESCROW() of every listed PaymentOperator that has code (an
    operator not deployed yet has none, and is not asked)
  * eth_call PaymentOperatorFactory.computeAddress(<the declared config>)
  * eth_call AuthCaptureEscrow.getHash(<a fixed probe payment, payer zeroed>),
    the ERC-3009 nonce the canonical collector derives; the Rust side checks
    its own computation of that nonce against this answer

Reads are sequential with a pause between them, and the run stops at the first
HTTP 429 or JSON-RPC error without writing anything: a partial fixture would
pass for a complete one.

Bytecode and call data are stored as hex WITHOUT a 0x prefix (the pre-commit
hook reads a 0x-prefixed 64-digit run as a private key). Needs `cast`
(Foundry) for the ABI encoding, which keeps it independent of the Rust
encoder the test checks it against.

Usage:
  python3 scripts/record_arc_escrow_fixture.py            # write the fixture
  python3 scripts/record_arc_escrow_fixture.py --dry-run  # print, write nothing
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ADDRESSES_RS = REPO / "src" / "payment_operator" / "addresses.rs"
FIXTURE = REPO / "tests" / "fixtures" / "escrow" / "arc-generation-d-chain.json"

NETWORKS = {
    "arc": {"chain_id": 5042, "rpc": "https://rpc.mainnet.arc.io", "variant": "Arc"},
    "arc-testnet": {
        "chain_id": 5042002,
        "rpc": "https://rpc.testnet.arc.network",
        "variant": "ArcTestnet",
    },
}

PAUSE_SECS = 1.4
COMPUTE_ADDRESS_SIG = "computeAddress((" + ",".join(["address"] * 12) + "))"
GET_HASH_SIG = (
    "getHash((address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256))"
)
# A payment in the shape the collector hashes: payer zeroed, everything else
# arbitrary but fixed. Arc USDC as the token.
PROBE = (
    "(0x0258472A1410Ac3Ad720f1BC83f22B3c0af1Fd9D,0x0000000000000000000000000000000000000000,"
    "0x2222222222222222222222222222222222222222,0x3600000000000000000000000000000000000000,"
    "1000000,1900000000,1900086400,1902678400,0,1300,0x0258472A1410Ac3Ad720f1BC83f22B3c0af1Fd9D,12345)"
)


class Stop(Exception):
    """Abort the whole recording."""


def module_consts(src: str, module: str) -> dict[str, str]:
    m = re.search(r"pub mod " + module + r" \{(.*?)\n\}", src, re.S)
    if not m:
        raise Stop(f"no `pub mod {module}` in {ADDRESSES_RS}")
    return dict(
        re.findall(r"pub const ([A-Z0-9_]+): Address =\s*address!\(\"([0-9a-fA-F]{40})\"\)", m.group(1))
    )


def declared(src: str, variant: str) -> tuple[list[str], list[str]]:
    """(payment operators, factory config) the code declares for a network."""
    arm = r"(?:Network::\w+\s*\|\s*)*Network::" + variant + r"\b(?:\s*\|\s*Network::\w+)*"
    m = re.search(arm + r"\s*=> Some\(Self \{(.*?)\}\),", src, re.S)
    if not m:
        raise Stop(f"no OperatorAddresses::for_network arm for Network::{variant}")
    body = m.group(1)
    ops = re.search(r"payment_operators: vec!\[(.*?)\]", body, re.S)
    operators = re.findall(r"address!\(\"([0-9a-fA-F]{40})\"\)", ops.group(1) if ops else "")
    cfg = re.search(r"pub const ARC_EM_OPERATOR_CONFIG: \[Address; 12\] = \[(.*?)\];", src, re.S)
    if not cfg:
        raise Stop("no ARC_EM_OPERATOR_CONFIG in addresses.rs")
    entries = re.findall(r"address!\(\"([0-9a-fA-F]{40})\"\)|Address::ZERO", cfg.group(1))
    config = [e or "0" * 40 for e in entries]
    if len(config) != 12:
        raise Stop(f"ARC_EM_OPERATOR_CONFIG has {len(config)} entries, expected 12")
    return operators, config


def checksum(addr: str) -> str:
    return subprocess.check_output(["cast", "to-check-sum-address", "0x" + addr.removeprefix("0x")], text=True).strip()


def rpc(url: str, method: str, params: list) -> str:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json", "User-Agent": "uvd-x402-fixture-recorder"}
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            answer = json.loads(resp.read())
    except urllib.error.HTTPError as e:
        raise Stop(f"{method}: HTTP {e.code} from {url} -- stopping") from e
    except (urllib.error.URLError, TimeoutError) as e:
        raise Stop(f"{method}: {e} from {url} -- stopping") from e
    finally:
        time.sleep(PAUSE_SECS)
    if "error" in answer:
        raise Stop(f"{method}: JSON-RPC error {answer['error']} from {url} -- stopping")
    return answer["result"]


def strip0x(hexstr: str) -> str:
    return hexstr[2:] if hexstr.startswith("0x") else hexstr


def record_network(name: str, net: dict, canonical: dict[str, str], create3: dict[str, str], src: str) -> dict:
    url = net["rpc"]
    operators, config = declared(src, net["variant"])
    chain_id = rpc(url, "eth_chainId", [])
    if int(chain_id, 16) != net["chain_id"]:
        raise Stop(f"{url} answered chain id {int(chain_id, 16)}, expected {net['chain_id']}")
    block = rpc(url, "eth_blockNumber", [])

    code: dict[str, str] = {}
    for addr in list(canonical.values()) + operators:
        key = checksum(addr)
        code[key] = strip0x(rpc(url, "eth_getCode", [key, block]))
    operator_escrow: dict[str, str] = {}
    for addr in operators:
        key = checksum(addr)
        if code[key]:
            escrow_sel = subprocess.check_output(["cast", "sig", "ESCROW()"], text=True).strip()
            operator_escrow[key] = strip0x(rpc(url, "eth_call", [{"to": key, "data": escrow_sel}, block]))
    absent: dict[str, str] = {}
    for addr in create3.values():
        key = checksum(addr)
        absent[key] = strip0x(rpc(url, "eth_getCode", [key, block]))

    factory = checksum(canonical["FACTORY_PAYMENT_OPERATOR"])
    tuple_arg = "(" + ",".join(checksum(a) for a in config) + ")"
    calldata = subprocess.check_output(["cast", "calldata", COMPUTE_ADDRESS_SIG, tuple_arg], text=True).strip()
    result = rpc(url, "eth_call", [{"to": factory, "data": calldata}, block])

    escrow = checksum(canonical["ESCROW"])
    probe_data = subprocess.check_output(["cast", "calldata", GET_HASH_SIG, PROBE], text=True).strip()
    probe_hash = rpc(url, "eth_call", [{"to": escrow, "data": probe_data}, block])

    return {
        "rpc": url,
        "recordedAt": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "chainId": chain_id,
        "block": block,
        "code": code,
        "operatorEscrow": operator_escrow,
        "create3Code": absent,
        "computeAddress": {"factory": factory, "calldata": strip0x(calldata), "result": strip0x(result)},
        "getHashProbe": {"escrow": escrow, "calldata": strip0x(probe_data), "result": strip0x(probe_hash)},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--dry-run", action="store_true", help="print the summary, write nothing")
    args = ap.parse_args()

    src = ADDRESSES_RS.read_text(encoding="utf-8")
    try:
        canonical = module_consts(src, "canonical_v1")
        create3 = {k: v for k, v in module_consts(src, "create3").items() if k in ("ESCROW", "TOKEN_COLLECTOR", "FACTORY_PAYMENT_OPERATOR")}
        doc = {
            "_about": "Recorded by scripts/record_arc_escrow_fixture.py from the RPCs named per network, "
            "for the addresses src/payment_operator/addresses.rs declares. JSON-RPC answers verbatim, "
            "hex without 0x. Do not edit by hand: re-run the script.",
            "networks": {},
        }
        for name, net in NETWORKS.items():
            doc["networks"][name] = record_network(name, net, canonical, create3, src)
    except Stop as e:
        print(f"[FAIL] {e}", file=sys.stderr)
        return 1

    for name, net in doc["networks"].items():
        print(f"{name}: chainId={int(net['chainId'], 16)} block={int(net['block'], 16)}")
        for addr, c in net["code"].items():
            print(f"  code {addr}: {len(c) // 2} bytes")
        for addr, c in net["create3Code"].items():
            print(f"  create3 {addr}: {len(c) // 2} bytes")
        print(f"  computeAddress -> 0x{net['computeAddress']['result'][-40:]}")
        print(f"  getHash(probe) -> {net['getHashProbe']['result'][:8]}...{net['getHashProbe']['result'][-8:]}")
    if args.dry_run:
        return 0
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"[OK] wrote {FIXTURE.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
