"""Every action a workflow runs is pinned to a full commit SHA (X402-SEG-CI, 2026-11-18).

A tag or a branch is a pointer the action's owner -- or whoever takes over their account --
can move: the next run executes whatever it points at, with this repository's token and,
in the plan and deploy jobs, its AWS session. A 40-hex commit cannot be moved. Until this
change all 16 `uses:` of the three workflows named a tag or a branch.

Each pin carries its version in a trailing comment (`@<sha> # v4.4.0`), the convention
Dependabot and Renovate read, so a human can tell what is pinned without the API. The same
action is pinned to one commit across every workflow: a half-done bump is a mistake.

Offline: the workflow files are read as text (PyYAML drops comments) and through the same
loader as scripts/ci_paths_selftest.py.

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import ci_paths_selftest  # noqa: E402

WORKFLOWS = REPO / ".github" / "workflows"

# A `uses:` line, as a step (`- uses:`) or a job (`uses:` of a reusable workflow), with the
# trailing comment if there is one. Quotes around the value are allowed by YAML.
USES_LINE = re.compile(
    r"^[ \t]*(?:-[ \t]+)?uses:[ \t]*['\"]?(?P<ref>[^'\"\s#]+)['\"]?[ \t]*(?:#[ \t]*(?P<comment>.*?))?[ \t]*$",
    re.M,
)
SHA = re.compile(r"[0-9a-f]{40}")
# What the comment must name: a version, v-prefixed or not (dtolnay/rust-toolchain's branches are
# bare toolchain versions).
VERSION = re.compile(r"v?[0-9]+(?:\.[0-9]+){0,2}")


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def uses_lines() -> list[tuple[str, int, str, str | None]]:
    """(workflow file, line, owner/repo@ref, comment) for every `uses:` line of every workflow."""
    found = []
    for path in sorted(WORKFLOWS.glob("*.y*ml")):
        text = read(path)
        for m in USES_LINE.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            comment = m.group("comment")
            found.append((path.name, line, m.group("ref"), comment.strip() if comment else None))
    return found


def yaml_uses() -> list[tuple[str, str]]:
    """(workflow file, uses) for every step and job `uses:` as YAML reads them."""
    found = []
    for wf_name, wf in ci_paths_selftest.load_workflows().items():
        for job in (wf.get("jobs") or {}).values():
            if "uses" in job:
                found.append((wf_name, str(job["uses"])))
            for step in job.get("steps") or []:
                if "uses" in step:
                    found.append((wf_name, str(step["uses"])))
    return found


class EveryActionIsPinnedToACommit(unittest.TestCase):
    def test_the_text_scan_sees_every_uses_yaml_sees(self):
        # The pins below are checked on the text; this keeps the text scan honest about coverage.
        text = sorted((wf, ref) for wf, _, ref, _ in uses_lines())
        yaml = sorted(yaml_uses())
        self.assertEqual(text, yaml, "a `uses:` the text scan does not parse would go unchecked")
        self.assertGreaterEqual(len(yaml), 16, "fewer than the 16 uses: measured on 2026-11-18: this guard checks less")

    def test_every_uses_names_a_full_commit_sha(self):
        for wf, line, ref, _ in uses_lines():
            with self.subTest(where=f"{wf}:{line}", uses=ref):
                self.assertFalse(ref.startswith("./") or ref.startswith("docker://"),
                                 "a local or docker action: teach this guard how it is pinned")
                action, sep, version = ref.partition("@")
                self.assertTrue(sep, f"{wf}:{line} uses {ref} with no ref at all")
                self.assertRegex(action, r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+$")
                self.assertIsNotNone(SHA.fullmatch(version),
                                     f"{wf}:{line} uses {ref}: a tag or branch can be moved; pin the 40-hex commit "
                                     "(gh api repos/<owner>/<repo>/commits/<tag> --jq .sha)")

    def test_every_pin_says_which_version_it_is(self):
        for wf, line, ref, comment in uses_lines():
            with self.subTest(where=f"{wf}:{line}", uses=ref):
                self.assertIsNotNone(comment, f"{wf}:{line} pins {ref} without `# <version>`")
                self.assertIsNotNone(VERSION.fullmatch(comment or ""),
                                     f"{wf}:{line}: the comment {comment!r} is not a version")

    def test_one_commit_and_one_version_per_action(self):
        pins: dict[str, set[tuple[str, str | None]]] = {}
        for _, _, ref, comment in uses_lines():
            action, _, version = ref.partition("@")
            pins.setdefault(action.lower(), set()).add((version, comment))
        for action, seen in pins.items():
            with self.subTest(action=action):
                self.assertEqual(len(seen), 1, f"{action} is pinned to more than one commit or version: {sorted(seen)}")


if __name__ == "__main__":
    unittest.main()
