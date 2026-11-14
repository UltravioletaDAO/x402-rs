#!/usr/bin/env python3
"""Every published count and list of networks, from config/network_capabilities.json.

``config/network_capabilities.json`` is written by ``src/network_manifest.rs``
from the functions the payment, escrow and ERC-8004 paths call, under
production's profile, and says per capability how many networks this
facilitator serves and which (payments, upto, x402r escrow, NEAR escrow,
ERC-8004 and its relayed feedback). A published text may state such a number
only bound to its capability, in one of the phrases of ``PHRASES``
("Escrow Deployed on 11 Networks", "ERC-8004 ... live on 14 mainnets and 25
networks in total"), and a list of networks only as the whole set of one
capability. Then a new network changes all of them with one command:

    python scripts/network_copy.py --generate

Modes (stdlib only, run from anywhere):

  --generate   recompute the manifest (cargo test with X402_WRITE_NETWORK_MANIFEST
               set, production features), then --write
  --write      rewrite every bound count, bound list and bound value of COVERED
               to the manifest, and rebuild static/llms-full.txt
  --check      exit 1 when a COVERED file states a number or a list of networks
               that does not come from the manifest, a link preview states a
               number at all, or production's Terraform disagrees with the profile
               the manifest was computed under (the default mode)
  --inventory  print every count and list of networks in COVERED (TSV: file, line,
               kind, status, text)
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Sequence, Set, Tuple

REPO = Path(__file__).resolve().parents[1]
MANIFEST_REL = "config/network_capabilities.json"
TERRAFORM_REL = "terraform/environments/production"
CARGO_FEATURES = "solana,near,stellar,algorand,sui,xrpl,hedera"
WRITE_ENV = "X402_WRITE_NETWORK_MANIFEST"
MANIFEST_TEST = "network_manifest::tests::the_committed_manifest_is_the_one_the_code_computes"
LOGO = "https://facilitator.ultravioletadao.xyz/logo.png"

# The published text this check owns: every page and document a visitor, an
# agent or a catalog reads, the strings that reach /docs and the MCP tools, and
# the repository's own instructions. Historical records (CHANGELOG, docs/handoffs,
# plans, reports) state what was true on their date and are never rewritten.
COVERED = (
    "README.md",
    "CLAUDE.md",
    "guides/ADDING_NEW_CHAINS.md",
    "static/*.html",
    "static/*.md",
    "static/*.txt",
    "static/*.json",
    "static/.well-known/*",
    "static/.well-known/*/*",
    "static/.well-known/*/*/*",
    "src/openapi.rs",
    "src/mcp.rs",
    "paybox-plugin/README.md",
    "paybox-plugin/src/catalogo.generated.ts",
)
# Written by another generator from covered sources: checked, never rewritten
# here (their generator would undo it), so a wrong number in one of them means
# "fix the source and re-run its generator". --write re-runs the first.
GENERATED = {
    "static/llms-full.txt": "scripts/build_llms_full.sh",
    "paybox-plugin/src/catalogo.generated.ts": "src/mcp.rs (paybox-plugin/README.md)",
}

# ---------------------------------------------------------------------------
# Vocabulary
# ---------------------------------------------------------------------------
# Spellings of a network beyond its id and its displayName in
# config/supported_tokens.json. Matched case-insensitively.
ALIASES: Dict[str, Tuple[str, ...]] = {
    "bsc": ("BSC", "BNB", "BNB Chain", "BNB Smart Chain"),
    "skale-base": ("SKALE Base", "SKALE", "skale"),
    "robinhood": ("Robinhood Chain", "Robinhood"),
    "robinhood-testnet": ("Robinhood Chain Testnet", "Robinhood Testnet"),
    "hashkey": ("HashKey Chain", "HashKey"),
    "hashkey-testnet": ("HashKey Chain Testnet", "HashKey Testnet"),
    "xrpl": ("XRPL", "XRP Ledger"),
    "xrpl-testnet": ("XRPL Testnet", "XRP Ledger Testnet"),
    "hedera:mainnet": ("Hedera", "native Hedera", "hedera"),
    "hedera:testnet": ("Hedera Testnet", "hedera-testnet"),
    "avalanche": ("Avalanche C-Chain",),
    "arc": ("Arc mainnet",),
    "near": ("NEAR Protocol",),
    "stellar": ("Stellar/Soroban",),
    "ethereum": ("Ethereum mainnet",),
}
# How a chain family is written. `svm` covers Solana and Fogo.
FAMILY_NAMES: Dict[str, Tuple[str, ...]] = {
    "evm": ("EVM",),
    "svm": ("SVM", "Solana", "SVM/Solana"),
    "near": ("NEAR",),
    "stellar": ("Stellar",),
    "algorand": ("Algorand",),
    "sui": ("Sui",),
    "xrpl": ("XRPL",),
    "hedera": ("Hedera", "native Hedera"),
}

# ---------------------------------------------------------------------------
# Phrases: a number of networks, bound to the capability (and field) it counts
# ---------------------------------------------------------------------------
# {count} {mainnets} {testnets} {families} are numbers (field of the capability);
# {list} is a list of networks bound to (capability, subset). Matched
# case-insensitively on a view of the text where whitespace runs are one space,
# `**` is gone and a Rust string continuation is a space. The longer match of
# an overlap wins: "Escrow Deployed on 11 Networks" is never read as the ERC-8004
# "Deployed on 25 Networks".


@dataclass(frozen=True)
class Phrase:
    capability: str  # a key of manifest.capabilities, or "families"
    pattern: str
    subset: str = "all"  # what a {list} must equal: all | mainnet | testnet
    style: str = "names"  # how --write spells a {list}: names | ids | short | code (`id`)


def _KEY(key: str) -> str:  # noqa: N802 -- reads as part of a pattern
    """The landing's i18n key right before its text: `data-i18n="k">` in the
    markup, `"k": "` in each dictionary."""
    return re.escape(key) + r'"(?:>|: ") ?'


_EVM_MAINNETS = r"### EVM Chains \(Mainnet\) {list} (?=### EVM)"
_EVM_TESTNETS = r"### EVM Chains \(Testnet\) {list} (?=Ethereum Sepolia is announced|### SVM)"


PHRASES: Tuple[Phrase, ...] = (
    # --- payments (no published count today; these are the words to use) ---
    Phrase("payments", r"{count} payment networks"),
    Phrase("payments", r"{mainnets} payment mainnets"),
    Phrase("payments", r"{count} redes de pago"),
    Phrase("families", r"{families} chain families"),
    Phrase("families", r"{families} familias de cadenas"),
    # --- ERC-8004 ---
    Phrase(
        "erc8004",
        r"(?:identity and reputation|reputation for AI agents|validation registries) across "
        r"{count} networks(?: \({mainnets} mainnets \+ {testnets} testnets\))?",
    ),
    Phrase("erc8004", r"identidad, reputación y validación en {count} redes"),
    Phrase("erc8004", r"{count} (?:networks|redes) \(EVM \+ Solana\)"),
    Phrase("erc8004", r"Supported ERC-8004 Networks \({count}\)"),
    # Shape-only wording ("live on N mainnets", "Deployed on N Networks", "+ N
    # testnets") is bound only next to what names its capability: the words
    # "ERC-8004", or the landing's i18n key, in the markup (`data-i18n="k">`) and
    # in each dictionary (`"k": "`). Without the anchor, --write would rewrite an
    # upto sentence with ERC-8004's numbers.
    Phrase(
        "erc8004",
        r"ERC-8004[^.\n]{0,80}?live on {mainnets} mainnets(?: and|,) {count} networks in total",
    ),
    Phrase(
        "erc8004",
        r"ERC-8004[^.\n]{0,80}?live on {mainnets} mainnets \({count} networks counting testnets\)",
    ),
    Phrase(
        "erc8004",
        r"identity and reputation on {mainnets} mainnets \({count} networks including testnets\)",
    ),
    Phrase("erc8004", _KEY("erc8004.networksTitle") + r"(?:Deployed on|Desplegado en) {count} (?:Networks|Redes)"),
    Phrase("erc8004", _KEY("erc8004.create2Note") + r"\+ {testnets} testnets\."),
    Phrase(
        "erc8004",
        _KEY("endpoints.erc8004Note") + r"{count} (?:networks|redes): {list} \+ testnets",
        "mainnet",
        "short",
    ),
    Phrase(
        "erc8004_canonical",
        _KEY("erc8004.addressesNote") + r"(?:Same addresses on|Las mismas direcciones en) {list}(?=\. SKALE)",
        "mainnet",
        "names",
    ),
    # --- x402r escrow ---
    Phrase(
        "escrow_x402r",
        _KEY("x402r.networksTitle") + r"Escrow (?:Deployed on|Desplegado en) {mainnets} (?:Networks|Redes)",
    ),
    Phrase("escrow_x402r", r"refunds for HTTP 402 payments across {mainnets} networks"),
    Phrase("escrow_x402r", r"reembolsos sin confianza para pagos HTTP 402 en {mainnets} redes"),
    Phrase(
        "escrow_x402r",
        _KEY("x402r.create2Note") + r"\+ {testnets} testnets \({list}\)",
        "testnet",
        "names",
    ),
    Phrase("escrow_x402r", r"Escrow contracts deployed on {count} networks"),
    Phrase("escrow_x402r", r"Escrow networks \({count}\): {list}\.(?= Each)", "all", "code"),
    # --- upto ---
    Phrase("upto", r"Upto networks \({count}\): {list} (?=—)", "all", "names"),
    # --- payments, by family, in /docs ---
    Phrase("payments", _EVM_MAINNETS, "mainnet", "names"),
    Phrase("payments", _EVM_TESTNETS, "testnet", "names"),
)
# The family a bound {list} is restricted to, by its phrase's pattern.
PHRASE_FAMILY = {_EVM_MAINNETS: "evm", _EVM_TESTNETS: "evm"}

# A count written as a bare value, by (file, regex with one number group).
VALUE_BINDINGS: Tuple[Tuple[str, str, str, str], ...] = (
    ("static/index.html", r'id="ovr-erc8004-networks"[^>]*>(?P<count>\d+)<', "erc8004", "count"),
)

@dataclass(frozen=True)
class SetBinding:
    """One network per row or per card between two markers: the rows' `id`
    group must name exactly the capability's subset (optionally only some
    families). Check-only: a row carries presentation (tokens, explorer, chain
    id, addresses) the manifest does not have."""

    rel: str
    start: str
    end: str
    pattern: str
    capability: str
    subset: str
    exclude_families: Tuple[str, ...] = ()


SET_BINDINGS: Tuple[SetBinding, ...] = (
    SetBinding("README.md", "\n### Mainnets\n", "\n### Testnets\n",
               r"^\| \*\*(?P<id>[^*|]+)\*\* \|", "served", "mainnet"),
    SetBinding("README.md", "\n### Testnets\n", "\n### Supported Stablecoins\n",
               r"^\| (?P<id>(?!Network\b)[A-Za-z][^|]*?) \|", "served", "testnet"),
    # The ERC-8004 table: one row per network, typed Mainnet or Testnet.
    SetBinding("README.md", "\n### Supported ERC-8004 Networks", "\nAll EVM mainnet contracts use CREATE2",
               r"^\| (?P<id>(?!Registry\b)[^|]+?) \| (?:Mainnet|Testnet) \|", "erc8004", "all"),
    # The x402r escrow table: its bold rows are the mainnets. Its testnet rows
    # record what /supported announced on a date (an operator is announced
    # only once it passes an on-chain self-check, at runtime) and are not bound.
    SetBinding("README.md", "\n**`escrow` scheme (PaymentOperator).**", "\nEvery network in this table",
               r"^\| \*\*(?P<id>[^*|]+)\*\* \|", "escrow_x402r", "mainnet"),
    # /docs: the networks of the families that are not EVM, one bullet per
    # family, each id in backticks and parentheses.
    SetBinding("src/openapi.rs", "\n### SVM Chains", "\nArc uses USDC",
               r"\(`(?P<id>[a-z0-9]+(?:[-:][a-z0-9]+)*)`\)", "payments", "all",
               exclude_families=("evm",)),
    SetBinding("static/index.html", 'data-i18n="x402r.networksTitle"', 'data-i18n="x402r.create2Note"',
               r'data-net-icon="(?P<id>[^"]+)"', "escrow_x402r", "mainnet"),
    SetBinding("static/index.html", 'data-i18n="erc8004.networksTitle"', 'data-i18n="erc8004.create2Note"',
               r'data-net-icon="(?P<id>[^"]+)"', "erc8004", "mainnet"),
    SetBinding("static/index.html", 'id="mainnet-tab"', "</body>",
               r'class="network-badge[^"]*"(?:(?!<div).){0,600}?data-explorer="(?P<id>[^"]+)"',
               "served", "all"),
)

# Named exceptions, each with its reason: exact (file, text) pairs, a finding
# excused only in that file, and only for exactly that text: a count by its
# words, a list by its set of items (a list that gains or loses one, such as a
# family list that drops Hedera, is no longer excused). A text that records a
# measurement of named networks or a fact about something other than a
# capability. One that no longer occurs is an error too: a stale exception
# would hide the next real finding with the same text.
PENDING: Dict[Tuple[str, str], str] = {
    (
        "src/openapi.rs",
        "Avalanche, Celo, Scroll, Unichain, Optimism Sepolia and HashKey Chain",
    ): (
        "A measurement, not a capability: the EVM networks carrying `exact` where the "
        "upto proxy's CREATE2 deployment was never replayed (checked with eth_getCode). "
        "The text says to query /supported rather than trust the list."
    ),
    ("CLAUDE.md", "7 EVM networks"): (
        "A measurement of a date: DX402's 116 chain-verified anchors of 2026-09-04 "
        "landed on 7 EVM networks (docs/plans/dx402/09-ESTADO-Y-CAMINO-A-UPSTREAM.md)."
    ),
    (
        "CLAUDE.md",
        "`base`, `avalanche`, `polygon`, `optimism`, `celo`, `hyperevm`, `ethereum`, "
        "`arbitrum`, `unichain`, `arc`, `robinhood`",
    ): (
        "The JSON keys of the facilitator-rpc-mainnet secret (which networks have a premium "
        "RPC URL), a configuration fact, not a capability. The list runs on past `robinhood` "
        "(its note holds a nested parenthesis the list reader stops at)."
    ),
    ("CLAUDE.md", "chain registered here (6)"): (
        "Token coverage: the EVM networks where AUSD is registered, from src/network.rs "
        "(scripts/stablecoin_matrix.py), not a capability of the network."
    ),
    ("CLAUDE.md", "Solana, Stellar, Algorand, NEAR"): (
        "DX402: the families whose address is already an ed25519 key. A property of the "
        "signature scheme, not a capability."
    ),
}
# The USDC EIP-712 domain names, by chain: a fact about a token's contract.
for _rel in ("CLAUDE.md", "static/skill.md", "static/llms-full.txt"):
    PENDING[(_rel, "Celo, HyperEVM, Unichain, Monad")] = (
        "Where USDC's EIP-712 domain name is `USDC` rather than `USD Coin`: a fact about "
        "each token contract (src/network.rs), not a capability."
    )
# What the networks that are not EVM did until 2.39.6: history, kept on purpose.
for _rel in ("src/openapi.rs", "static/skill.md", "static/llms-full.txt"):
    PENDING[(_rel, "NEAR, Stellar, Algorand, Sui and XRPL")] = (
        "The families that reported a lost submission as `200 success:false` until 2.39.6: "
        "a past behaviour, true of those five and no capability."
    )
# Stablecoin coverage: README's stablecoin table, from scripts/stablecoin_matrix.py.
for _text in (
    "Ethereum, Polygon, Arbitrum, Avalanche, Monad, BSC, Solana",
    "Ethereum, Base, Avalanche, Arc mainnet",
    "Arbitrum, Celo, Optimism, Monad",
):
    PENDING[("README.md", _text)] = (
        "Where a token (AUSD, EURC, USDT) is served: token coverage from "
        "scripts/stablecoin_matrix.py, not a capability of the network."
    )

# ---------------------------------------------------------------------------
# Loose counts and lists
# ---------------------------------------------------------------------------
_UNIT = r"(?:networks?|mainnets?|testnets?|chains?|blockchains?|redes|cadenas|famil(?:y|ies|ias))"
# Up to two qualifiers between the number and its unit ("7 EVM networks", "3 new
# mainnets"), never a function word: "503 when a mainnet is down" counts nothing.
_STOP = r"(?:when|while|if|a|an|the|of|per|on|in|for|to|and|or|is|are|was|by|at|from|with|as|its|on)"
LOOSE_COUNT = re.compile(
    r"(?<![\w.$/:=#-])(\d{1,3})\+?\s+(?:(?!" + _STOP + r"\b)[A-Za-z][\w-]*\s+){0,2}?"
    + _UNIT + r"\b(?!-)",
    re.I,
)
# The same number after its unit: "payment-network count (21)", "Networks (25)",
# and a label: "Networks: 45".
_UNIT_AHEAD = r"\b(?:networks?|mainnets?|testnets?|chains?|redes|cadenas)\b"
LOOSE_AFTER = (
    re.compile(_UNIT_AHEAD + r"(?:[ -](?!" + _STOP + r"\b)[A-Za-z][\w-]*){0,2}\s*\((\d{1,3})\)", re.I),
    # A displayed label is capitalised: `{mainnet:0}` in a script is not one.
    re.compile(r"\b(?:Networks?|Mainnets?|Testnets?|Chains?|Redes|Cadenas)\s*:\s+(\d{1,3})\b(?![\w.:/-])"),
)
LOOSE_PATTERNS = (LOOSE_COUNT,) + LOOSE_AFTER
MIN_LIST = 4
# A list introduced as an example claims no set.
ILLUSTRATION = re.compile(
    r"(?:e\.g\.|for example|for instance|such as|including but not limited to|por ejemplo|p\. ej\.|como)[\s,:(]*$",
    re.I,
)

# ---------------------------------------------------------------------------
# Text views
# ---------------------------------------------------------------------------


@dataclass
class View:
    rel: str
    raw: str
    flat: str
    pos: List[int]

    def line(self, flat_index: int) -> int:
        index = self.pos[min(flat_index, len(self.pos) - 1)] if self.pos else 0
        return self.raw.count("\n", 0, index) + 1

    def raw_span(self, start: int, end: int) -> Tuple[int, int]:
        """The raw span of flat[start:end] (end exclusive)."""
        return self.pos[start], self.pos[end - 1] + 1


_INLINE_TAG = re.compile(r"</?(?:strong|b|em|i|code|span)(?:\s[^<>]*)?>", re.I)
_ANY_TAG = re.compile(r"<[^<>]{0,2000}>")


def make_view(rel: str, raw: str, all_tags: bool = False) -> View:
    """Whitespace runs -> one space; `**` and inline HTML tags (`<strong>`, `<b>`,
    `<code>`, ...) dropped, so neither splits a number from its unit; `\\` +
    newline (a Rust string continuation) and an escaped `\\n` / `\\t` are
    whitespace. With `all_tags`, every tag is whitespace: a number in one `<div>`
    and its unit in the next read as one count."""
    flat: List[str] = []
    pos: List[int] = []
    i = 0
    n = len(raw)
    pending_space = -1
    while i < n:
        c = raw[i]
        if c == "<":
            tag = (_ANY_TAG if all_tags else _INLINE_TAG).match(raw, i)
            if tag:
                if all_tags and pending_space < 0:
                    pending_space = i
                i = tag.end()
                continue
        if c == "\\" and i + 1 < n and raw[i + 1] in "\n\r":
            if pending_space < 0:
                pending_space = i
            i += 1
            continue
        if c == "\\" and i + 1 < n and raw[i + 1] in "nt":
            if pending_space < 0:
                pending_space = i
            i += 2
            continue
        if c == "*" and i + 1 < n and raw[i + 1] == "*":
            i += 2
            continue
        if c.isspace():
            if pending_space < 0:
                pending_space = i
            i += 1
            continue
        if pending_space >= 0:
            if flat:
                flat.append(" ")
                pos.append(pending_space)
            pending_space = -1
        flat.append(c)
        pos.append(i)
        i += 1
    return View(rel, raw, "".join(flat), pos)


# ---------------------------------------------------------------------------
# Manifest
# ---------------------------------------------------------------------------


class Manifest:
    def __init__(self, data: dict, repo: Path):
        self.data = data
        self.caps: Dict[str, dict] = data["capabilities"]
        self.rows: Dict[str, dict] = {row["id"]: row for row in data["networks"]}
        self.families: List[str] = list(data["families"]["slugs"])
        registry = json.loads((repo / "config" / "supported_tokens.json").read_text(encoding="utf-8"))
        self.spellings: Dict[str, str] = {}  # lowercase spelling -> id
        for nid, row in self.rows.items():
            names = {nid, row["displayName"], nid.replace("-", " ")}
            names.update(ALIASES.get(nid, ()))
            for group in registry.values():
                if isinstance(group, dict) and isinstance(group.get(nid), dict):
                    if group[nid].get("displayName"):
                        names.add(group[nid]["displayName"])
            for name in names:
                self.spellings.setdefault(name.lower(), nid)
        self.family_spellings: Dict[str, str] = {}
        for slug, names in FAMILY_NAMES.items():
            for name in names + (slug,):
                self.family_spellings[name.lower()] = slug

    def value(self, capability: str, fld: str) -> int:
        if capability == "families":
            return int(self.data["families"]["count"])
        return int(self.caps[capability][fld])

    def subset(
        self, capability: str, subset: str, family: Optional[str] = None, exclude: Sequence[str] = ()
    ) -> List[str]:
        cap = self.caps[capability]
        ids = {
            "all": cap["networks"],
            "mainnet": cap["mainnet_networks"],
            "testnet": cap["testnet_networks"],
        }[subset]
        if family:
            ids = [i for i in ids if self.rows[i]["family"] == family]
        if exclude:
            ids = [i for i in ids if self.rows[i]["family"] not in exclude]
        return list(ids)

    def candidate_sets(self) -> List[Tuple[str, frozenset]]:
        out = []
        for key in self.caps:
            for subset in ("all", "mainnet", "testnet"):
                for family in [None] + self.families:
                    ids = frozenset(self.subset(key, subset, family))
                    if len(ids) >= MIN_LIST:
                        label = f"{key}.{subset}" + (f".{family}" if family else "")
                        out.append((label, ids))
        return out


def load_manifest(repo: Path) -> Manifest:
    path = repo / MANIFEST_REL
    return Manifest(json.loads(path.read_text(encoding="utf-8")), repo)


# ---------------------------------------------------------------------------
# Findings
# ---------------------------------------------------------------------------


@dataclass
class Finding:
    rel: str
    line: int
    kind: str
    text: str
    message: str
    # --write: (raw_start, raw_end, replacement)
    fix: Optional[Tuple[int, int, str]] = None


@dataclass
class Occurrence:
    rel: str
    line: int
    kind: str  # count | list | value | set
    status: str  # bound:<cap> | loose | excepted | unbound | ...
    text: str


@dataclass
class Report:
    findings: List[Finding] = field(default_factory=list)
    seen: List[Occurrence] = field(default_factory=list)
    used_exceptions: Set[Tuple[str, str]] = field(default_factory=set)


# ---------------------------------------------------------------------------
# Phrase machinery
# ---------------------------------------------------------------------------
_NUMBER_FIELDS = ("count", "mainnets", "testnets", "families")


def _accent(pattern: str) -> str:
    """Each non-ASCII letter also matches its JSON/JS escape (the landing's
    Spanish dictionary writes `reputaci` + escape + `n`)."""
    out = []
    for ch in pattern:
        if ord(ch) > 127:
            out.append("(?:" + ch + "|" + "\\\\" + "u" + format(ord(ch), "04x") + ")")
        else:
            out.append(ch)
    return "".join(out)


def compile_phrase(phrase: Phrase) -> re.Pattern:
    pattern = _accent(phrase.pattern)
    for fld in _NUMBER_FIELDS:
        pattern = pattern.replace("{" + fld + "}", r"(?P<" + fld + r">\d+)")
    pattern = pattern.replace("{list}", r"(?P<list>[^\n]{1,700}?)")
    return re.compile(r"(?<![\w-])" + pattern, re.I)


_COMPILED = [(p, compile_phrase(p)) for p in PHRASES]

_PAREN = re.compile(r"\s*\([^()]*(?:\([^()]*\)[^()]*)*\)")


def split_list(text: str) -> List[str]:
    """Items of a written list: parentheticals, backticks and a leading `native`
    dropped; separated by commas and a final and/or/y/o."""
    text = _PAREN.sub("", text)
    text = text.replace("`", "")
    parts = re.split(r"\s*,\s*(?:(?:and|or|y|o)\s+)?|\s+(?:and|or|y|o|\+)\s+", text.strip())
    return [p.strip() for p in parts if p.strip()]


def spell_list(items: Sequence[str], template: str) -> str:
    """Join `items` the way `template` (the list being replaced) joins its own."""
    if re.search(r",\s+and\s+\S+\s*$", template):
        last = ", and "
    elif re.search(r"\s+and\s+\S+\s*$", template):
        last = " and "
    elif re.search(r"\s+y\s+\S+\s*$", template):
        last = " y "
    elif re.search(r"\s+or\s+\S+\s*$", template):
        last = " or "
    else:
        last = ", "
    if len(items) == 1:
        return items[0]
    return ", ".join(items[:-1]) + last + items[-1]


def render_items(manifest: Manifest, ids: Sequence[str], style: str, original: Sequence[str]) -> List[str]:
    if style == "short":
        return [i.split("-")[0] for i in ids]
    if style == "code":
        return ["`" + i + "`" for i in ids]
    if style == "ids" or (original and all(o == o.lower() and " " not in o for o in original)):
        return list(ids)
    # Keep the spelling the text already used for a network it still names.
    used = {manifest.spellings.get(o.lower()): o for o in original}
    return [used.get(i) or manifest.rows[i]["displayName"] for i in ids]


def resolve_items(manifest: Manifest, items: Iterable[str]) -> Tuple[List[str], List[str]]:
    ids, unknown = [], []
    for item in items:
        key = item.strip().strip("*`").strip().lower()
        nid = manifest.spellings.get(key)
        if nid is None:
            nid = manifest.spellings.get(re.sub(r"^(?:the|native)\s+", "", key))
        if nid is None:
            unknown.append(item)
        else:
            ids.append(nid)
    return ids, unknown


# ---------------------------------------------------------------------------
# The check
# ---------------------------------------------------------------------------


def covered_files(repo: Path) -> List[str]:
    tracked = subprocess.run(
        ["git", "ls-files"], cwd=repo, capture_output=True, text=True, check=False
    ).stdout.split("\n")
    if not any(tracked):
        tracked = [p.relative_to(repo).as_posix() for p in repo.rglob("*") if p.is_file()]
    out = []
    for rel in tracked:
        if not rel:
            continue
        if any(fnmatch.fnmatchcase(rel, g) and rel.count("/") == g.count("/") for g in COVERED):
            path = repo / rel
            if path.is_file() and path.suffix.lower() not in (".png", ".ico", ".woff2", ".svg"):
                out.append(rel)
    return sorted(set(out))


def check_text(manifest: Manifest, rel: str, raw: str, report: Report) -> None:
    view = make_view(rel, raw)
    flat = view.flat
    taken: List[Tuple[int, int]] = []

    # 1. Phrases, longest first.
    matches = []
    for phrase, rx in _COMPILED:
        for m in rx.finditer(flat):
            matches.append((m.end() - m.start(), m.start(), phrase, m))
    matches.sort(key=lambda t: (-t[0], t[1]))
    for _length, start, phrase, m in matches:
        if any(start < e and m.end() > s for s, e in taken):
            continue
        taken.append((m.start(), m.end()))
        line = view.line(m.start())
        report.seen.append(Occurrence(rel, line, "count", f"bound:{phrase.capability}", m.group(0)))
        for fld in _NUMBER_FIELDS:
            if fld in m.re.groupindex and m.group(fld) is not None:
                expected = manifest.value(phrase.capability, fld)
                got = int(m.group(fld))
                if got != expected:
                    s, e = view.raw_span(m.start(fld), m.end(fld))
                    report.findings.append(
                        Finding(
                            rel, line, "wrong-count", m.group(0),
                            f"says {got} for {phrase.capability}.{fld}; the manifest says {expected}",
                            (s, e, str(expected)),
                        )
                    )
        if "list" in m.re.groupindex:
            check_bound_list(manifest, view, phrase, m, report)

    # 2. Bare values.
    bound_raw = [view.raw_span(s, e) for s, e in taken]
    for vrel, pattern, capability, fld in VALUE_BINDINGS:
        if vrel != rel:
            continue
        found = False
        for m in re.finditer(pattern, raw):
            found = True
            bound_raw.append((m.start(), m.end()))
            expected = manifest.value(capability, fld)
            line = raw.count("\n", 0, m.start()) + 1
            report.seen.append(Occurrence(rel, line, "value", f"bound:{capability}", m.group(0)))
            if int(m.group(fld)) != expected:
                report.findings.append(
                    Finding(rel, line, "wrong-count", m.group(0),
                            f"says {m.group(fld)} for {capability}.{fld}; the manifest says {expected}",
                            (m.start(fld), m.end(fld), str(expected)))
                )
        if not found:
            report.findings.append(Finding(rel, 1, "missing", pattern, "a bound value is gone"))

    # 3. Loose counts: the number before its unit, after it in parentheses, or
    # after a label; in HTML also with every tag read as a space, where a card
    # puts the number and its unit in two elements.
    views = [view] + ([make_view(rel, raw, all_tags=True)] if rel.endswith(".html") else [])
    reported: Set[int] = set()
    for v in views:
        for pattern in LOOSE_PATTERNS:
            for m in pattern.finditer(v.flat):
                number = v.pos[m.start(1)]
                if number in reported or any(s <= number < e for s, e in bound_raw):
                    continue
                reported.add(number)
                line = v.line(m.start())
                if _excepted(rel, m.group(0), report, "count"):
                    report.seen.append(Occurrence(rel, line, "count", "excepted", m.group(0)))
                    continue
                report.seen.append(Occurrence(rel, line, "count", "loose", m.group(0)))
                report.findings.append(
                    Finding(rel, line, "loose-count", m.group(0),
                            "a number of networks bound to no capability: say it in one of PHRASES "
                            "(scripts/network_copy.py) or drop the number")
                )

    # 4. Unbound lists.
    check_unbound_lists(manifest, view, taken, report)

    # 5. One-per-row sets.
    for sb in SET_BINDINGS:
        if sb.rel != rel:
            continue
        text = raw.replace("\r\n", "\n")
        a = text.find(sb.start)
        if a < 0:
            report.findings.append(Finding(rel, 1, "missing", sb.start.strip(), "a bound table or grid is gone"))
            continue
        b = text.find(sb.end, a + len(sb.start))
        block = text[a: b if b > 0 else len(text)]
        names = [m.group("id").strip() for m in re.finditer(sb.pattern, block, re.M | re.S)]
        ids, unknown = resolve_items(manifest, names)
        expected = manifest.subset(sb.capability, sb.subset, exclude=sb.exclude_families)
        line = text.count("\n", 0, a) + 2
        label = f"{sb.capability}.{sb.subset}" + (
            f" without {', '.join(sb.exclude_families)}" if sb.exclude_families else ""
        )
        report.seen.append(Occurrence(rel, line, "set", f"bound:{label}", sb.start.strip()))
        problems = _set_problems(ids, unknown, expected)
        if problems:
            report.findings.append(
                Finding(rel, line, "wrong-set", sb.start.strip(),
                        f"one row/card per {label} network: {problems}")
            )

    # 6. Link previews.
    if rel.startswith("static/") and rel.endswith(".html"):
        check_link_preview(rel, raw, report)


def _set_problems(ids: Sequence[str], unknown: Sequence[str], expected: Sequence[str]) -> str:
    out = []
    if unknown:
        out.append("unknown " + ", ".join(unknown))
    missing = [i for i in expected if i not in ids]
    extra = sorted({i for i in ids if i not in expected})
    dupes = sorted({i for i in ids if ids.count(i) > 1})
    if missing:
        out.append("missing " + ", ".join(missing))
    if extra:
        out.append("not in the capability " + ", ".join(extra))
    if dupes:
        out.append("twice " + ", ".join(dupes))
    return "; ".join(out)


def check_bound_list(manifest: Manifest, view: View, phrase: Phrase, m: re.Match, report: Report) -> None:
    text = m.group("list")
    items = split_list(text)
    ids, unknown = resolve_items(manifest, items)
    family = PHRASE_FAMILY.get(phrase.pattern)
    expected = manifest.subset(phrase.capability, phrase.subset, family)
    line = view.line(m.start("list"))
    report.seen.append(Occurrence(view.rel, line, "list", f"bound:{phrase.capability}.{phrase.subset}", text))
    problems = _set_problems(ids, unknown, expected)
    if not problems:
        return
    fix = None
    if not unknown and not _PAREN.search(text) and ("`" not in text or phrase.style == "code"):
        rendered = spell_list(render_items(manifest, expected, phrase.style, items), text)
        s, e = view.raw_span(m.start("list"), m.end("list"))
        fix = (s, e, rendered)
    report.findings.append(
        Finding(view.rel, line, "wrong-list", text,
                f"must name exactly {phrase.capability}.{phrase.subset}"
                + (f".{family}" if family else "") + f": {problems}"
                + ("" if fix else " (annotated list: fix it by hand)"),
                fix)
    )


def _vocab_regex(manifest: Manifest) -> re.Pattern:
    names = sorted(set(manifest.spellings) | set(manifest.family_spellings), key=len, reverse=True)
    item = r"(?:`)?(?:native )?(?:" + "|".join(re.escape(n) for n in names) + r")(?:`)?(?:\s*\([^()]{0,200}\))?"
    # A comma needs a space after it: `solana,near,stellar` is a cargo feature
    # list, not a list of networks.
    sep = r"(?:\s*,\s+(?:(?:and|or|y|o)\s+)?|\s+(?:and|or|y|o|\+)\s+)"
    return re.compile(r"(?<![\w-])" + item + r"(?:" + sep + item + r"){%d,}(?![\w-])" % (MIN_LIST - 1), re.I)


def check_unbound_lists(manifest: Manifest, view: View, taken: List[Tuple[int, int]], report: Report) -> None:
    rx = _vocab_regex(manifest)
    candidates = None
    for m in rx.finditer(view.flat):
        if any(m.start() < e and m.end() > s for s, e in taken):
            continue
        text = m.group(0)
        items = split_list(text)
        line = view.line(m.start())
        if _excepted(view.rel, text, report, "list"):
            report.seen.append(Occurrence(view.rel, line, "list", "excepted", text))
            continue
        if ILLUSTRATION.search(view.flat[max(0, m.start() - 40): m.start()]):
            report.seen.append(Occurrence(view.rel, line, "list", "illustration", text))
            continue
        lowered = [re.sub(r"^native\s+", "", i.strip("*`"), flags=re.I).lower() for i in items]
        if all(i in manifest.family_spellings for i in lowered):
            slugs = [manifest.family_spellings[i] for i in lowered]
            families = set(manifest.families)
            # Every family, or every family but EVM ("the equivalent primitive
            # on ..."); a list that names EVM or SVM claims the whole set.
            whole = set(slugs) == families or set(slugs) == families - {"evm"}
            if whole and len(slugs) == len(set(slugs)):
                report.seen.append(Occurrence(view.rel, line, "list", "bound:families", text))
                continue
            if any(i in ("evm", "svm") for i in lowered):
                report.seen.append(Occurrence(view.rel, line, "list", "bound:families", text))
                report.findings.append(
                    Finding(view.rel, line, "wrong-list", text,
                            f"a list of chain families must name the {len(manifest.families)} the "
                            f"payment networks span ({', '.join(manifest.families)}): "
                            + _set_problems(slugs, [], manifest.families))
                )
                continue
        ids, unknown = resolve_items(manifest, items)
        idset = frozenset(ids)
        if candidates is None:
            candidates = manifest.candidate_sets()
        hits = [label for label, s in candidates if s == idset]
        if hits and not unknown and len(ids) == len(idset):
            report.seen.append(Occurrence(view.rel, line, "list", "matches:" + hits[0], text))
            continue
        closest = min(candidates, key=lambda t: len(t[1] ^ idset))
        report.seen.append(Occurrence(view.rel, line, "list", "unbound", text))
        report.findings.append(
            Finding(view.rel, line, "unbound-list", text,
                    f"names {len(idset)} networks that are no capability's set (closest: {closest[0]}: "
                    + _set_problems(ids, unknown, sorted(closest[1])) + "); bind it to a phrase, "
                    "make it a capability's set, or name it in PENDING with its reason")
        )


_META_TAG = re.compile(r"<meta\b[^>]*>", re.I)
_ATTR = re.compile(r"""([\w:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')""")
_TITLE = re.compile(r"<title[^>]*>(?P<content>[^<]*)</title>", re.I)
# A preview image, under any of the keys platforms read; og:image:alt is text.
_IMAGE_KEYS = ("og:image", "og:image:url", "og:image:secure_url", "twitter:image", "twitter:image:src")


def check_link_preview(rel: str, raw: str, report: Report) -> None:
    """A link preview never states a number of networks (it is cached by every
    platform that unfurls it and ages there) and its image is the logo."""
    entries: List[Tuple[int, str, str]] = []
    for tag in _META_TAG.finditer(raw):
        attrs = {a.group(1).lower(): a.group(2) if a.group(2) is not None else a.group(3)
                 for a in _ATTR.finditer(tag.group(0))}
        key = (attrs.get("property") or attrs.get("name") or "").lower()
        if (key.startswith(("og:", "twitter:")) or key == "description") and "content" in attrs:
            entries.append((tag.start(), key, attrs["content"]))
    entries += [(m.start(), "title", m.group("content")) for m in _TITLE.finditer(raw)]
    for start, key, content in entries:
        line = raw.count("\n", 0, start) + 1
        if key in _IMAGE_KEYS:
            if content != LOGO:
                report.findings.append(
                    Finding(rel, line, "preview", content,
                            f"{key} must be {LOGO}: previews are network-agnostic (static/LANDING_PAGE.md)")
                )
            continue
        flat = make_view(rel, content).flat
        if any(p.search(flat) for p in LOOSE_PATTERNS):
            report.findings.append(
                Finding(rel, line, "preview", content,
                        f"{key} states a number of networks: a link preview is cached where it is "
                        "shared and ages there; say it without the number")
            )


def _same_text(a: str, b: str) -> bool:
    return " ".join(a.split()).lower() == " ".join(b.split()).lower()


def _same_items(a: str, b: str) -> bool:
    def items(text: str) -> Set[str]:
        return {i.strip("*` ").lower() for i in split_list(text)}

    return items(a) == items(b)


def _excepted(rel: str, text: str, report: Report, kind: str) -> bool:
    """A PENDING text excuses exactly itself: a count by its words, a list by its
    set of items, so a list that gains or loses a network is judged again."""
    same = _same_items if kind == "list" else _same_text
    for key in PENDING:
        if key[0] == rel and same(key[1], text):
            report.used_exceptions.add(key)
            return True
    return False


# ---------------------------------------------------------------------------
# Production's profile, held to the Terraform
# ---------------------------------------------------------------------------
# Which variable lets a family's provider exist (src/chain/*::from_env). An
# EVM, SVM or NEAR provider needs its RPC URL; Stellar, Algorand and Sui need
# their key; Hedera its switch; XRPL builds a relay provider with none.
KEY_GATES = {
    "stellar": "STELLAR_PRIVATE_KEY_{env}",
    "algorand": "ALGORAND_MNEMONIC_{env}",
    "sui": "SUI_PRIVATE_KEY_{env}",
}
FLAG_GATES = {"hedera": "HEDERA_ENABLED_{env}"}
NO_GATE = ("xrpl",)
RPC_GATED = ("evm", "svm", "near")
NEAR_ESCROW_VARS = {"near": "NEAR_ESCROW_CONTRACTS_MAINNET", "near-testnet": "NEAR_ESCROW_CONTRACTS_TESTNET"}


def _tf_scalars(text: str) -> Dict[str, str]:
    out = {}
    for m in re.finditer(r"^\s*([a-z_][a-z0-9_]*)\s*=\s*(true|false|\"[^\"\n]*\"|-?\d+)\s*(?:#.*)?$", text, re.M):
        out[m.group(1)] = m.group(2).strip('"')
    return out


def _conditional_spans(text: str) -> List[Tuple[int, int, str]]:
    """(open, close, flag) of every `var.flag ? [ ... ]` / `var.flag ? { ... }`:
    the bracket that opens the true branch to the one that closes it."""
    spans = []
    for m in re.finditer(r"var\.(\w+)\s*\?\s*[\[{]", text):
        depth, j = 0, m.end() - 1
        while j < len(text):
            if text[j] in "[{":
                depth += 1
            elif text[j] in "]}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        spans.append((m.end() - 1, j, m.group(1)))
    return spans


def terraform_env(tf_dir: Path) -> Dict[str, Optional[str]]:
    """Every variable the facilitator's task definition declares: name -> its
    literal value (None for a secret). The balances Lambda's environment is not
    the task's and is skipped; a declaration inside `var.x ? [{ ... }]` counts
    only when production.auto.tfvars sets x to true."""
    tfvars: Dict[str, str] = {}
    defaults: Dict[str, str] = {}
    files = sorted(tf_dir.glob("*.tf"))
    for f in files:
        for m in re.finditer(r'variable\s+"(\w+)"\s*\{(.*?)\n\}', f.read_text(encoding="utf-8"), re.S):
            d = re.search(r"^\s*default\s*=\s*(true|false|\"[^\"\n]*\"|-?\d+)", m.group(2), re.M)
            if d:
                defaults[m.group(1)] = d.group(1).strip('"')
    for f in sorted(tf_dir.glob("*.auto.tfvars")):
        tfvars.update(_tf_scalars(f.read_text(encoding="utf-8")))

    def var(name: str) -> Optional[str]:
        return tfvars.get(name, defaults.get(name))

    env: Dict[str, Optional[str]] = {}
    for f in files:
        if f.name == "lambda-balances.tf":
            continue
        text = f.read_text(encoding="utf-8")
        switched = _conditional_spans(text)
        for m in re.finditer(r'\bname\s*=\s*"([A-Z][A-Z0-9_]*)"', text):
            # Inside `var.x ? [ ... ]` (any entry, at any distance) it counts only
            # when x is true.
            if any(s < m.start() < e and var(flag) != "true" for s, e, flag in switched):
                continue
            after = text[m.end(): m.end() + 400].split("}")[0]
            literal = re.search(r'\bvalue\s*=\s*"([^"]*)"', after)
            via_var = re.search(r"\bvalue\s*=\s*tostring\(var\.(\w+)\)", after)
            if literal:
                env[m.group(1)] = literal.group(1)
            elif via_var:
                env[m.group(1)] = var(via_var.group(1))
            else:
                env[m.group(1)] = None
    return env


def check_profile(manifest: Manifest, repo: Path, report: Report) -> None:
    tf_dir = repo / TERRAFORM_REL
    rel = MANIFEST_REL
    if not tf_dir.is_dir():
        report.findings.append(Finding(rel, 1, "profile", TERRAFORM_REL, "the production Terraform is missing"))
        return
    env = terraform_env(tf_dir)
    for name, value in manifest.data["profile"]["env"].items():
        got = env.get(name, "<absent>")
        if got != value:
            report.findings.append(
                Finding(rel, 1, "profile", name,
                        f"the manifest counts with {name}={value}; production's Terraform sets {got}")
            )
    payments = set(manifest.caps["payments"]["networks"])
    for nid, row in manifest.rows.items():
        family = row["family"]
        which = "TESTNET" if row["testnet"] else "MAINNET"
        if family in RPC_GATED:
            gate, ok = row["rpcEnv"], row["rpcEnv"] in env
        elif family in KEY_GATES:
            gate = KEY_GATES[family].format(env=which)
            ok = gate in env
        elif family in FLAG_GATES:
            gate = FLAG_GATES[family].format(env=which)
            ok = env.get(gate) == "true"
        elif family in NO_GATE:
            gate, ok = "(none)", True
        else:
            report.findings.append(Finding(rel, 1, "profile", nid, f"no provider gate known for family {family}"))
            continue
        if ok != (nid in payments):
            report.findings.append(
                Finding(rel, 1, "profile", nid,
                        f"{nid}: production's Terraform {'configures' if ok else 'does not configure'} "
                        f"its provider ({gate}) but exactServed in config/supported_tokens.json says "
                        f"{'it takes' if nid in payments else 'it does not take'} payments")
            )
    for nid in manifest.caps["escrow_near"]["networks"]:
        name = NEAR_ESCROW_VARS.get(nid)
        if not name or not env.get(name):
            report.findings.append(
                Finding(rel, 1, "profile", nid, f"escrow_near counts {nid}, but production sets no {name}")
            )


def check_features(manifest: Manifest, repo: Path, report: Report) -> None:
    """The manifest is computed under one feature set: the one CI tests, the
    image ships and --generate compiles. Four places spell it; they agree."""
    expected = set(manifest.data["profile"]["features"])
    spelled: List[Tuple[str, str]] = [("scripts/network_copy.py CARGO_FEATURES", CARGO_FEATURES)]
    ci = repo / ".github" / "workflows" / "ci.yaml"
    if ci.is_file():
        spelled += [(".github/workflows/ci.yaml", m.group(1))
                    for m in re.finditer(r"^\s*CARGO_FEATURES:\s*(\S+)", ci.read_text(encoding="utf-8"), re.M)]
    docker = repo / "Dockerfile"
    if docker.is_file():
        spelled += [("Dockerfile", m.group(1))
                    for m in re.finditer(r"--features\s+(\S+)", docker.read_text(encoding="utf-8"))]
    for source in (".github/workflows/ci.yaml", "Dockerfile"):
        if not any(where == source for where, _ in spelled):
            report.findings.append(Finding(MANIFEST_REL, 1, "profile", "features",
                                           f"{source} no longer names the features it builds with"))
    for where, value in spelled:
        if set(value.split(",")) != expected:
            report.findings.append(
                Finding(MANIFEST_REL, 1, "profile", where,
                        f"{where} builds with {value}; the manifest is computed under "
                        f"{','.join(sorted(expected))}")
            )


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def run_check(repo: Path, files: Optional[List[str]] = None) -> Tuple[Report, Manifest]:
    manifest = load_manifest(repo)
    report = Report()
    for rel in files if files is not None else covered_files(repo):
        # Bytes, not read_text: a CRLF checkout keeps its offsets, so --write
        # replaces exactly the digits it found.
        raw = (repo / rel).read_bytes().decode("utf-8")
        check_text(manifest, rel, raw, report)
    if files is None:
        for key, reason in PENDING.items():
            if key not in report.used_exceptions:
                report.findings.append(
                    Finding(key[0], 1, "stale-exception", key[1],
                            f"PENDING names a text that no longer occurs ({reason[:60]}...): remove it")
                )
        check_profile(manifest, repo, report)
        check_features(manifest, repo, report)
    return report, manifest


def apply_fixes(repo: Path, report: Report) -> List[str]:
    by_file: Dict[str, List[Tuple[int, int, str]]] = {}
    for f in report.findings:
        if f.fix and f.rel not in GENERATED:
            by_file.setdefault(f.rel, []).append(f.fix)
    for rel, fixes in by_file.items():
        path = repo / rel
        raw = path.read_bytes().decode("utf-8")
        for s, e, text in sorted(set(fixes), reverse=True):
            raw = raw[:s] + text + raw[e:]
        path.write_bytes(raw.encode("utf-8"))
    return sorted(by_file)


def rebuild_generated(repo: Path) -> None:
    """static/llms-full.txt is the four agent documents joined: rebuild it from
    them every time (idempotent), so an edit by hand is carried too."""
    subprocess.run(["bash", "scripts/build_llms_full.sh"], cwd=repo, check=True)


def generate_manifest(repo: Path) -> None:
    env = dict(os.environ)
    env[WRITE_ENV] = "1"
    cmd = [
        "cargo", "test", "--locked", "-p", "x402-rs", "--features", CARGO_FEATURES, "--lib",
        MANIFEST_TEST, "--", "--exact", "--test-threads=1",
    ]
    print("+ " + " ".join(cmd), file=sys.stderr)
    out = subprocess.run(cmd, cwd=repo, env=env, capture_output=True, text=True)
    sys.stderr.write(out.stdout[-2000:] + out.stderr[-2000:])
    if out.returncode != 0 or "1 passed" not in out.stdout:
        raise SystemExit("[FAIL] the manifest generator did not run (expected `1 passed`)")


def print_findings(report: Report) -> None:
    for f in report.findings:
        print(f"{f.rel}:{f.line}: [{f.kind}] {f.message}\n    {f.text[:200]}")


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--generate", action="store_true")
    mode.add_argument("--inventory", action="store_true")
    ap.add_argument("--repo", default=str(REPO), help=argparse.SUPPRESS)
    args = ap.parse_args(argv)
    repo = Path(args.repo).resolve()

    if args.generate:
        generate_manifest(repo)
    try:
        report, manifest = run_check(repo)
    except (OSError, ValueError, KeyError) as e:
        print(f"[FAIL] could not read {MANIFEST_REL}: {e}", file=sys.stderr)
        return 2

    if args.inventory:
        print("file\tline\tkind\tstatus\ttext")
        for o in report.seen:
            print(f"{o.rel}\t{o.line}\t{o.kind}\t{o.status}\t{o.text[:160]}")
        return 0

    if args.write or args.generate:
        changed = apply_fixes(repo, report)
        rebuild_generated(repo)
        for rel in changed:
            print(f"rewrote {rel}")
        report, manifest = run_check(repo)

    caps = manifest.caps
    summary = ", ".join(
        f"{k} {caps[k]['count']} ({caps[k]['mainnets']}+{caps[k]['testnets']})" for k in caps
    )
    if report.findings:
        print_findings(report)
        print(f"\n[FAIL] {len(report.findings)} finding(s). Manifest: {summary}; "
              f"{manifest.data['families']['count']} payment families. "
              "Fix the text, or run `python scripts/network_copy.py --write` (--generate after a "
              "change of networks in the code).")
        return 1
    print(f"[OK] every published count and list of networks is the manifest's: {summary}; "
          f"{manifest.data['families']['count']} payment families.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
