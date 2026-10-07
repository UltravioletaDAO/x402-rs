"""Guards on the two `enable_zama` switches (decision 171, 2026-10-06).

The Zama stack (terraform/environments/zama-testnet) has to be all-or-nothing:
`enable_zama = false` must leave its state EMPTY and `true` must rebuild it
exactly. One resource added later without `count` would survive every "off",
and one reference to a counted resource without its index fails the plan the
first time the switch is off. The production half has to reach the container,
or the facilitator keeps advertising a scheme whose Lambda is gone.

Offline and without credentials: they read the .tf files as text. A plan
against the real states is c0der's to run (README.md there, "On/off").

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
ZAMA = REPO / "terraform" / "environments" / "zama-testnet"
PROD = REPO / "terraform" / "environments" / "production"

# Blocks whose switch is not the plain `count = local.zama_count`, and why.
OWN_SWITCH = {
    # Already counted by its own flag; the switch is ANDed in.
    ("resource", "aws_lambda_provisioned_concurrency_config", "zama"): re.compile(
        r"^\s*count\s*=\s*var\.enable_zama\s*&&\s*var\.enable_provisioned_concurrency\s*\?\s*1\s*:\s*0\s*$",
        re.M,
    ),
    # for_each over the (counted) certificate's validation options: empty
    # when there is no certificate, keyed by domain name as before.
    ("resource", "aws_route53_record", "cert_validation"): re.compile(
        r"^\s*for\s+dvo\s+in\s+flatten\(aws_acm_certificate\.main\[\*\]\.domain_validation_options\)",
        re.M,
    ),
}

PLAIN_SWITCH = re.compile(r"^  count = local\.zama_count$", re.M)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def blocks(text: str) -> dict[tuple[str, str, str], str]:
    """Top-level `resource`/`data` blocks, keyed by (kind, type, name)."""
    found = {}
    for match in re.finditer(r'^(resource|data) "([a-z0-9_]+)" "([a-z0-9_]+)" \{$', text, re.M):
        end = text.index("\n}\n", match.end())
        found[(match.group(1), match.group(2), match.group(3))] = text[match.end() : end]
    return found


ZAMA_MAIN = read(ZAMA / "main.tf")
ZAMA_BLOCKS = blocks(ZAMA_MAIN)


class ZamaStackSwitch(unittest.TestCase):
    def test_the_stack_has_blocks_to_check(self):
        self.assertGreaterEqual(len(ZAMA_BLOCKS), 25, sorted(ZAMA_BLOCKS))

    def test_every_resource_and_data_source_is_switched(self):
        for key, body in ZAMA_BLOCKS.items():
            pattern = OWN_SWITCH.get(key, PLAIN_SWITCH)
            self.assertRegex(body, pattern, f"{key} is not behind enable_zama")

    def test_the_switch_is_one_bool_and_one_local(self):
        variables = read(ZAMA / "variables.tf")
        block = re.search(r'variable "enable_zama" \{(.*?)\n\}', variables, re.S)
        self.assertIsNotNone(block, "variables.tf declares no enable_zama")
        self.assertRegex(block.group(1), r"\n  type\s+= bool\n")
        self.assertRegex(block.group(1), r"\n  default\s+= (true|false)$")
        self.assertRegex(ZAMA_MAIN, r"\n  zama_count = var\.enable_zama \? 1 : 0\n")

    def test_every_counted_resource_keeps_its_address_through_a_move(self):
        moved = re.findall(
            r"moved \{\n  from = ([a-z0-9_.]+)\n  to   = ([a-z0-9_.]+\[0\])\n\}",
            read(ZAMA / "moved.tf"),
        )
        self.assertEqual(
            sorted(f"{source}[0]" for source, _ in moved),
            sorted(target for _, target in moved),
            "every move goes from an address to its [0]",
        )
        counted = sorted(
            f"{kind_type}.{name}"
            for (kind, kind_type, name), body in ZAMA_BLOCKS.items()
            if kind == "resource" and PLAIN_SWITCH.search(body)
        )
        self.assertEqual(sorted(source for source, _ in moved), counted)

    def test_no_counted_block_is_referenced_without_its_index(self):
        # Counted blocks only: a for_each resource is referenced as its map.
        counted = [
            ("data." if kind == "data" else "") + f"{kind_type}.{name}"
            for (kind, kind_type, name), body in ZAMA_BLOCKS.items()
            if re.search(r"^  count\s*=", body, re.M)
        ]
        for path in sorted(ZAMA.glob("*.tf")):
            if path.name == "moved.tf":
                continue
            text = read(path)
            for address in counted:
                for hit in re.finditer(re.escape(address) + r"\b(?![\[\w])", text):
                    line = text[: hit.start()].count("\n") + 1
                    before = text[text.rfind("\n", 0, hit.start()) + 1 : hit.start()]
                    if before.lstrip().startswith("#"):
                        continue
                    # depends_on names a resource, which is fine counted.
                    if re.search(r"depends_on|^\s*" + re.escape(address) + r",?$", text.splitlines()[line - 1]):
                        continue
                    self.fail(f"{path.name}:{line} uses {address} without [0] or [*]")


class ProductionSwitch(unittest.TestCase):
    MAIN = read(PROD / "main.tf")
    VARIABLES = read(PROD / "variables.tf")
    TFVARS = read(PROD / "production.auto.tfvars")

    def default(self) -> str:
        block = re.search(r'variable "enable_zama" \{(.*?)\n\}', self.VARIABLES, re.S)
        self.assertIsNotNone(block, "production declares no enable_zama")
        self.assertRegex(block.group(1), r"\n  type\s+= bool\n")
        default = re.search(r"\n  default\s+= (true|false)$", block.group(1))
        self.assertIsNotNone(default, "enable_zama has no literal default")
        return default.group(1)

    def test_the_container_receives_the_switch(self):
        self.assertRegex(
            self.MAIN,
            r'name  = "ENABLE_ZAMA"\n\s*value = tostring\(var\.enable_zama\)',
        )

    def test_the_tfvars_ci_applies_agree_with_the_default(self):
        """CI applies production.auto.tfvars; a run without it must not
        silently land on the other value."""
        value = re.search(r"^enable_zama = (true|false)$", self.TFVARS, re.M)
        self.assertIsNotNone(value, "production.auto.tfvars does not set enable_zama")
        self.assertEqual(value.group(1), self.default())


if __name__ == "__main__":
    unittest.main()
