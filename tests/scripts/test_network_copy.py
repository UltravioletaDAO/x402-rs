"""Tests for scripts/network_copy.py: every published count and list of networks
comes from config/network_capabilities.json.

Run: python3 tests/scripts/test_network_copy.py  (or unittest discover -p 'test_network_copy.py')
"""

from __future__ import annotations

import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import network_copy as nc  # noqa: E402

MANIFEST = nc.load_manifest(REPO)
CAPS = MANIFEST.caps
BS = "\\"  # one backslash, spelled so no editor or tool turns an escape into its character
# Neutral file names: phrases bind in every file, but the landing, the README
# and /docs also carry per-file bindings (tables, grids, a bare value) a
# snippet lacks.
PAGE = "static/page.html"
DOC = "static/index.md"


def names(capability: str, subset: str = "networks") -> list:
    return [MANIFEST.rows[i]["displayName"] for i in CAPS[capability][subset]]


def key(k: str, text: str) -> str:
    """`text` as a landing dictionary writes it, under its i18n key."""
    return f'"{k}": "{text}",'


def readme(mainnets=None, testnets=None, extra: str = "", erc8004=None, escrow=None) -> str:
    """A README with the four tables the check binds, each one row per network."""
    mainnets = names("served", "mainnet_networks") if mainnets is None else mainnets
    testnets = names("served", "testnet_networks") if testnets is None else testnets
    erc8004 = CAPS["erc8004"]["networks"] if erc8004 is None else erc8004
    escrow = names("escrow_x402r", "mainnet_networks") if escrow is None else escrow
    erc_rows = "\n".join(
        f"| {MANIFEST.rows[i]['displayName']} | {'Testnet' if MANIFEST.rows[i]['testnet'] else 'Mainnet'} | x | x |"
        for i in erc8004
    )
    return (
        "x\n### Mainnets\n\n| Network | Chain ID | Token | Explorer |\n|---|---|---|---|\n"
        + "\n".join(f"| **{n}** | 1 | USDC | x |" for n in mainnets)
        + "\n\n### Testnets\n\n| Network | Chain ID | Faucet |\n|---|---|---|\n"
        + "\n".join(f"| {n} | 1 | - |" for n in testnets)
        + "\n\n### Supported Stablecoins\n"
        + extra
        + f"\n### Supported ERC-8004 Networks ({CAPS['erc8004']['count']})\n\n"
        + "| Registry | Mainnet | Testnet |\n|---|---|---|\n| Identity | `0x1` | `0x2` |\n\n"
        + "| Network | Type | Identity Registry | Reputation Registry |\n|---|---|---|---|\n"
        + erc_rows
        + "\n\nAll EVM mainnet contracts use CREATE2 deterministic deployment.\n\n"
        + "**`escrow` scheme (PaymentOperator).** A payment.\n\n| Network | `network` | Set | Notes |\n|---|---|---|---|\n"
        + "\n".join(f"| **{n}** | `x` | per-chain | |" for n in escrow)
        + "\n| Base Sepolia | `base-sepolia` | per-chain | testnet |\n\nEvery network in this table had one.\n"
    )


def check(rel: str, text: str) -> nc.Report:
    report = nc.Report()
    nc.check_text(MANIFEST, rel, text, report)
    return report


def kinds(report: nc.Report) -> list:
    return [f.kind for f in report.findings]


def fixed(text: str, report: nc.Report) -> str:
    for s, e, new in sorted({f.fix for f in report.findings if f.fix}, reverse=True):
        text = text[:s] + new + text[e:]
    return text


