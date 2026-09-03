#!/usr/bin/env python3
"""verify_landing_canonical.py -- single source of truth check.

The landing page (static/index.html) must NEVER display a network count or
network list that disagrees with what the facilitator actually supports. This
script is the canonical map: it derives the real numbers from authoritative
sources and fails (exit 1) if the landing page drifts from them.

Canonical sources
-----------------
  * Payment networks  -> GET /supported (the live facilitator)         [21 mainnets]
  * Escrow networks   -> src/payment_operator/addresses.rs             [9 mainnets]
  * ERC-8004 networks -> src/erc8004/mod.rs (supported_networks)       [11 mainnets / 20 total]

The landing page is the CONSUMER; these three are the PRODUCERS. If they ever
disagree, this script tells you exactly where.

The landing page is bilingual in ONE document, so a count is written in three
places -- the English markup, the `en` dictionary and the `es` dictionary -- and
until 2026-09-02 only the first was checked. The Spanish side carried the same
numbers by hand and nothing would have said a word when it stopped.

Usage
-----
  python scripts/verify_landing_canonical.py
  python scripts/verify_landing_canonical.py --url https://facilitator.ultravioletadao.xyz
  python scripts/verify_landing_canonical.py --supported-file /tmp/supported.json
  python scripts/verify_landing_canonical.py --offline    # CI: no network at all

Wire this into:
  * CI (the `Build & test` job runs it with --offline), and
  * every deploy (pre-flight, live), and
  * the /add-network skill (after adding a network, before shipping).

`--offline` skips the ONE producer that needs the network (`GET /supported`) and
keeps everything else, including the whole EN/ES cross-check. It exists because
a CI check that calls production goes red when production is down, which is the
worst possible moment to block a deploy -- and because the drift that actually
happened is visible without leaving the repository.

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
# Consumer: numbers shown on the landing page -- markup AND both dictionaries
# ---------------------------------------------------------------------------
# Every landing string that states a network count, and which producer owns it.
# The pattern is per key because a bare \d+ finds the 8004 in "ERC-8004" long
# before it finds the count, and a checker that reads the wrong number is worse
# than no checker.
COUNT_KEYS = {
    "sdk.networks":                    ("payment",  r"(\d+)\s+mainnets"),
    "networks.summary":                ("payment",  r"(\d+)\s+(?:payment mainnets|mainnets de pago)"),
    "erc8004.networksTitle":           ("erc8004",  r"(\d+)\s+(?:Networks|Redes)"),
    "x402r.networksTitle":             ("escrow",   r"(\d+)\s+(?:Networks|Redes)"),
    "features.reputation.description": ("erc8004",  r"(?:across|en)\s+(\d+)\s+(?:networks|redes)"),
    "endpoints.erc8004Note":           ("erc8004",  r"^(\d+)\s+(?:networks|redes)"),
    "x402r.description":               ("escrow",   r"(?:across|en)\s+(\d+)\s+(?:networks|redes)"),
}


def _brace_block(text: str, start: int) -> str:
    """The inside of the first {...} at or after `start`, string-aware.

    A plain `.index("}")` stops at the first brace inside a translated value,
    and several of them carry inline HTML with `style="..."` attributes.
    """
    i = text.index("{", start)
    depth = 0
    quote = None
    escaped = False
    for j in range(i, len(text)):
        c = text[j]
        if quote:
            if escaped:
                escaped = False
            elif c == "\\":
                escaped = True
            elif c == quote:
                quote = None
            continue
        if c in "\"'`":
            quote = c
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return text[i + 1: j]
    raise ValueError("the translations object is not brace-balanced")


def landing_dictionaries(html: str) -> dict:
    """{'en': <block>, 'es': <block>} from `const translations = {...}`."""
    block = _brace_block(html, html.index("const translations = {"))
    out = {}
    for lang in ("en", "es"):
        pos = 0
        while True:
            at = block.index(f"{lang}:", pos)
            after = at + len(lang) + 1
            if block[after:].lstrip().startswith("{"):
                out[lang] = _brace_block(block, after)
                break
            pos = after
    return out


def dict_value(block: str, key: str):
    m = re.search(r'"%s"\s*:\s*"((?:[^"\\]|\\.)*)"' % re.escape(key), block)
    return m.group(1) if m else None

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

    # The same counts as written in each dictionary. `None` means the key is
    # missing; `"?"` means the sentence no longer matches its pattern, which is
    # a real failure -- a count that moved into prose nobody checks is exactly
    # how the Spanish side drifted in the first place.
    dicts = landing_dictionaries(html)
    counts: dict = {}
    for key, (_producer, pattern) in COUNT_KEYS.items():
        for lang in ("en", "es"):
            value = dict_value(dicts[lang], key)
            if value is None:
                counts[(lang, key)] = None
                continue
            m = re.search(pattern, value)
            counts[(lang, key)] = int(m.group(1)) if m else "?"
    out["dict_counts"] = counts
    return out


# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default=DEFAULT_URL, help="facilitator base URL")
    ap.add_argument("--supported-file", help="read /supported JSON from a file instead of HTTP")
    ap.add_argument("--expect-mainnets", type=int, default=21,
                    help="expected canonical mainnet payment-network count (default 21)")
    ap.add_argument("--offline", action="store_true",
                    help="skip GET /supported (the only producer that needs the network) "
                         "and check everything else, including the EN/ES cross-check")
    args = ap.parse_args()

    errors: list[str] = []
    notes: list[str] = []

    # ----- payment networks -----
    if args.offline and not args.supported_file:
        chains = set()
        pay_count = None
        notes.append("--offline: GET /supported was not read, so the payment-network "
                     "count is unchecked. The EN/ES cross-check still ran.")
    else:
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
    print(f"  payment mainnets  (/supported)            : "
          f"{'not read (--offline)' if pay_count is None else pay_count}")
    if chains:
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
    print("-" * 70)
    print("LANDING DICTIONARIES  (en / es, same document, one URL)")
    for key, (producer, _pattern) in COUNT_KEYS.items():
        en = land["dict_counts"][("en", key)]
        es = land["dict_counts"][("es", key)]
        print(f"  {key:34}: en={en}  es={es}   [{producer}]")
    print("=" * 70)

    # ----- assertions -----
    if pay_count is not None and pay_count != args.expect_mainnets:
        notes.append(f"/supported has {pay_count} mainnets, expected {args.expect_mainnets} "
                     f"(update --expect-mainnets if you intentionally changed the network set)")
    if pay_count is not None and land["sdk_mainnets"] != pay_count:
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

    # ----- the dictionaries, both languages -----
    producers = {"payment": pay_count, "escrow": len(escrow), "erc8004": len(erc_all)}
    for key, (producer, _pattern) in COUNT_KEYS.items():
        en = land["dict_counts"][("en", key)]
        es = land["dict_counts"][("es", key)]
        for lang, got in (("en", en), ("es", es)):
            if got is None:
                errors.append(f"'{key}' is missing from the `{lang}` dictionary")
            elif got == "?":
                errors.append(f"'{key}' in `{lang}` no longer states its count in a "
                              f"shape this script can read; the sentence was rewritten "
                              f"and the number is now unchecked")
        if isinstance(en, int) and isinstance(es, int) and en != es:
            errors.append(f"'{key}' says {en} in English and {es} in Spanish. Same "
                          f"document, one URL: a reader gets a different number "
                          f"depending on which button they pressed")
        expected = producers[producer]
        if expected is None:
            continue  # --offline: the payment producer was not read
        for lang, got in (("en", en), ("es", es)):
            if isinstance(got, int) and got != expected:
                errors.append(f"'{key}' says {got} in `{lang}` but {producer} has "
                              f"{expected}")

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
