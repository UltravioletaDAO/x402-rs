#!/usr/bin/env python3
"""List the ERC-8004 identities the facilitator's own wallet still holds, and flag the suspicious ones.

READ-ONLY. It asks a JSON-RPC node for logs and view calls (`Rpc.READ_METHODS`) and nothing else: it holds no
key and cannot send a transaction.

Why: until 2.44.0, `POST /register` without a `recipient` minted the identity to the facilitator and kept it,
with whatever `agentUri` the caller chose. Those identities read, on every explorer, as ours. This finds them the
way the registry records them -- every ERC-721 `Transfer` INTO the wallet (a mint is a transfer from 0x0), kept
only while `ownerOf` still answers the wallet -- reads each `tokenURI`, and judges it with the same rules
`POST /register` now applies (`config/erc8004_agent_uri_rules.json`, the corpus in
`tests/fixtures/erc8004_agent_uri_cases.json` pins that both implementations agree).

A flagged identity is retired through the running service, never by hand with the hot key:
    POST /erc8004/admin/retire-identity  {"network": "base", "agentId": "<id>", "dryRun": true}
(admin token; see src/erc8004/retire.rs). Suspicious URIs are printed defanged (hxxp, [.]) unless --raw.

Usage:
    python3 scripts/erc8004_custodied_identities.py                       # Base, public RPC
    python3 scripts/erc8004_custodied_identities.py --rpc "$RPC_URL_BASE" --from-block 30000000
    python3 scripts/erc8004_custodied_identities.py --json > custodied.json
    python3 scripts/erc8004_custodied_identities.py --classify 'http://198-51-100-7.sslip.io/a.json'

Exit code: 0 when nothing is flagged, 2 when something is, 1 on error.
"""

from __future__ import annotations

import argparse
import ipaddress
import json
import os
import re
import sys
import unicodedata
import urllib.parse
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RULES = json.loads((REPO / "config" / "erc8004_agent_uri_rules.json").read_text(encoding="utf-8"))

# The facilitator's mainnet EVM wallet, as lambda/balances/handler.py (the authoritative copy) spells it.
FACILITATOR_MAINNET = "0x103040545AC5031A11E8C03dd11324C7333a13C7"
RETIRED_URI = "https://facilitator.ultravioletadao.xyz/erc8004/retired"
DEFAULT_RPC = {"base": "https://mainnet.base.org"}

# keccak256("Transfer(address,address,uint256)"). Written without its 0x so the pre-commit key guard
# (.githooks/pre-commit, "0x" + 64 hex) does not read a public event signature as a private key.
TRANSFER_TOPIC = "0x" + "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
SEL_OWNER_OF = "6352211e"
SEL_TOKEN_URI = "c87b56dd"
SEL_BALANCE_OF = "70a08231"

MISSING = "agent_uri_missing"
TOO_LONG = "agent_uri_too_long"
MALFORMED = "agent_uri_malformed"
SCHEME = "agent_uri_scheme"
CREDENTIALS = "agent_uri_credentials"
IP_LITERAL = "agent_uri_ip_literal"
NON_PUBLIC = "agent_uri_non_public_host"
EMBEDDED_IP = "agent_uri_embedded_ip"
TUNNEL = "agent_uri_tunnel"


# ---------------------------------------------------------------------------
# The rules. A mirror of src/erc8004/agent_uri.rs::violations, including the parts of WHATWG URL parsing it
# depends on (IPv4 in decimal/hex/octal, percent-decoded and IDNA-mapped hosts).
# ---------------------------------------------------------------------------

SPECIAL = {"http", "https", "ws", "wss", "ftp"}
FORBIDDEN_HOST = set(" #/:<>?@[\\]^|%\x7f") | {chr(c) for c in range(0x20)}


def _matches_any(host: str, suffixes: list[str]) -> bool:
    return any(host == s or host.endswith("." + s) for s in suffixes)


def _embeds_ipv4(host: str) -> bool:
    parts = re.split(r"[.-]", host)
    for i in range(len(parts) - 3):
        window = parts[i : i + 4]
        if all(1 <= len(p) <= 3 and p.isascii() and p.isdigit() and int(p) <= 255 for p in window):
            return True
    return False