class TheTreeAndTheManifest(unittest.TestCase):
    def test_every_covered_file_states_the_manifests_counts_and_lists(self):
        report, _ = nc.run_check(REPO)
        self.assertEqual(
            [f"{f.rel}:{f.line} [{f.kind}] {f.message}" for f in report.findings], []
        )

    def test_the_manifest_counts_its_own_lists(self):
        served = set(CAPS["served"]["networks"])
        for k, cap in CAPS.items():
            self.assertEqual(cap["count"], len(cap["networks"]), k)
            self.assertEqual(cap["mainnets"] + cap["testnets"], cap["count"], k)
            self.assertEqual(cap["mainnets"], len(cap["mainnet_networks"]), k)
            self.assertTrue(set(cap["networks"]) <= served, k)
        self.assertEqual(MANIFEST.data["families"]["count"], len(MANIFEST.families))

    def test_the_landing_checker_reads_its_counts_from_the_manifest(self):
        import verify_landing_canonical as vlc

        self.assertEqual(vlc.payment_mainnets(), CAPS["payments"]["mainnets"])
        self.assertEqual(vlc.escrow_mainnets(), set(CAPS["escrow_x402r"]["mainnet_networks"]))
        self.assertEqual(vlc.erc8004_networks()[1], set(CAPS["erc8004"]["networks"]))


class BoundCounts(unittest.TestCase):
    def test_a_wrong_bound_count_is_reported_with_the_manifests_number_and_fixed(self):
        text = key("x402r.networksTitle", "Escrow Deployed on 9 Networks")
        report = check(PAGE, text)
        self.assertEqual(kinds(report), ["wrong-count"])
        expected = CAPS["escrow_x402r"]["mainnets"]
        self.assertIn(f"manifest says {expected}", report.findings[0].message)
        self.assertEqual(fixed(text, report), key("x402r.networksTitle", f"Escrow Deployed on {expected} Networks"))

    def test_the_markup_form_of_a_key_is_bound_too(self):
        n = CAPS["erc8004"]["count"]
        good = f'<h4 style="x" data-i18n="erc8004.networksTitle">Deployed on {n} Networks</h4>'
        self.assertEqual(kinds(check(PAGE, good)), [])
        self.assertEqual(kinds(check(PAGE, good.replace(f" {n} ", f" {n + 1} "))), ["wrong-count"])

    def test_a_shape_without_its_capability_is_never_rewritten_with_another_ones_numbers(self):
        # "Deployed on N Networks" is ERC-8004's title only under its key: an
        # upto sentence of the same shape stays a loose count, with no rewrite.
        report = check(PAGE, f"Upto Deployed on {CAPS['upto']['count']} Networks")
        self.assertEqual(kinds(report), ["loose-count"])
        self.assertIsNone(report.findings[0].fix)
        report = check(DOC, "Upto is live on 8 mainnets, 11 networks in total.")
        self.assertEqual(kinds(report), ["loose-count", "loose-count"])
        # "ERC-8004" in the sentence, but upto is the subject of the count.
        report = check(DOC, "ERC-8004 sits next to upto, which is live on 8 mainnets and 11 networks in total.")
        self.assertEqual(kinds(report), ["loose-count", "loose-count"])
        self.assertEqual([f.fix for f in report.findings], [None, None])

    def test_each_number_of_a_phrase_is_its_own_field(self):
        e = CAPS["erc8004"]
        good = (
            f"identity and reputation across {e['count']} networks "
            f"({e['mainnets']} mainnets + {e['testnets']} testnets)."
        )
        self.assertEqual(kinds(check(DOC, good)), [])
        bad = good.replace(f"+ {e['testnets']} testnets", f"+ {e['testnets'] - 1} testnets")
        report = check(DOC, bad)
        self.assertEqual(kinds(report), ["wrong-count"])
        self.assertIn("erc8004.testnets", report.findings[0].message)

    def test_bold_markdown_and_inline_html_do_not_hide_a_number(self):
        e = CAPS["erc8004"]["count"]
        self.assertEqual(
            kinds(check(DOC, f"identity and reputation across **{e + 1} networks** (x).")), ["wrong-count"]
        )
        self.assertEqual(
            kinds(check(PAGE, f"identity and reputation across <strong>{e + 1}</strong> networks.")),
            ["wrong-count"],
        )

    def test_a_spanish_phrase_written_with_json_escapes_is_bound(self):
        e = CAPS["erc8004"]["count"]
        esc = BS + "u00f3"
        # No "(EVM + Solana)" after the count: that phrase alone would bind it.
        good = f'"x": "registros de identidad, reputaci{esc}n y validaci{esc}n en {e} redes."'
        self.assertEqual(kinds(check(PAGE, good)), [])
        bad = good.replace(f"en {e} redes", f"en {e - 1} redes")
        self.assertEqual(kinds(check(PAGE, bad)), ["wrong-count"])

    def test_the_longer_phrase_wins_even_when_the_shorter_starts_first(self):
        n = CAPS["escrow_x402r"]["mainnets"]
        phrases = (
            nc.Phrase("erc8004", r"Deployed on {count} Networks"),
            nc.Phrase("escrow_x402r", r"on {mainnets} Networks of the escrow rail"),
        )
        compiled = [(p, nc.compile_phrase(p)) for p in phrases]
        with mock.patch.object(nc, "_COMPILED", compiled):
            report = check(PAGE, f"Deployed on {n} Networks of the escrow rail")
        self.assertEqual(kinds(report), [])
        self.assertEqual([o.status for o in report.seen if o.kind == "count"], ["bound:escrow_x402r"])

    def test_a_parenthesised_count_is_bound_or_loose(self):
        e = CAPS["erc8004"]["count"]
        self.assertEqual(kinds(check(DOC, f"### Supported ERC-8004 Networks ({e})")), [])
        self.assertEqual(kinds(check(DOC, f"### Supported ERC-8004 Networks ({e - 1})")), ["wrong-count"])
        self.assertEqual(kinds(check(DOC, "# Canonical mainnet payment-network count (21).")), ["loose-count"])

    def test_a_bare_value_is_bound_and_fixed(self):
        text = '<div class="stat-value" id="ovr-erc8004-networks" style="x">20</div>'
        report = check("static/index.html", text)
        self.assertIn("wrong-count", kinds(report))
        self.assertIn(f">{CAPS['erc8004']['count']}<", fixed(text, report))

    def test_a_bound_value_that_disappears_is_reported(self):
        self.assertIn("missing", kinds(check("static/index.html", "<p>nothing</p>")))


