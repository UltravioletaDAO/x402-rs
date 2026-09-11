#!/usr/bin/env python3
"""Prove that the `paths` filters on .github/workflows/*.yml do what they claim.

A path filter fails silently in both directions. Too wide and the Actions budget
keeps burning on prose; too narrow and a commit that changes the facilitator
merges into main, goes green, and never reaches ECS -- with no run to notice,
because the run is exactly what did not happen. A merge to main IS a release
here, so the second failure mode has to be caught before the filter ships, not
after.

So this reimplements GitHub's own glob matching and asserts, file by file:

  1. Every build input the pipeline actually reads is matched (under-trigger).
  2. Prose is matched by nothing (over-trigger).
  3. push and pull_request carry byte-identical `paths` lists. GitHub Actions
     has no YAML anchors, so the list is written twice and the two copies can
     drift; this is the only thing stopping them.

Run it with no arguments:  python3 scripts/ci_paths_selftest.py
Exit code 0 = every case holds. Any failure prints the case and exits 1.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - the runner image ships PyYAML
    sys.exit("PyYAML is required: pip install pyyaml")

REPO = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO / ".github" / "workflows"


# ---------------------------------------------------------------------------
# GitHub's glob syntax, which is NOT fnmatch
# ---------------------------------------------------------------------------
#
# fnmatch's `*` crosses directory separators, so `src/*` would match
# `src/chain/evm.rs` there and not here. That single difference is enough to
# make a hand-check of this filter agree with a wrong answer, which is why the
# matching is spelled out instead of imported.
#
#   **  zero or more characters, separators included
#   *   zero or more characters, but never a `/`
#   ?   exactly one character, but never a `/`
#   !   at the head of a pattern, negates it; later patterns win over earlier
#
# Everything else is literal.
def _glob_to_regex(pattern: str) -> re.Pattern[str]:
    out: list[str] = []
    i = 0
    while i < len(pattern):
        c = pattern[i]
        if c == "*":
            if pattern[i + 1 : i + 2] == "*":
                out.append(".*")
                i += 2
                continue
            out.append("[^/]*")
        elif c == "?":
            out.append("[^/]")
        else:
            out.append(re.escape(c))
        i += 1
    return re.compile("^" + "".join(out) + "$")


def matches(path: str, patterns: list[str]) -> bool:
    """True when `path` satisfies a GitHub `paths` list (last match wins)."""
    verdict = False
    for pattern in patterns:
        negated = pattern.startswith("!")
        rx = _glob_to_regex(pattern[1:] if negated else pattern)
        if rx.match(path):
            verdict = not negated
    return verdict


def triggers(workflow: dict, event: str, changed: list[str]) -> bool:
    """Would `event` with this changed-file set start a run of `workflow`?"""
    # PyYAML parses the bare key `on:` as the boolean True (YAML 1.1).
    on = workflow.get("on", workflow.get(True))
    if not isinstance(on, dict) or event not in on:
        return False
    spec = on[event] or {}
    if not isinstance(spec, dict):
        return True  # e.g. `pull_request:` with no filters at all
    if "paths" in spec:
        return any(matches(f, spec["paths"]) for f in changed)
    if "paths-ignore" in spec:
        return any(not matches(f, spec["paths-ignore"]) for f in changed)
    return True


# ---------------------------------------------------------------------------
# The cases
# ---------------------------------------------------------------------------
#
# Each entry is (description, changed files, {workflow file: must it run?}).
# A workflow left out of the dict is not asserted on.
CI = "ci.yaml"
NOACC = "no-account-id.yml"

CASES: list[tuple[str, list[str], dict[str, bool]]] = [
    # -- must NOT build/test/deploy -----------------------------------------
    ("handoff doc only", ["docs/handoffs/2026-09-11-ci-paths.md"], {CI: False}),
    ("README only", ["README.md"], {CI: False}),
    ("CHANGELOG only", ["docs/CHANGELOG.md"], {CI: False}),
    ("CLAUDE.md only", ["CLAUDE.md"], {CI: False}),
    ("operator guide only", ["guides/ADDING_NEW_CHAINS.md"], {CI: False}),
    ("benchmark harness only",
     ["scripts/bench/run_bench.py", "scripts/bench/README.md",
      "docs/reports/2026-09-10-benchmark-capacidad.md"], {CI: False}),
    ("an operator script CI never runs", ["scripts/stablecoin_matrix.py"], {CI: False}),
    ("agent definitions", [".claude/agents/security-auditor.md"], {CI: False}),
    ("local dev compose", ["docker-compose.yml", "justfile"], {CI: False}),
    ("the other workflow", [".github/workflows/no-account-id.yml"], {CI: False}),
    ("the whole docs tree at once",
     ["docs/DX402.md", "docs/plans/dx402/08-SPEC-v0.2.md", "README.md",
      "guides/ADDING_NEW_CHAINS.md", "scripts/bench/cost_model.py"], {CI: False}),

    # -- must build/test/deploy --------------------------------------------
    ("a facilitator source file", ["src/handlers.rs"], {CI: True}),
    ("a chain module", ["src/chain/evm.rs"], {CI: True}),
    ("a workspace crate", ["crates/x402-axum/src/layer.rs"], {CI: True}),
    ("an example crate", ["examples/x402-axum-example/src/main.rs"], {CI: True}),
    ("the Dockerfile", ["Dockerfile"], {CI: True}),
    ("the dockerignore", [".dockerignore"], {CI: True}),
    ("a release bump", ["VERSION"], {CI: True}),
    ("the lockfile", ["Cargo.lock"], {CI: True}),
    ("the manifest", ["Cargo.toml"], {CI: True}),
    ("a crate manifest", ["crates/x402-compliance/Cargo.toml"], {CI: True}),
    ("the toolchain pin", ["rust-toolchain.toml"], {CI: True}),
    ("the cargo patch table", [".cargo/config.toml"], {CI: True}),
    # The landing is include_str!'d into the binary (src/handlers.rs:581), so a
    # portada-only change is a code change. There is no S3 sync to shortcut.
    ("the landing page", ["static/index.html"], {CI: True}),
    ("a network logo", ["static/base.png"], {CI: True}),
    ("a shared stylesheet", ["static/uv.css"], {CI: True}),
    ("a well-known document", ["static/.well-known/mcp/server-card.json"], {CI: True}),
    ("a font", ["static/fonts/uv-sans.woff2"], {CI: True}),
    ("a contract ABI", ["abi/USDC.json"], {CI: True}),
    ("shipped config", ["config/bazaar_curation.json"], {CI: True}),
    ("a sanctions list", ["config/ofac_addresses.json"], {CI: True}),
    ("a Rust integration test", ["tests/wire_conformance.rs"], {CI: True}),
    ("a test fixture", ["tests/fixtures/anchors.json"], {CI: True}),
    ("terraform", ["terraform/environments/production/main.tf"], {CI: True}),
    ("an alarm file", ["terraform/environments/production/alerts-solana-mint.tf"], {CI: True}),
    ("the balances Lambda", ["lambda/balances/handler.py"], {CI: True}),
    ("the landing checker the test job runs", ["scripts/verify_landing_canonical.py"], {CI: True}),
    ("this pipeline", [".github/workflows/ci.yaml"], {CI: True}),

    # -- mixed: one build input anywhere in the push is enough --------------
    ("docs plus one source file", ["docs/CHANGELOG.md", "src/network.rs"], {CI: True}),
    ("a release: VERSION plus prose", ["VERSION", "docs/CHANGELOG.md", "README.md"], {CI: True}),
]

# The leak gate reads prose on purpose: its bare-12-digit rule scans *.md and
# .claude/ and nothing else. Any `paths` filter there would disarm the one check
# that looks at documentation, on a public repo with no branch protection. It is
# asserted unfiltered rather than left unasserted, so that narrowing it later has
# to be a deliberate edit to this file.
NOACC_CASES: list[tuple[str, list[str]]] = [
    ("a doc", ["docs/handoffs/2026-09-11-ci-paths.md"]),
    ("an agent definition", [".claude/agents/security-auditor.md"]),
    ("a source file", ["src/handlers.rs"]),
    ("terraform", ["terraform/environments/production/main.tf"]),
]


def main() -> int:
    workflows = {p.name: yaml.safe_load(p.read_text()) for p in sorted(WORKFLOWS.iterdir())
                 if p.suffix in (".yml", ".yaml")}
    missing = {CI, NOACC} - workflows.keys()
    if missing:
        print(f"FAIL  workflow file(s) not found: {sorted(missing)}")
        return 1

    failures = 0
    print(f"Workflows under test: {', '.join(sorted(workflows))}\n")

    # 1. push and pull_request must carry the same list.
    for name, wf in workflows.items():
        on = wf.get("on", wf.get(True)) or {}
        push = (on.get("push") or {})
        pr = (on.get("pull_request") or {})
        if isinstance(push, dict) and isinstance(pr, dict) and "paths" in push and "paths" in pr:
            if push["paths"] != pr["paths"]:
                only_push = [p for p in push["paths"] if p not in pr["paths"]]
                only_pr = [p for p in pr["paths"] if p not in push["paths"]]
                print(f"FAIL  {name}: push and pull_request `paths` have drifted apart.")
                print(f"        only on push: {only_push}")
                print(f"        only on pull_request: {only_pr}")
                failures += 1
            else:
                print(f"ok    {name}: push and pull_request `paths` are identical "
                      f"({len(push['paths'])} patterns)")

    print()
    # 2. The trigger table.
    for desc, changed, expect in CASES:
        for wf_name, want in expect.items():
            for event in ("push", "pull_request"):
                got = triggers(workflows[wf_name], event, changed)
                if got != want:
                    print(f"FAIL  {wf_name} on {event}: {desc} -> ran={got}, expected {want}")
                    print(f"        changed: {changed}")
                    failures += 1
        verdicts = " ".join(f"{w}={'run' if r else 'skip'}" for w, r in expect.items())
        print(f"ok    {desc:46s} {verdicts}")

    print()
    # 3. Every pattern must match something that is actually in the tree. A
    #    typo ('lambdas/**', 'statics/**') is invisible in every test above --
    #    it simply never matches, and the job it was protecting stops running.
    tracked = subprocess.run(["git", "ls-files"], cwd=REPO, capture_output=True,
                             text=True, check=True).stdout.split("\n")
    tracked = [f for f in tracked if f]
    ci_on = workflows[CI].get("on", workflows[CI].get(True)) or {}
    for pattern in (ci_on.get("push") or {}).get("paths", []):
        if pattern.startswith("!"):
            continue
        hits = sum(1 for f in tracked if matches(f, [pattern]))
        if hits == 0:
            print(f"FAIL  {CI}: pattern '{pattern}' matches no tracked file. Typo, or the "
                  f"input it guarded was deleted; either way it guards nothing now.")
            failures += 1
        else:
            print(f"ok    '{pattern}' matches {hits} tracked file(s)")

    print()
    # 4. The leak gate stays unfiltered.
    for desc, changed in NOACC_CASES:
        for event in ("push", "pull_request"):
            if not triggers(workflows[NOACC], event, changed):
                print(f"FAIL  {NOACC} on {event} did not run for {desc}: {changed}")
                print("        This gate reads *.md and .claude/ for a leaked AWS account ID.")
                failures += 1
        print(f"ok    {NOACC} scans {desc}")

    print()
    if failures:
        print(f"{failures} failing case(s).")
        return 1
    print(f"All {sum(len(e) for _, _, e in CASES) * 2 + len(NOACC_CASES) * 2} assertions hold.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
