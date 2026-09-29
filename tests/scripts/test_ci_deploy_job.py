"""Guards on the shape of the deploy job in .github/workflows/ci.yaml.

Two holes this job had, each of which left production in a state nobody had
checked, and neither of which any test would have noticed coming back:

  1. A failed step skipped the rollout wait and the health check: one after
     the image apply (2.19.0, #34), and the image apply itself after it had
     moved the service (2026-09-26). Both times production was live with no
     verdict.
  2. The steps that apply resources outside the image deploy's -target list
     decided whether to run from this push's diff alone. A change whose own
     deploy failed was never applied by the pushes after it (2026-09-10).

Offline and without credentials: they read the workflow file through the same
PyYAML loader as scripts/ci_paths_selftest.py.

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import fnmatch
import re
import shlex
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import ci_last_green_deploy  # noqa: E402
import ci_paths_selftest  # noqa: E402

CI = ci_paths_selftest.load_workflows()["ci.yaml"]
DEPLOY = CI["jobs"]["deploy"]
STEPS = DEPLOY["steps"]

ECS_APPLY = "Terraform apply (roll ECS to the new image)"
LAMBDA = "Deploy balances Lambda"
OBSERVABILITY = "Deploy observability"
WAIT = "Wait for ECS rollout to stabilize"
HEALTH = "Verify deployment health"
DISCOVERY = "Deploy discovery bucket versioning"

# `${{ github.event.before }}`, however it is spaced. Inside `run:` an
# expression is interpolated even in a shell comment, so prose that merely
# names the context without the braces is not a use and is not matched.
EVENT_BEFORE = re.compile(r"\$\{\{[^}]*\bgithub\.event\.before\b[^}]*\}\}")

# The compare API lists at most this many files; a list that long may be cut short.
COMPARE_FILE_CAP = 300


def step(prefix: str) -> dict:
    found = [s for s in STEPS if str(s.get("name", "")).startswith(prefix)]
    if len(found) != 1:
        raise AssertionError(f"expected exactly one deploy step named {prefix!r}..., "
                             f"found {len(found)}")
    return found[0]


def conjuncts(expr: str) -> set[str]:
    """`${{ a && b }}` -> {'a', 'b'}, whitespace normalised."""
    body = expr.strip()
    if body.startswith("${{") and body.endswith("}}"):
        body = body[3:-2]
    return {" ".join(part.split()) for part in body.split("&&")}


class RolloutVerdictSurvivesAFailedStep(unittest.TestCase):
    """Hole 1: the wait and the health check hang off the ECS apply alone."""

    def test_the_ecs_apply_step_has_an_id(self):
        self.assertTrue(step(ECS_APPLY).get("id"),
                        f"'{ECS_APPLY}' needs an id: the wait and the health check "
                        "key on its outcome.")

    def test_wait_and_health_run_whenever_the_ecs_apply_ran(self):
        # Not `== 'success'`: an apply can fail after the service has moved
        # (2026-09-26), and that is exactly when the verdict is needed.
        apply_id = step(ECS_APPLY).get("id")
        for name in (WAIT, HEALTH):
            with self.subTest(step=name):
                cond = step(name).get("if")
                self.assertIsNotNone(
                    cond,
                    f"'{name}' has no if:, so any failed step before it skips it and "
                    "the new image stays live with no verdict.")
                cond = str(cond)
                self.assertNotIn("||", cond, f"'{name}': an || can reopen the skip.")
                parts = conjuncts(cond)
                self.assertIn("!cancelled()", parts,
                              f"'{name}' must run after a failed step (not after a cancel): {cond}")
                self.assertIn(f"steps.{apply_id}.outcome != 'skipped'", parts,
                              f"'{name}' must run whenever the ECS apply ran, failed or not: {cond}")

    def test_no_deploy_step_can_turn_a_failure_green(self):
        # The if: above only decides what runs. The job must still end red when a
        # step failed, and continue-on-error is the one thing that would stop that.
        for s in STEPS:
            with self.subTest(step=s.get("name", s.get("uses"))):
                self.assertNotIn("continue-on-error", s)
        self.assertNotIn("continue-on-error", DEPLOY)

    def test_the_order_of_the_deploy_steps_is_unchanged(self):
        # The fix is the if:, not moving steps: the observability apply is part of
        # what a release ships and stays before the wait, and the bucket stays last.
        names = [str(s.get("name", "")) for s in STEPS]
        idx = [next(i for i, n in enumerate(names) if n.startswith(p))
               for p in (ECS_APPLY, LAMBDA, OBSERVABILITY, WAIT, HEALTH, DISCOVERY)]
        self.assertEqual(idx, sorted(idx), f"deploy step order changed: {names}")
        self.assertEqual(idx[-1], len(STEPS) - 1,
                         f"'{DISCOVERY}' must stay the last step of the job.")


class NoApplySkipsOnOnePushDiff(unittest.TestCase):
    """Hole 2: nothing in the deploy job decides from this push's diff alone."""

    def test_no_deploy_step_uses_github_event_before(self):
        for s in STEPS:
            text = "\n".join(str(s.get(k, "")) for k in ("if", "run"))
            text += "\n" + "\n".join(str(v) for v in (s.get("env") or {}).values())
            text += "\n" + "\n".join(str(v) for v in (s.get("with") or {}).values())
            with self.subTest(step=s.get("name", s.get("uses"))):
                self.assertIsNone(
                    EVENT_BEFORE.search(text),
                    "github.event.before is the parent of this push, not of the last "
                    "deploy that went green: a skip decided on it loses the change of "
                    "every deploy that failed.")

    def test_observability_and_discovery_apply_on_every_deploy(self):
        for name in (OBSERVABILITY, DISCOVERY):
            with self.subTest(step=name):
                s = step(name)
                run = str(s.get("run", ""))
                self.assertNotIn("if", s, f"'{name}' must not be conditional.")
                self.assertIn("terraform apply", run)
                self.assertIsNone(re.search(r"\bexit 0\b", run),
                                  f"'{name}' has an early exit 0: it can skip its apply.")
                self.assertNotIn("/compare/", run,
                                 f"'{name}' diffs commits again; it must apply every time.")

    def test_the_lambda_diffs_against_the_last_green_deploy(self):
        run = str(step(LAMBDA).get("run", ""))
        self.assertIn("scripts/ci_last_green_deploy.py", run)
        self.assertRegex(run, r"/compare/\$\{base\}\.\.\.",
                         "the compare must start at the base the script picked")
        # Unsure means apply: a failed script and the script's own answer both land there.
        self.assertIn("|| base=aplicar", run)
        self.assertIn(f'"$base" != "{ci_last_green_deploy.APLICAR}"', run)

    def test_a_compare_at_the_api_file_cap_applies(self):
        # A compare that reached the cap cannot prove the Lambda did not change,
        # so only a list shorter than the cap may decide a skip.
        run = str(step(LAMBDA).get("run", ""))
        bound = re.search(r'wc -l\)"\s+-lt\s+(\d+)', run)
        self.assertIsNotNone(bound, "the Lambda step no longer bounds the compare's file count")
        self.assertEqual(int(bound.group(1)), COMPARE_FILE_CAP,
                         f"the compare lists at most {COMPARE_FILE_CAP} files; a longer bound "
                         "lets a cut-short list skip the Lambda.")

    def test_the_lambda_step_can_read_the_run_history(self):
        perms = DEPLOY.get("permissions") or {}
        self.assertEqual(perms.get("actions"), "read",
                         "without actions: read the runs API may refuse the token, and "
                         "the Lambda would be applied on every deploy.")
        self.assertIn("GH_TOKEN", step(LAMBDA).get("env") or {})


