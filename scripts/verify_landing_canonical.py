#!/usr/bin/env python3
"""verify_landing_canonical.py -- single source of truth check.

The landing page (static/index.html) must NEVER display a network count or
network list that disagrees with what the facilitator actually supports. This
script is the canonical map: it derives the real numbers from authoritative
sources and fails (exit 1) if the landing page drifts from them.

Canonical sources
-----------------
  * Payment networks  -> GET /supported (the live facilitator)         [20 mainnets]
  * Escrow networks   -> src/payment_operator/addresses.rs             [9 mainnets]
  * ERC-8004 networks -> src/erc8004/mod.rs (supported_networks)       [11 mainnets / 20 total]

The landing page is the CONSUMER; these three are the PRODUCERS. If they ever
disagree, this script tells you exactly where.

Usage
-----
  python scripts/verify_landing_canonical.py
  python scripts/verify_landing_canonical.py --url https://facilitator.ultravioletadao.xyz
  python scripts/verify_landing_canonical.py --supported-file /tmp/supported.json   # offline / CI

Wire this into:
  * every deploy (pre-flight), and
  * the /add-network skill (after adding a network, before shipping).

Exit codes: 0 = landing matches reality, 1 = drift detected, 2 = could not read a source.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_URL = "https://facilitator.ultravioletadao.xyz"

# Substrings in a network id / Network enum variant that mark it as a testnet.
TESTNET_MARKERS = (
    "testnet", "sepolia", "devnet", "fuji", "amoy",
    "alfajores", "holesky", "baklava",
)


def is_testnet(name: str) -> bool:
    low = name.lower()
    return any(m in low for m in TESTNET_MARKERS)


# ---------------------------------------------------------------------------
# Producer 1: payment networks from GET /supported
# ---------------------------------------------------------------------------
def load_supported(url: str | None, supported_file: str | None) -> dict:
    if supported_file:
        return json.loads(Path(supported_file).read_text())
    req = urllib.request.Request(
        url.rstrip("/") + "/supported",
        headers={"User-Agent": "verify-landing-canonical/1.0"},
    )
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.loads(resp.read().decode())


def supported_mainnet_chains(data: dict) -> set[str]:
    """Distinct MAINNET payment chains from /supported.

    /supported lists each network in several alias forms (plain, '-sepolia',
    CAIP-2 'eip155:...', 'solana:...', etc). We keep only the plain mainnet
    names so each chain is counted exactly once -- the same logic the landing
    page runs in the browser.
    """
    chains: set[str] = set()
    for kind in data.get("kinds", []):
        net = kind.get("network")
        if not isinstance(net, str):
            continue
        if ":" in net:           # skip CAIP-2 aliases
            continue
        if is_testnet(net):      # skip testnets
            continue
        chains.add(net[: -len("-mainnet")] if net.endswith("-mainnet") else net)
    return chains


# ---------------------------------------------------------------------------
# Producer 2 & 3: escrow + ERC-8004 networks parsed from Rust source
# ---------------------------------------------------------------------------
def _network_variants(text: str) -> list[str]:
    return re.findall(r"Network::([A-Za-z0-9]+)", text)


def _slice_block(text: str, start_pat: str) -> str:
    """Return text from the first match of start_pat to the next ']' or '}'."""
    m = re.search(start_pat, text)
    if not m:
        return ""
    rest = text[m.end():]
    end = re.search(r"[\]\}]", rest)
    return rest[: end.start()] if end else rest


def escrow_mainnets() -> set[str]:
    """Mainnet networks that have a PaymentOperator escrow deployment."""
    src = (REPO / "src" / "payment_operator" / "addresses.rs").read_text()
    # The supported list is the array of Network:: entries near the top of the
    # escrow-address resolver. Take every Network:: in the file's match/list and
    # drop testnets -- escrow deployment is keyed on these variants.
    block = _slice_block(src, r"escrow_for_network|SUPPORTED|pub fn escrow")
    variants = _network_variants(block) or _network_variants(src)
    return {v for v in variants if not is_testnet(v)}


def erc8004_networks() -> tuple[set[str], set[str]]:
    """(mainnet variants, all variants) with an ERC-8004 deployment."""
    src = (REPO / "src" / "erc8004" / "mod.rs").read_text()
    block = _slice_block(src, r"pub fn supported_networks\s*\(")
    variants = _network_variants(block)
    if not variants:  # fallback: the get_contracts match
        block = _slice_block(src, r"pub fn get_contracts")
        variants = _network_variants(block)
    allv = set(variants)
    return {v for v in allv if not is_testnet(v)}, allv


# ---------------------------------------------------------------------------
# Consumer: numbers shown on the landing page
# ---------------------------------------------------------------------------
def landing_numbers() -> dict:
    html = (REPO / "static" / "index.html").read_text()
    out: dict = {"raw": html}

    def first_int(pattern: str):
        m = re.search(pattern, html)
        return int(m.group(1)) if m else None

    out["sdk_mainnets"] = first_int(r'data-i18n="sdk\.networks"[^>]*>(\d+)\s+mainnets')
    out["erc8004_title"] = first_int(r'data-i18n="erc8004\.networksTitle">Deployed on (\d+) Networks')
    out["erc8004_stat"] = first_int(r'id="ovr-erc8004-networks"[^>]*>(\d+)<')
    out["escrow_title"] = first_int(r'data-i18n="x402r\.networksTitle">Escrow Deployed on (\d+) Networks')
    # logo cards inside each showcase grid (small 20px icons)
    out["hedera_refs"] = len(re.findall(r"hedera", html, re.I))
    out["scroll_present"] = bool(re.search(r'src="/scroll\.png"', html))
    return out


# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default=DEFAULT_URL, help="facilitator base URL")
    ap.add_argument("--supported-file", help="read /supported JSON from a file instead of HTTP")
    ap.add_argument("--expect-mainnets", type=int, default=20,
                    help="expected canonical mainnet payment-network count (default 20)")
    args = ap.parse_args()

    errors: list[str] = []
    notes: list[str] = []

    # ----- payment networks -----
    try:
        data = load_supported(args.url, args.supported_file)
        chains = supported_mainnet_chains(data)
        pay_count = len(chains)
    except Exception as e:  # noqa: BLE001
        print(f"[FAIL] could not read /supported: {e}", file=sys.stderr)
        return 2

    # ----- escrow / erc8004 from source -----
    try:
        escrow = escrow_mainnets()
        erc_main, erc_all = erc8004_networks()
    except Exception as e:  # noqa: BLE001
        print(f"[FAIL] could not parse Rust sources: {e}", file=sys.stderr)
        return 2

    land = landing_numbers()

    print("=" * 70)
    print("CANONICAL MAP  (source of truth)")
    print("=" * 70)
    print(f"  payment mainnets  (/supported)            : {pay_count}")
    print(f"    -> {', '.join(sorted(chains))}")
    print(f"  escrow mainnets   (payment_operator)      : {len(escrow)}")
    print(f"    -> {', '.join(sorted(escrow))}")
    print(f"  erc-8004 mainnets (erc8004/mod.rs)        : {len(erc_main)}")
    print(f"  erc-8004 total    (mainnet + testnet)     : {len(erc_all)}")
    print("-" * 70)
    print("LANDING PAGE  (static/index.html)")
    print(f"  sdk 'N mainnets supported'                : {land['sdk_mainnets']}")
    print(f"  erc-8004 'Deployed on N Networks'         : {land['erc8004_title']}")
    print(f"  erc-8004 stat card                        : {land['erc8004_stat']}")
    print(f"  escrow 'Escrow Deployed on N Networks'    : {land['escrow_title']}")
    print(f"  hedera references                         : {land['hedera_refs']}")
    print(f"  scroll logo present                       : {land['scroll_present']}")
    print("=" * 70)

    # ----- assertions -----
    if pay_count != args.expect_mainnets:
        notes.append(f"/supported has {pay_count} mainnets, expected {args.expect_mainnets} "
                     f"(update --expect-mainnets if you intentionally changed the network set)")
    if land["sdk_mainnets"] != pay_count:
        errors.append(f"landing says '{land['sdk_mainnets']} mainnets supported' "
                      f"but /supported has {pay_count}")
    if land["escrow_title"] != len(escrow):
        errors.append(f"landing escrow shows {land['escrow_title']} networks "
                      f"but payment_operator has {len(escrow)} mainnet deployments")
    if land["erc8004_title"] != len(erc_all):
        errors.append(f"landing ERC-8004 shows 'Deployed on {land['erc8004_title']} Networks' "
                      f"but erc8004/mod.rs has {len(erc_all)} deployments")
    if land["erc8004_stat"] not in (len(erc_all), len(erc_main)):
        errors.append(f"landing ERC-8004 stat card = {land['erc8004_stat']} "
                      f"but source has {len(erc_all)} total / {len(erc_main)} mainnet")
    if land["hedera_refs"] != 0:
        errors.append(f"landing still has {land['hedera_refs']} 'hedera' reference(s)")

    for n in notes:
        print(f"[NOTE] {n}")
    if errors:
        print()
        for e in errors:
            print(f"[DRIFT] {e}")
        print(f"\n[FAIL] {len(errors)} drift(s) between the landing page and the canonical sources.")
        return 1

    print("[OK] landing page matches /supported, escrow, and ERC-8004 sources.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