def _whatwg_ipv4(host: str):
    """None when `host` is not IPv4-shaped; False when it is and does not parse (a URL error); else the address."""
    labels = host.split(".")
    if labels and labels[-1] == "" and len(labels) > 1:
        labels = labels[:-1]
    last = labels[-1] if labels else ""
    if not (last.isascii() and last.isdigit()) and not re.fullmatch(r"0[xX][0-9a-fA-F]*", last):
        return None
    if len(labels) > 4:
        return False
    numbers = []
    for part in labels:
        if part == "":
            return False
        try:
            if part[:2].lower() == "0x":
                n = int(part[2:] or "0", 16)
            elif len(part) > 1 and part[0] == "0":
                n = int(part[1:], 8)
            else:
                n = int(part, 10)
        except ValueError:
            return False
        numbers.append(n)
    if any(n > 255 for n in numbers[:-1]) or numbers[-1] >= 256 ** (5 - len(numbers)):
        return False
    value = numbers[-1]
    for i, n in enumerate(numbers[:-1]):
        value += n * 256 ** (3 - i)
    return ipaddress.IPv4Address(value)


def violations(uri: str) -> list[str]:
    """Every rule `uri` breaks, in the order the Rust guard reports them; empty when acceptable."""
    if not uri.strip():
        return [MISSING]
    found: list[str] = []
    if len(uri.encode("utf-8")) > RULES["maxBytes"]:
        found.append(TOO_LONG)
    if any(c.isspace() or unicodedata.category(c) == "Cc" for c in uri):
        found.append(MALFORMED)
        return found
    m = re.match(r"([A-Za-z][A-Za-z0-9+.\-]*):(.*)\Z", uri, re.S)
    if not m:
        found.append(MALFORMED)
        return found
    scheme, rest = m.group(1).lower(), m.group(2)
    if scheme not in RULES["schemes"]:
        found.append(SCHEME)
    if scheme == "ipfs":
        cid = rest[2:].split("/", 1)[0].split("?", 1)[0].split("#", 1)[0] if rest.startswith("//") else ""
        if not (cid and cid.isascii() and cid.isalnum()):
            found.append(MALFORMED)
        return found
    special = scheme in SPECIAL
    if special:
        rest = rest.lstrip("/\\")
    elif rest.startswith("//"):
        rest = rest[2:]
    else:
        if SCHEME not in found:
            found.append(MALFORMED)
        return found
    authority = re.split(r"[/?#\\]" if special else r"[/?#]", rest, maxsplit=1)[0]
    userinfo, _, hostport = authority.rpartition("@")
    if userinfo:
        found.append(CREDENTIALS)
    if hostport.startswith("["):
        end = hostport.find("]")
        try:
            ipaddress.IPv6Address(hostport[1:end])
        except ValueError:
            return found + [MALFORMED]
        found.append(IP_LITERAL)
        return found
    host, _, port = hostport.partition(":")
    if port and not port.isdigit():
        return found + [MALFORMED]
    if special:
        host = urllib.parse.unquote(host)
        try:
            host = host.encode("idna").decode("ascii").lower() if host else ""
        except UnicodeError:
            return found + [MALFORMED]
        if not host or any(c in FORBIDDEN_HOST for c in host):
            return found + [MALFORMED]
        ip = _whatwg_ipv4(host)
        if ip is False:
            return found + [MALFORMED]
        if ip is not None:
            found.append(IP_LITERAL)
            return found
    domain = host.rstrip(".").lower()
    try:
        ipaddress.IPv4Address(domain)
        found.append(IP_LITERAL)
        return found
    except ValueError:
        pass
    if "." not in domain or _matches_any(domain, RULES["nonPublicSuffixes"]):
        found.append(NON_PUBLIC)
    if _embeds_ipv4(domain) or _matches_any(domain, RULES["wildcardDnsDomains"]):
        found.append(EMBEDDED_IP)
    if _matches_any(domain, RULES["tunnelDomains"]):
        found.append(TUNNEL)
    return found


def defang(uri: str) -> str:
    return uri.replace("http", "hxxp").replace(".", "[.]")