class LooseCounts(unittest.TestCase):
    def test_a_number_of_networks_bound_to_no_capability_is_reported(self):
        for text in (
            "The facilitator settles on 24 networks.",
            "It pays on **24** networks.",
            "12 EVM chains and more.",
            "Funciona en 24 redes.",
            "Networks: 45",
        ):
            with self.subTest(text=text):
                self.assertEqual(kinds(check("static/skill.md", text)), ["loose-count"])

    def test_a_number_and_its_unit_in_two_html_elements_are_one_count(self):
        for text in (
            '<p><strong style="x">45</strong> networks</p>',
            '<div class="stat-value">45</div>\n<div class="stat-label">Networks</div>',
        ):
            with self.subTest(text=text):
                self.assertEqual(kinds(check(PAGE, text)), ["loose-count"])

    def test_numbers_that_count_no_networks_pass(self):
        for text in (
            "/health/ready answers status 503 when a mainnet is down.",
            "116 chain-verified anchors on record.",
            "ERC-8004 networks have their own scope.",
            "x402 networks and eip155:8453 network ids.",
            "One chain, two or more chains.",
            "const seen = {mainnet:0, testnet:0};",
        ):
            with self.subTest(text=text):
                self.assertEqual(kinds(check("static/skill.md", text)), [])

    def test_a_rust_string_continuation_or_an_escaped_newline_hides_no_number(self):
        continued = '"The facilitator settles on ' + BS + "\n" + '        24 networks today."'
        self.assertEqual(kinds(check("src/mcp.rs", continued)), ["loose-count"])
        escaped = '"description": "Supported.' + BS + "n" + BS + "n" + '24 networks."'
        self.assertEqual(kinds(check("static/.well-known/x402", escaped)), ["loose-count"])