class TheTestJobRunsTheseGuards(unittest.TestCase):
    """A guard nobody runs guards nothing. The test job does not discover all of
    tests/scripts: it runs chosen patterns, and these files must match one."""

    def discover_patterns(self) -> list[str]:
        patterns = []
        for s in CI["jobs"]["test"]["steps"]:
            for line in str(s.get("run", "")).splitlines():
                if "unittest discover" not in line:
                    continue
                args = shlex.split(line)
                start = args.index("discover") + 1
                opts = dict(zip(args[start::2], args[start + 1::2]))
                if opts.get("-s") == "tests/scripts":
                    patterns.append(opts.get("-p", "test*.py"))  # unittest's default
        return patterns

    def test_every_deploy_guard_file_is_discovered_by_the_test_job(self):
        patterns = self.discover_patterns()
        guards = sorted(p.name for p in (REPO / "tests" / "scripts").glob("test_ci_*.py"))
        self.assertIn(Path(__file__).name, guards)
        for name in guards:
            with self.subTest(file=name):
                self.assertTrue(any(fnmatch.fnmatch(name, p) for p in patterns),
                                f"no `unittest discover -s tests/scripts -p ...` in the test "
                                f"job matches {name}; its patterns are {patterns}")


class TheScriptMatchesTheWorkflow(unittest.TestCase):
    """ci_last_green_deploy.py names the job, the file and the branch it reads."""

    def test_the_deploy_job_name_matches(self):
        self.assertEqual(ci_last_green_deploy.DEPLOY_JOB_NAME, DEPLOY["name"],
                         "renaming the deploy job without the script makes every "
                         "deploy look red to it, and the Lambda applies every time.")

    def test_the_workflow_file_and_branch_match(self):
        self.assertIn(ci_last_green_deploy.WORKFLOW_FILE, ci_paths_selftest.load_workflows())
        on = CI.get("on", CI.get(True))
        self.assertIn(ci_last_green_deploy.BRANCH, on["push"]["branches"])
        for event in ci_last_green_deploy.DEPLOY_EVENTS:
            self.assertIn(f"github.event_name == '{event}'", " ".join(DEPLOY["if"].split()))


if __name__ == "__main__":
    unittest.main()