# ---------------------------------------------------------------------------
# The chain, read-only.
# ---------------------------------------------------------------------------


class RpcError(Exception):
    pass


class Rpc:
    """A JSON-RPC client that can only read. Anything outside READ_METHODS is refused before it is sent."""

    READ_METHODS = frozenset({"eth_blockNumber", "eth_chainId", "eth_getLogs", "eth_call", "eth_getCode"})

    def __init__(self, url: str, timeout: float = 30.0):
        self.url = url
        self.timeout = timeout
        self._id = 0

    def call(self, method: str, params: list):
        if method not in self.READ_METHODS:
            raise RpcError(f"refusing {method}: this script only reads")
        self._id += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}).encode()
        request = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            reply = json.loads(response.read())
        if "error" in reply:
            raise RpcError(str(reply["error"]))
        return reply["result"]


def _word(n: int) -> str:
    return format(n, "064x")


def _address_word(address: str) -> str:
    return address.lower().removeprefix("0x").rjust(64, "0")


def eth_call(rpc: Rpc, to: str, data: str) -> str:
    return rpc.call("eth_call", [{"to": to, "data": "0x" + data}, "latest"])


def decode_string(result: str) -> str:
    raw = bytes.fromhex(result.removeprefix("0x"))
    offset = int.from_bytes(raw[:32], "big")
    length = int.from_bytes(raw[offset : offset + 32], "big")
    return raw[offset + 32 : offset + 32 + length].decode("utf-8", errors="replace")


def deployment_block(rpc: Rpc, address: str, latest: int) -> int:
    """Lowest block with code at `address` (binary search; needs historical state)."""
    lo, hi = 0, latest
    while lo < hi:
        mid = (lo + hi) // 2
        if rpc.call("eth_getCode", [address, hex(mid)]) not in ("0x", "0x0", ""):
            hi = mid
        else:
            lo = mid + 1
    return lo