class Lists(unittest.TestCase):
    def test_a_bound_list_that_misses_a_network_is_reported_and_rewritten(self):
        text = key("erc8004.addressesNote", "Same addresses on Ethereum, Base and Arc. SKALE Base carries Identity.")
        report = check(PAGE, text)
        self.assertEqual(kinds(report), ["wrong-list"])
        canonical = CAPS["erc8004_canonical"]["names"]
        rewritten = fixed(text, report)
        self.assertIn("Same addresses on Ethereum, Base, ", rewritten)
        self.assertIn(" and " + canonical[-1] + ". SKALE", rewritten)
        self.assertEqual(kinds(check(PAGE, rewritten)), [])

    def test_a_spanish_bound_list_keeps_its_y(self):
        text = key("erc8004.addressesNote", "Las mismas direcciones en Ethereum y Arc. SKALE Base solo tiene Identidad.")
        rewritten = fixed(text, check(PAGE, text))
        self.assertIn(" y " + CAPS["erc8004_canonical"]["names"][-1] + ". SKALE", rewritten)

    def test_a_short_id_list_is_rewritten_in_short_ids(self):
        e = CAPS["erc8004"]
        text = key("endpoints.erc8004Note", f"{e['count']} networks: ethereum, base + testnets.")
        rewritten = fixed(text, check(PAGE, text))
        short = [i.split("-")[0] for i in e["mainnet_networks"]]
        self.assertIn(", ".join(short) + " + testnets", rewritten)
        self.assertIn("skale", short)

    def test_a_code_list_is_rewritten_in_code(self):
        x = CAPS["escrow_x402r"]
        text = f"**Escrow networks ({x['count']}):** `base`, `ethereum`. Each lists `commerce`."
        rewritten = fixed(text, check(DOC, text))
        self.assertIn(", ".join(f"`{i}`" for i in x["networks"][:-1]), rewritten)
        self.assertEqual(kinds(check(DOC, rewritten)), [])

    def test_an_unknown_name_in_an_annotated_list_is_reported_without_a_rewrite(self):
        text = (
            "### EVM Chains (Testnet) Base Sepolia, Monad Testnet (`x`), Arc Testnet "
            "### SVM Chains"
        )
        report = check(DOC, text)
        self.assertEqual(kinds(report), ["wrong-list"])
        self.assertIn("unknown Monad Testnet", report.findings[0].message)
        self.assertIsNone(report.findings[0].fix)

    def test_an_unbound_list_passes_only_as_a_whole_capability(self):
        relay = ", ".join(CAPS["erc8004_relay"]["networks"])
        self.assertEqual(kinds(check("static/skill.md", f"Relayed on {relay}.")), [])
        partial = ", ".join(CAPS["erc8004_relay"]["networks"][:-1])
        self.assertEqual(kinds(check("static/skill.md", f"Relayed on {partial}.")), ["unbound-list"])

    def test_an_example_list_and_a_cargo_feature_list_claim_no_set(self):
        for text in (
            "Network name (e.g., ethereum, base, solana, solana-devnet)",
            "Por ejemplo: base, polygon, celo, monad.",
            "cargo run --features solana,near,stellar,algorand",
        ):
            with self.subTest(text=text):
                self.assertEqual(kinds(check("src/mcp.rs", text)), [])

    def test_a_list_of_families_must_name_every_family(self):
        self.assertEqual(
            kinds(check("CLAUDE.md", "Families: EVM, Solana, NEAR, Stellar, Algorand, Sui, XRPL.")),
            ["wrong-list"],
        )
        self.assertEqual(
            kinds(check("CLAUDE.md", "EVM, SVM, NEAR, Stellar, Sui, Algorand, XRPL and native Hedera.")),
            [],
        )
        self.assertEqual(
            kinds(check("CLAUDE.md", "EVM + Solana + NEAR + Stellar are the families.")),
            ["wrong-list"],
        )

    def test_every_family_but_evm_is_a_whole_set(self):
        text = "(or the equivalent primitive on Solana, NEAR, Stellar, Sui, Algorand, XRPL and Hedera)"
        self.assertEqual(kinds(check("static/auth.md", text)), [])
        short = text.replace(" and Hedera", "")
        self.assertEqual(kinds(check("static/auth.md", short)), ["unbound-list"])


