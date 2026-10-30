#!/usr/bin/env python3
"""verify_eip712_domains.py -- read-only check of every static EIP-712 domain.

For each EVM token deployment in src/network.rs that declares a static
EIP-712 domain (`eip712: Some(TokenDeploymentEip712 { name, version })`), read
`name()`, `version()`, `eip712Domain()` (EIP-5267) and `DOMAIN_SEPARATOR()` off
the token contract and compare them with what the facilitator declares.

Why the separator decides, not name(): the separator is the one number a
payer's signature actually commits to. Recomputed from the declared
(name, version, chain id, address) it must equal what the contract publishes,
or every signature built from our table is refused by the contract. name() is
not the domain name everywhere -- AUSD answers "AUSD" and signs under
"Agora Dollar" -- some tokens revert on version() (USDT0, USDG), and a name
can differ by one Unicode character that no terminal shows (USD\\u20ae0 vs
USDT0 on Monad, 2.52.1). Without a DOMAIN_SEPARATOR() the script falls back to
eip712Domain(), then to name()/version(), and says so in the row.

Where things come from -- all parsed from the repository, nothing typed here:
  * deployments and their domains  -> src/network.rs (`static ...: Lazy<...Deployment>`)
  * chain ids                      -> src/network.rs (`Network::to_caip2`, eip155 arms)
  * RPC env var per network        -> src/from_env.rs (`rpc_env_name_from_network`)
  * RPC URL                        -> that env var if set; else the public URL in the
                                      ECS task definition (terraform/environments/
                                      production/main.tf); else .env.example
                                      (commented-out lines included). Networks whose
                                      production RPC is a secret fall through to
                                      .env.example's public endpoint.

Only `eth_chainId`, `eth_blockNumber` and `eth_call` are sent. Nothing is
signed or broadcast. A network whose RPC does not answer, or answers another
chain id, is reported UNMEASURED: no verdict is guessed for it.

This script uses the network and is NOT part of CI.

Usage
-----
  python3 scripts/verify_eip712_domains.py            # text table
  python3 scripts/verify_eip712_domains.py --md       # Markdown table
  python3 scripts/verify_eip712_domains.py --json     # JSON rows
  RPC_URL_MONAD=https://... python3 scripts/verify_eip712_domains.py

Needs a keccak-256: pycryptodome (`pip install pycryptodome`) or eth-hash.

Exit code: 1 if any measured row MISMATCHes, else 0 (UNMEASURED rows do not
fail the run; they are listed).
"""

import argparse
import json
import os
import re
import sys
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NETWORK_RS = ROOT / "src" / "network.rs"
FROM_ENV_RS = ROOT / "src" / "from_env.rs"
MAIN_TF = ROOT / "terraform" / "environments" / "production" / "main.tf"
ENV_EXAMPLE = ROOT / ".env.example"

SEL_NAME = "0x06fdde03"  # name()
SEL_VERSION = "0x54fd4d50"  # version()
SEL_DOMAIN_SEPARATOR = "0x3644e515"  # DOMAIN_SEPARATOR()
SEL_EIP712_DOMAIN = "0x84b0196e"  # eip712Domain(), EIP-5267
DOMAIN_TYPE = "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"


def _keccak_impl():
    try:
        from Crypto.Hash import keccak as _k

        def k(data: bytes) -> bytes:
            return _k.new(digest_bits=256, data=data).digest()

        return k
    except ImportError:
        pass
    try:
        from eth_hash.auto import keccak as k

        return k
    except ImportError:
        sys.exit("ERROR: needs a keccak-256 (pip install pycryptodome, or eth-hash)")


keccak = _keccak_impl()
# hashlib.sha3_256 is NOT keccak-256; refuse to run on a library that confuses them.
if keccak(b"").hex() != "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470":
    sys.exit("ERROR: the keccak implementation does not produce keccak-256")


# ---------------------------------------------------------------------------
# Parsing the Rust sources
# ---------------------------------------------------------------------------