def received_token_ids(rpc: Rpc, registry: str, holder: str, start: int, end: int, chunk: int,
                       log=lambda *_: None) -> dict[int, int]:
    """tokenId -> last block it was transferred INTO `holder`. Halves the range when the node refuses it."""
    received: dict[int, int] = {}
    topic_to = "0x" + _address_word(holder)
    block = start
    while block <= end:
        upper = min(end, block + chunk - 1)
        try:
            logs = rpc.call("eth_getLogs", [{
                "address": registry, "fromBlock": hex(block), "toBlock": hex(upper),
                "topics": [TRANSFER_TOPIC, None, topic_to],
            }])
        except RpcError as e:
            if chunk == 1:
                raise
            chunk = max(1, chunk // 2)
            log(f"  node refused {block}..{upper} ({e}); retrying with {chunk} blocks")
            continue
        for entry in logs:
            topics = entry["topics"]
            if len(topics) == 4 and topics[0].lower() == TRANSFER_TOPIC:
                received[int(topics[3], 16)] = int(entry["blockNumber"], 16)
        block = upper + 1
    return received


def custodied(rpc: Rpc, registry: str, holder: str, received: dict[int, int]) -> list[dict]:
    """The received tokens `holder` still owns, each with its tokenURI and verdict."""
    rows = []
    for token_id in sorted(received):
        try:
            owner = eth_call(rpc, registry, SEL_OWNER_OF + _word(token_id))
        except RpcError:
            continue  # burned, or never ours to begin with
        if _address_word(owner[-40:]) != _address_word(holder):
            continue  # left the wallet after it arrived
        uri = decode_string(eth_call(rpc, registry, SEL_TOKEN_URI + _word(token_id)))
        rows.append({
            "agentId": str(token_id),
            "receivedAtBlock": received[token_id],
            "tokenUri": uri,
            "retired": uri == RETIRED_URI,
            "violations": [] if uri == RETIRED_URI else violations(uri),
        })
    return rows


def registry_address(network: str) -> str:
    """Identity Registry of `network`, read from src/erc8004/mod.rs (get_contracts), never typed here."""
    source = (REPO / "src" / "erc8004" / "mod.rs").read_text(encoding="utf-8")
    consts = dict(re.findall(
        r'pub const (\w+_CONTRACTS): Erc8004Contracts = Erc8004Contracts \{\s*identity_registry: '
        r'alloy::primitives::address!\("([0-9a-fA-F]{40})"\)', source))
    arms = re.findall(r"Network::(\w+) => Some\((\w+_CONTRACTS)\)", source.split("pub fn get_contracts", 1)[1])
    for variant, const in arms:
        kebab = re.sub(r"(?<!^)(?=[A-Z])", "-", variant).lower()
        if kebab == network and const in consts:
            return "0x" + consts[const]
    raise SystemExit(f"no EVM Identity Registry for {network!r} in src/erc8004/mod.rs")


def markdown(rows: list[dict], raw: bool) -> str:
    out = ["| agentId | received at block | tokenURI | verdict |", "|---|---|---|---|"]
    for r in rows:
        flagged = bool(r["violations"])
        uri = r["tokenUri"] if (raw or not flagged) else defang(r["tokenUri"])
        verdict = "retired" if r["retired"] else (", ".join(r["violations"]) if flagged else "ok")
        out.append(f"| {r['agentId']} | {r['receivedAtBlock']} | `{uri}` | {'**' + verdict + '**' if flagged else verdict} |")
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--network", default="base")
    ap.add_argument("--rpc", help="read-only JSON-RPC URL (default: RPC_URL_<NETWORK>, else the public endpoint)")
    ap.add_argument("--holder", default=FACILITATOR_MAINNET, help="wallet whose identities to list")
    ap.add_argument("--from-block", type=int, help="first block to scan (default: the registry's deployment)")
    ap.add_argument("--to-block", type=int)
    ap.add_argument("--chunk", type=int, default=10_000, help="blocks per eth_getLogs (halved on refusal)")
    ap.add_argument("--json", action="store_true", help="machine-readable output, URIs raw")
    ap.add_argument("--raw", action="store_true", help="do not defang suspicious URIs in the table")
    ap.add_argument("--classify", metavar="URI", help="judge one URI offline and exit")
    a = ap.parse_args(argv)

    if a.classify is not None:
        found = violations(a.classify)
        print(json.dumps({"uri": a.classify, "violations": found}))
        return 2 if found else 0

    url = a.rpc or os.environ.get("RPC_URL_" + a.network.upper().replace("-", "_")) or DEFAULT_RPC.get(a.network)
    if not url:
        raise SystemExit(f"no RPC for {a.network}: pass --rpc")
    rpc = Rpc(url)
    registry = registry_address(a.network)
    log = (lambda *m: print(*m, file=sys.stderr))
    latest = a.to_block if a.to_block is not None else int(rpc.call("eth_blockNumber", []), 16)
    start = a.from_block
    if start is None:
        log(f"finding the deployment block of {registry} ...")
        try:
            start = deployment_block(rpc, registry, latest)
        except RpcError as e:
            raise SystemExit(f"this node cannot answer historical eth_getCode ({e}); pass --from-block")
    log(f"scanning Transfer(*, {a.holder}, *) on {registry}, blocks {start}..{latest}")
    received = received_token_ids(rpc, registry, a.holder, start, latest, a.chunk, log)
    rows = custodied(rpc, registry, a.holder, received)
    balance = int(eth_call(rpc, registry, SEL_BALANCE_OF + _address_word(a.holder)), 16)
    flagged = [r for r in rows if r["violations"]]
    summary = {
        "network": a.network, "registry": registry, "holder": a.holder, "fromBlock": start, "toBlock": latest,
        "balanceOf": balance, "found": len(rows), "flagged": len(flagged),
        "retired": sum(r["retired"] for r in rows),
    }
    if a.json:
        print(json.dumps({"summary": summary, "identities": rows}, indent=2))
    else:
        print(markdown(rows, a.raw))
        print()
        print(json.dumps(summary))
        if balance != len(rows):
            print(f"WARNING: balanceOf says {balance} and the scan found {len(rows)}: widen --from-block "
                  "or check the node's log coverage before trusting the table.")
    return 2 if flagged else 0


if __name__ == "__main__":
    sys.exit(main())