class Tables(unittest.TestCase):
    def test_the_readme_tables_hold_one_row_per_network(self):
        mains = names("served", "mainnet_networks")
        tests = names("served", "testnet_networks")
        self.assertEqual(kinds(check("README.md", readme())), [])
        report = check("README.md", readme(mainnets=mains[1:]))
        self.assertEqual(kinds(report), ["wrong-set"])
        self.assertIn("missing " + CAPS["served"]["mainnet_networks"][0], report.findings[0].message)
        self.assertEqual(kinds(check("README.md", readme(testnets=tests + ["Monad Testnet"]))), ["wrong-set"])
        self.assertEqual(kinds(check("README.md", readme(mainnets=mains + mains[:1]))), ["wrong-set"])
        self.assertEqual(kinds(check("README.md", readme(erc8004=CAPS["erc8004"]["networks"][1:]))), ["wrong-set"])
        self.assertEqual(
            kinds(check("README.md", readme(escrow=names("escrow_x402r", "mainnet_networks")[1:]))), ["wrong-set"]
        )

    def test_a_table_that_disappears_is_reported(self):
        self.assertEqual(kinds(check("README.md", "no tables")), ["missing"] * 4)

    def test_docs_name_every_network_of_the_families_that_are_not_evm(self):
        ids = [i for i in CAPS["payments"]["networks"] if MANIFEST.rows[i]["family"] != "evm"]

        def block(chosen):
            bullets = "\n".join(f"- **X**: Mainnet (`{i}`)" for i in chosen)
            return f"x\n### SVM Chains\n{bullets} not the EVM `xrpl-evm` chain\n\nArc uses USDC.\n"

        self.assertEqual([k for k in kinds(check("src/openapi.rs", block(ids))) if k != "missing"], [])
        self.assertIn("wrong-set", kinds(check("src/openapi.rs", block(ids[1:]))))


class LinkPreviews(unittest.TestCase):
    def page(self, description: str, image: str = nc.LOGO) -> str:
        return (
            f'<meta property="og:description" content="{description}">\n'
            f'<meta property="og:image" content="{image}">\n<title>x</title>'
        )

    def test_a_preview_states_no_number_of_networks_and_shows_the_logo(self):
        self.assertEqual(kinds(check("static/x402.html", self.page("Gasless x402."))), [])
        self.assertEqual(
            kinds(check("static/x402.html", self.page("Settles on 24 networks."))), ["loose-count", "preview"]
        )
        self.assertEqual(
            kinds(check("static/x402.html", self.page("Gasless.", "https://example.com/card-24.png"))),
            ["preview"],
        )

    def test_attribute_order_and_other_image_keys_are_read(self):
        reversed_attrs = '<meta content="Settles on 24 networks." property="og:description">'
        self.assertIn("preview", kinds(check("static/x402.html", reversed_attrs)))
        card = "<meta name='twitter:image:src' content='https://example.com/card.png'>"
        self.assertEqual(kinds(check("static/x402.html", card)), ["preview"])
        unquoted = "<meta property=og:image content=https://example.com/card.png>"
        self.assertEqual(kinds(check("static/x402.html", unquoted)), ["preview"])