_SIMPLE_ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}


def rust_unescape(s: str) -> str:
    """Decode the escapes a Rust string literal may carry (\\u{20AE}, \\x41, \\\\, ...)."""
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        n = s[i + 1]
        if n == "u":
            end = s.index("}", i)
            out.append(chr(int(s[i + 3 : end], 16)))
            i = end + 1
        elif n == "x":
            out.append(chr(int(s[i + 2 : i + 4], 16)))
            i += 4
        else:
            out.append(_SIMPLE_ESCAPES[n])
            i += 2
    return "".join(out)


def balanced(text: str, open_at: int) -> str:
    """The text from the '(' at `open_at` to its matching ')', skipping strings and // comments."""
    depth, i, in_str = 0, open_at, False
    while i < len(text):
        c = text[i]
        if in_str:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                in_str = False
        elif c == '"':
            in_str = True
        elif c == "/" and text.startswith("//", i):
            i = text.index("\n", i)
            continue
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return text[open_at : i + 1]
        i += 1
    raise ValueError(f"unbalanced parenthesis at offset {open_at}")


_STR = r'"((?:[^"\\]|\\.)*)"'


def parse_deployments(src: str):
    """Every EVM deployment that declares a static EIP-712 domain, plus the EVM ones that do not."""
    with_domain, without_domain = [], []
    for m in re.finditer(r"^static (\w+): Lazy<(\w+)Deployment> = Lazy::new\(", src, re.M):
        body = balanced(src, m.end() - 1)
        addr = re.search(r'address!\(\s*"(?:0x)?([0-9a-fA-F]{40})"\s*\)', body)
        if not addr:
            continue  # not an EVM deployment (Solana pubkey, NEAR account, ...)
        net = re.search(r"network:\s*Network::(\w+)", body)
        if not net:
            raise ValueError(f"{m.group(1)}: no network")
        row = {
            "static": m.group(1),
            "token": m.group(2),
            "network": net.group(1),
            "address": "0x" + addr.group(1),
        }
        some = re.search(r"eip712:\s*Some\(", body)
        if not some:
            without_domain.append(row)
            continue
        domain = balanced(body, some.end() - 1)
        name = re.search(r"name:\s*" + _STR, domain)
        version = re.search(r"version:\s*" + _STR, domain)
        if not (name and version):
            raise ValueError(f"{m.group(1)}: eip712 without a literal name and version")
        row["name"] = rust_unescape(name.group(1))
        row["version"] = rust_unescape(version.group(1))
        with_domain.append(row)
    # A deployment this parser skipped would vanish from the table without a word.
    declared = len(re.findall(r"eip712:\s*Some\(", src))
    if declared != len(with_domain):
        raise ValueError(f"src/network.rs declares {declared} static domains, parsed {len(with_domain)}")
    return with_domain, without_domain


def parse_chain_ids(src: str):
    ids = {}
    for variant, chain_id in re.findall(r'Network::(\w+)\s*=>\s*"eip155:(\d+)"', src):
        if ids.setdefault(variant, int(chain_id)) != int(chain_id):
            raise ValueError(f"Network::{variant} maps to two chain ids")
    return ids


def parse_rpc_env_names(src: str):
    consts = dict(re.findall(r'pub const (ENV_RPC_\w+): &str = "([A-Z0-9_]+)";', src))
    fn = src[src.index("pub fn rpc_env_name_from_network") :]
    fn = fn[: fn.index("\n}\n")]
    names = {}
    for variant, target in re.findall(r"Network::(\w+)\s*=>\s*(ENV_RPC_\w+|\"[A-Z0-9_]+\")", fn):
        names[variant] = target.strip('"') if target.startswith('"') else consts[target]
    return names