class Exceptions(unittest.TestCase):
    def test_an_exception_excuses_its_text_in_its_own_file_only(self):
        text = "Tokens: Arbitrum, Celo, Optimism, Monad."
        self.assertEqual(kinds(check("README.md", readme(extra=text))), [])
        self.assertEqual(kinds(check("static/skill.md", text)), ["unbound-list"])

    def test_an_exception_excuses_its_exact_list_only(self):
        grown = "Tokens: Arbitrum, Celo, Optimism, Monad, Base."
        self.assertEqual(kinds(check("README.md", readme(extra=grown))), ["unbound-list"])
        # A family list that drops Hedera is not excused by the exception that
        # names five of its families.
        stale = "The families are EVM, SVM, NEAR, Stellar, Algorand, Sui and XRPL."
        self.assertEqual(kinds(check("static/skill.md", stale)), ["wrong-list"])
        # The same five names in another order are another sentence.
        reordered = "The other families are XRPL, Sui, Algorand, Stellar and NEAR."
        self.assertEqual(kinds(check("static/skill.md", reordered)), ["unbound-list"])

    def test_an_exception_that_no_longer_occurs_is_an_error(self):
        stale = dict(nc.PENDING)
        stale[("README.md", "Zzz, Yyy, Xxx, Www")] = "a text that is gone"
        with mock.patch.object(nc, "PENDING", stale):
            report, _ = nc.run_check(REPO)
        self.assertEqual(kinds(report), ["stale-exception"])


class Profile(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.tf = self.tmp / nc.TERRAFORM_REL
        shutil.copytree(REPO / nc.TERRAFORM_REL, self.tf, ignore=shutil.ignore_patterns(".terraform*", "*.tfstate*"))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def findings(self):
        report = nc.Report()
        nc.check_profile(MANIFEST, self.tmp, report)
        return report.findings

    def edit(self, name: str, old: str, new: str):
        path = self.tf / name
        text = path.read_text(encoding="utf-8")
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    def test_production_terraform_matches_the_profile(self):
        self.assertEqual([f.message for f in self.findings()], [])

    def test_a_payment_network_production_does_not_configure_is_reported(self):
        self.edit("main.tf", '"RPC_URL_SCROLL"', '"RPC_URL_SCROLL_OFF"')
        messages = [f.message for f in self.findings()]
        self.assertEqual(len(messages), 1, messages)
        self.assertIn("scroll", messages[0])

    def test_a_conditional_declaration_counts_only_when_its_switch_is_on(self):
        env = nc.terraform_env(self.tf)
        self.assertIn("RPC_URL_ARC", env)
        self.edit("production.auto.tfvars", "arc_mainnet_enabled = true", "arc_mainnet_enabled = false")
        self.assertNotIn("RPC_URL_ARC", nc.terraform_env(self.tf))
        self.assertTrue(any("arc:" in f.message for f in self.findings()))

    def test_every_entry_of_a_switched_list_follows_its_switch_at_any_distance(self):
        filler = "  # " + "x" * 600 + "\n"
        (self.tf / "zz_switched.tf").write_text(
            'variable "x_on" {\n  default = false\n}\n\nlocals {\n  x_list = var.x_on ? [\n'
            '    { name = "RPC_URL_FIRST", value = "a" },\n' + filler
            + '    { name = "RPC_URL_SECOND", value = "b" },\n  ] : []\n}\n',
            encoding="utf-8",
        )
        env = nc.terraform_env(self.tf)
        self.assertNotIn("RPC_URL_FIRST", env)
        self.assertNotIn("RPC_URL_SECOND", env)
        with (self.tf / "production.auto.tfvars").open("a", encoding="utf-8") as f:
            f.write("\nx_on = true\n")
        env = nc.terraform_env(self.tf)
        self.assertEqual((env.get("RPC_URL_FIRST"), env.get("RPC_URL_SECOND")), ("a", "b"))

    def test_a_declaration_behind_a_condition_this_reader_cannot_evaluate_counts_as_absent(self):
        (self.tf / "zz_compound.tf").write_text(
            'locals {\n  y_list = var.a_on && var.b_on ? [\n    { name = "RPC_URL_COMPOUND", value = "a" },\n'
            '  ] : []\n  z_list = !var.c_on ? [{ name = "RPC_URL_NEGATED", value = "b" }] : []\n}\n',
            encoding="utf-8",
        )
        env = nc.terraform_env(self.tf)
        self.assertNotIn("RPC_URL_COMPOUND", env)
        self.assertNotIn("RPC_URL_NEGATED", env)

    def test_the_balances_lambda_environment_is_not_the_tasks(self):
        with (self.tf / "lambda-balances.tf").open("a", encoding="utf-8") as f:
            f.write('\nlocals {\n  lambda_extra = [{ name = "RPC_URL_ETHEREUM_SEPOLIA", value = "x" }]\n}\n')
        env = nc.terraform_env(self.tf)
        self.assertNotIn("RPC_URL_ETHEREUM_SEPOLIA", env)
        self.assertEqual(env.get("ENABLE_ZAMA"), "false")  # tostring(var.enable_zama)
        self.assertIsNone(env["RPC_URL_BASE"])  # a secret: declared, no literal

    def test_a_switch_production_turns_off_is_reported(self):
        main = (self.tf / "main.tf").read_text(encoding="utf-8")
        at = main.index('"ENABLE_UPTO"')
        new = main[:at] + main[at:].replace('value = "true"', 'value = "false"', 1)
        (self.tf / "main.tf").write_text(new, encoding="utf-8")
        self.assertTrue(any("ENABLE_UPTO" in f.message for f in self.findings()))

    def test_near_escrow_needs_its_contracts(self):
        self.edit("main.tf", '"NEAR_ESCROW_CONTRACTS_TESTNET"', '"NEAR_ESCROW_CONTRACTS_OTHER"')
        self.assertTrue(any("near-testnet" in f.message for f in self.findings()))


class Features(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / ".github" / "workflows").mkdir(parents=True)
        shutil.copy(REPO / ".github" / "workflows" / "ci.yaml", self.tmp / ".github" / "workflows" / "ci.yaml")
        shutil.copy(REPO / "Dockerfile", self.tmp / "Dockerfile")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def findings(self):
        report = nc.Report()
        nc.check_features(MANIFEST, self.tmp, report)
        return [f.text for f in report.findings]

    def test_ci_the_image_and_the_manifest_build_the_same_features(self):
        self.assertEqual(self.findings(), [])

    def test_an_image_built_with_other_features_is_reported(self):
        docker = self.tmp / "Dockerfile"
        docker.write_text(docker.read_text(encoding="utf-8").replace(",hedera", "", 1), encoding="utf-8")
        self.assertEqual(self.findings(), ["Dockerfile"])

    def test_a_ci_without_its_feature_list_is_reported(self):
        ci = self.tmp / ".github" / "workflows" / "ci.yaml"
        ci.write_text(ci.read_text(encoding="utf-8").replace("CARGO_FEATURES:", "OTHER:", 1), encoding="utf-8")
        self.assertEqual(self.findings(), ["features"])


class Write(unittest.TestCase):
    def test_a_crlf_file_keeps_its_line_ends_and_gets_exactly_the_digits(self):
        tmp = Path(tempfile.mkdtemp())
        try:
            (tmp / "static").mkdir()
            rel = "static/skill.md"
            body = "line one\r\nERC-8004 is live on 3 mainnets, 4 networks in total.\r\nend\r\n"
            (tmp / rel).write_bytes(body.encode("utf-8"))
            report = nc.Report()
            nc.check_text(MANIFEST, rel, (tmp / rel).read_bytes().decode("utf-8"), report)
            nc.apply_fixes(tmp, report)
            e = CAPS["erc8004"]
            self.assertEqual(
                (tmp / rel).read_bytes().decode("utf-8"),
                f"line one\r\nERC-8004 is live on {e['mainnets']} mainnets, {e['count']} networks in total.\r\nend\r\n",
            )
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_a_generated_file_is_never_rewritten_here(self):
        tmp = Path(tempfile.mkdtemp())
        try:
            (tmp / "static").mkdir()
            rel = "static/llms-full.txt"
            (tmp / rel).write_text("ERC-8004, live on 3 mainnets and 4 networks in total", encoding="utf-8")
            report = nc.Report()
            nc.check_text(MANIFEST, rel, (tmp / rel).read_text(encoding="utf-8"), report)
            self.assertTrue(report.findings)
            self.assertEqual(nc.apply_fixes(tmp, report), [])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