def parse_public_rpcs():
    """RPC env var -> public URL: the task definition's `environment` first, then .env.example."""
    urls = {}
    for var, url in re.findall(
        r'name\s*=\s*"(RPC_URL_\w+)"\s*\n\s*value\s*=\s*"(https?://[^"$]+)"', MAIN_TF.read_text("utf-8")
    ):
        urls.setdefault(var, (url, "main.tf"))
    for var, url in re.findall(r"^#?\s*(RPC_URL_\w+)=(https?://\S+)", ENV_EXAMPLE.read_text("utf-8"), re.M):
        urls.setdefault(var, (url, ".env.example"))
    return urls


# ---------------------------------------------------------------------------
# JSON-RPC (read-only)
# ---------------------------------------------------------------------------


class RpcError(Exception):
    pass


def rpc(url: str, method: str, params, timeout: float):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(
        url,
        data=body,
        headers={"content-type": "application/json", "user-agent": "x402-rs-verify-eip712-domains/1"},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            answer = json.loads(resp.read())
    except Exception as e:  # network, HTTP or JSON: no answer, no verdict
        raise RpcError(f"{type(e).__name__}: {e}") from None
    if "error" in answer:
        raise RpcError(str(answer["error"].get("message", answer["error"]))[:120])
    return answer.get("result")


REVERTS, UNDECODABLE = "(reverts)", "(undecodable)"


def call(url, to, data, block, timeout):
    """eth_call; None when the call reverts or returns nothing."""
    try:
        result = rpc(url, "eth_call", [{"to": to, "data": data}, block], timeout)
    except RpcError as e:
        if "revert" in str(e).lower():
            return None
        raise
    if not result or result == "0x":
        return None
    return bytes.fromhex(result[2:])


def read(url, to, selector, block, timeout, decode):
    """One getter, decoded; REVERTS or UNDECODABLE instead of a value it does not have."""
    raw = call(url, to, selector, block, timeout)
    if raw is None:
        return REVERTS
    try:
        return decode(raw)
    except (ValueError, IndexError, UnicodeDecodeError):
        return UNDECODABLE  # AUSD's version() answers 96 bytes that are no ABI string


def readable(value) -> bool:
    return value not in (REVERTS, UNDECODABLE)


def _abi_string(raw: bytes, offset: int) -> str:
    length = int.from_bytes(raw[offset : offset + 32], "big")
    if offset % 32 or offset + 32 + length > len(raw):
        raise ValueError("not an ABI string")
    return raw[offset + 32 : offset + 32 + length].decode("utf-8")


def decode_string(raw: bytes) -> str:
    if len(raw) == 32:  # a bytes32 name, as some old tokens return
        text = raw.rstrip(b"\0")
        if b"\0" in text:
            raise ValueError("not a bytes32 string")
        return text.decode("utf-8")
    if int.from_bytes(raw[:32], "big") != 32:
        raise ValueError("not an ABI string")
    return _abi_string(raw, 32)


def decode_bytes32(raw: bytes) -> str:
    if len(raw) != 32:
        raise ValueError("not a bytes32")
    return raw.hex()


def decode_eip712_domain(raw: bytes) -> dict:
    """EIP-5267 `eip712Domain()`: (bytes1 fields, string name, string version,
    uint256 chainId, address verifyingContract, bytes32 salt, uint256[] extensions)."""
    words = [raw[i : i + 32] for i in range(0, 7 * 32, 32)]
    if len(words[6]) != 32:
        raise ValueError("short eip712Domain() answer")
    return {
        "fields": words[0][0],
        "name": _abi_string(raw, int.from_bytes(words[1], "big")),
        "version": _abi_string(raw, int.from_bytes(words[2], "big")),
        "chain_id": int.from_bytes(words[3], "big"),
        "verifying_contract": "0x" + words[4][12:].hex(),
    }


def separator(name: str, version: str, chain_id: int, address: str) -> bytes:
    return keccak(
        keccak(DOMAIN_TYPE.encode())
        + keccak(name.encode("utf-8"))
        + keccak(version.encode("utf-8"))
        + chain_id.to_bytes(32, "big")
        + bytes(12)
        + bytes.fromhex(address[2:])
    )


# ---------------------------------------------------------------------------
# Measuring
# ---------------------------------------------------------------------------


def measure_network(network, rows, chain_id, rpc_var, rpc_url, timeout):
    """Fill in every row of one network; all reads pinned to one block."""
    common = {"rpc_var": rpc_var, "rpc": rpc_url, "chain_id": chain_id}
    if rpc_url is None:
        return [dict(r, **common, verdict="UNMEASURED", detail=f"no RPC URL for {rpc_var}") for r in rows]
    try:
        answered = int(rpc(rpc_url, "eth_chainId", [], timeout), 16)
        block = rpc(rpc_url, "eth_blockNumber", [], timeout)
    except RpcError as e:
        return [dict(r, **common, verdict="UNMEASURED", detail=f"RPC: {e}") for r in rows]
    # A few blocks behind the head: a load-balanced RPC may route the read to a
    # node that has not seen the head yet ("unsupported block number").
    common["block"] = max(int(block, 16) - 5, 0)
    block = hex(common["block"])
    if answered != chain_id:
        detail = f"RPC answers chain id {answered}, declared {chain_id}"
        return [dict(r, **common, verdict="UNMEASURED", detail=detail) for r in rows]
    out = []
    for r in rows:
        r = dict(r, **common)
        addr = r["address"]
        try:
            r["onchain_separator"] = read(rpc_url, addr, SEL_DOMAIN_SEPARATOR, block, timeout, decode_bytes32)
            r["onchain_name"] = read(rpc_url, addr, SEL_NAME, block, timeout, decode_string)
            r["onchain_version"] = read(rpc_url, addr, SEL_VERSION, block, timeout, decode_string)
            r["onchain_eip712_domain"] = read(rpc_url, addr, SEL_EIP712_DOMAIN, block, timeout, decode_eip712_domain)
        except RpcError as e:
            out.append(dict(r, verdict="UNMEASURED", detail=f"RPC: {e}"))
            continue
        r["declared_separator"] = separator(r["name"], r["version"], chain_id, addr).hex()
        r["verdict"], r["detail"], r["separator_match"] = judge(r, chain_id)
        out.append(r)
    return out


def judge(r, chain_id):
    """(verdict, detail, separator_match). The separator decides whenever the contract publishes one."""
    domain = r["onchain_eip712_domain"] if readable(r["onchain_eip712_domain"]) else None
    if readable(r["onchain_separator"]):
        if r["onchain_separator"] == r["declared_separator"]:
            note = ""
            if readable(r["onchain_name"]) and r["onchain_name"] != r["name"]:
                note = f"name() is {show(r['onchain_name'])}, not the domain name"
            return "OK", note, True
        # Which domain does reproduce it? eip712Domain() first, then name()/version().
        candidates = []
        if domain:
            candidates.append(("eip712Domain()", domain["name"], domain["version"]))
        if readable(r["onchain_name"]):
            version = r["onchain_version"] if readable(r["onchain_version"]) else r["version"]
            candidates.append(("name()/version()", r["onchain_name"], version))
        for source, name, version in candidates:
            if separator(name, version, chain_id, r["address"]).hex() == r["onchain_separator"]:
                return "MISMATCH", f"the separator is ({show(name)}, {version}), from {source}", False
        return "MISMATCH", "the separator is not the declared domain, nor any domain read here", False
    # No DOMAIN_SEPARATOR(): the best left is eip712Domain(), then the getters.
    if domain:
        source, name, version = "eip712Domain()", domain["name"], domain["version"]
    elif readable(r["onchain_name"]):
        source, name, version = "name()/version()", r["onchain_name"], r["onchain_version"]
    else:
        return "UNMEASURED", "DOMAIN_SEPARATOR(), eip712Domain() and name() all unreadable", None
    differs = [f for f, a, b in (("name", name, r["name"]), ("version", version, r["version"]))
               if readable(a) and a != b]
    if differs:
        return "MISMATCH", f"no DOMAIN_SEPARATOR(); {source} differs in {', '.join(differs)}", None
    return "OK", f"no DOMAIN_SEPARATOR(); compared with {source} only", None


def show(name):
    """A name as text, with its non-ASCII code points spelled out (USD\\u20ae0 looks like USDT0)."""
    if not readable(name):
        return name
    extra = [f"U+{ord(c):04X}" for c in name if ord(c) > 0x7F]
    return name + (f" [{' '.join(extra)}]" if extra else "")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    fmt = ap.add_mutually_exclusive_group()
    fmt.add_argument("--md", action="store_true", help="Markdown table")
    fmt.add_argument("--json", action="store_true", help="JSON rows")
    ap.add_argument("--timeout", type=float, default=20.0, help="seconds per RPC call (default 20)")
    args = ap.parse_args()

    network_rs = NETWORK_RS.read_text("utf-8")
    rows, without = parse_deployments(network_rs)
    chain_ids = parse_chain_ids(network_rs)
    env_names = parse_rpc_env_names(FROM_ENV_RS.read_text("utf-8"))
    public = parse_public_rpcs()

    by_network = {}
    for r in rows:
        by_network.setdefault(r["network"], []).append(r)
    jobs = []
    for network, net_rows in by_network.items():
        if network not in chain_ids:
            sys.exit(f"ERROR: Network::{network} has no eip155 chain id in to_caip2")
        var = env_names[network]
        if os.environ.get(var):
            url, source = os.environ[var], "env"
        else:
            url, source = public.get(var, (None, None))
        for r in net_rows:
            r["rpc_source"] = source
        jobs.append((network, net_rows, chain_ids[network], var, url))

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = [row for rs in pool.map(lambda j: measure_network(*j, args.timeout), jobs) for row in rs]
    results.sort(key=lambda r: (r["network"], r["token"]))

    if args.json:
        print(json.dumps({"rows": results, "evm_deployments_without_static_domain": without}, indent=2))
    else:
        md = args.md
        head = ["Network", "Token", "Address", "Chain", "RPC", "Block", "Declared name / version",
                "name() / version()", "eip712Domain()", "DOMAIN_SEPARATOR()", "Verdict"]
        if md:
            print("| " + " | ".join(head) + " |")
            print("|" + "---|" * len(head))
        for r in results:
            onchain = domain = sep = "-"
            if "onchain_name" in r:
                onchain = f"{show(r['onchain_name'])} / {show(r['onchain_version'])}"
                d = r["onchain_eip712_domain"]
                domain = f"{show(d['name'])} / {d['version']}" if readable(d) else d
                s = r["onchain_separator"]
                if readable(s):
                    sep = s[:8] + "... " + {True: "match", False: "DIFFERS"}[r["separator_match"]]
                else:
                    sep = s
            host = urllib.parse.urlsplit(r["rpc"]).hostname if r.get("rpc") else "-"
            cells = [r["network"], r["token"], r["address"], str(r["chain_id"]), host, str(r.get("block", "-")),
                     f"{show(r['name'])} / {r['version']}", onchain, domain, sep,
                     r["verdict"] + (f" ({r['detail']})" if r.get("detail") else "")]
            if md:
                print("| " + " | ".join(c.replace("|", "\\|") for c in cells) + " |")
            else:
                print("  ".join(cells))
        print()
        counts = {}
        for r in results:
            counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
        print("Totals: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) + f" (of {len(results)})")
        if without:
            names = ", ".join(f"{w['token']} on {w['network']}" for w in without)
            print(f"EVM deployments with no static EIP-712 domain (not compared): {names}")
    sys.exit(1 if any(r["verdict"] == "MISMATCH" for r in results) else 0)


if __name__ == "__main__":
    main()
