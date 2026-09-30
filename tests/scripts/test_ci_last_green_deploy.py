"""scripts/ci_last_green_deploy.py against a stand-in for gh.

The script decides the base the balances-Lambda step diffs against. Its whole
contract is two answers: the head SHA of the last green deploy, or `aplicar`
whenever that is not certain. These tests hold both, with no network: a fake
`gh` callable for the logic, and a fake `gh` executable on PATH for the CLI.

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "ci_last_green_deploy.py"
sys.path.insert(0, str(REPO / "scripts"))

import ci_last_green_deploy as lgd  # noqa: E402

OWNER_REPO = "example-org/example-repo"
GREEN = "a" * 40
OLDER_GREEN = "c" * 40
RED = "b" * 40
# What `github.event.before` would say on this push. The answer must never be it.
BEFORE = "d" * 40

RUNS = f"repos/{OWNER_REPO}/actions/workflows/ci.yaml/runs"


def jobs_path(run_id: int) -> str:
    return f"repos/{OWNER_REPO}/actions/runs/{run_id}/jobs"


def run(run_id, sha, started, event="push", branch="main", conclusion="success", created=None):
    # A re-run keeps its created_at and gets a new run_started_at, so the two can differ.
    return {"id": run_id, "head_sha": sha, "event": event, "head_branch": branch,
            "run_started_at": started, "created_at": created or started,
            "conclusion": conclusion}


def jobs(deploy_conclusion, extra=()):
    listed = [{"name": "Build & test", "conclusion": "success"},
              {"name": "Check deploy prerequisites", "conclusion": "success"},
              {"name": lgd.DEPLOY_JOB_NAME, "conclusion": deploy_conclusion}]
    return {"jobs": listed + list(extra)}


class FakeGh:
    """Answers `gh api <path>` from a table keyed by path prefix."""

    def __init__(self, answers=None, fail=False):
        self.answers = answers or {}
        self.fail = fail
        self.calls: list[str] = []

    def __call__(self, path):
        self.calls.append(path)
        if self.fail:
            raise subprocess.CalledProcessError(1, ["gh", "api", path],
                                                stderr="HTTP 502: Bad Gateway")
        for prefix, answer in self.answers.items():
            if path.startswith(prefix):
                return answer
        raise subprocess.CalledProcessError(1, ["gh", "api", path], stderr="HTTP 404")


class CompareBase(unittest.TestCase):
    def setUp(self):
        # A push event whose `before` is BEFORE, the way the runner presents it.
        fd, self.event_file = tempfile.mkstemp(suffix=".json")
        with os.fdopen(fd, "w") as fh:
            json.dump({"before": BEFORE, "after": "e" * 40}, fh)
        patcher = mock.patch.dict(os.environ, {"GITHUB_EVENT_PATH": self.event_file,
                                              "GITHUB_EVENT_NAME": "push"})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(os.remove, self.event_file)
        quiet = mock.patch.object(lgd, "_say")  # the CLI tests below cover stderr
        quiet.start()
        self.addCleanup(quiet.stop)

    def base(self, gh, event="push", exclude_run="300"):
        return lgd.compare_base(gh, repo=OWNER_REPO, event=event, exclude_run=exclude_run)

    def test_a_green_deploy_found_returns_its_sha(self):
        gh = FakeGh({
            RUNS: {"workflow_runs": [
                run(300, "f" * 40, "2026-09-29T10:00:00Z"),        # this run, in progress
                run(299, RED, "2026-09-29T09:00:00Z", conclusion="failure"),
                run(298, GREEN, "2026-09-29T08:00:00Z"),
                run(297, OLDER_GREEN, "2026-09-29T07:00:00Z"),
            ]},
            jobs_path(299): jobs("failure"),
            jobs_path(298): jobs("success"),
            jobs_path(297): jobs("success"),
        })
        got = self.base(gh)
        self.assertEqual(got, GREEN)
        self.assertNotEqual(got, BEFORE)
        self.assertNotIn(jobs_path(300), gh.calls, "the asking run must be excluded")

    def test_a_deploy_that_did_not_succeed_is_not_green(self):
        for conclusion in ("failure", "cancelled", "skipped", None):
            with self.subTest(deploy=conclusion):
                gh = FakeGh({
                    RUNS: {"workflow_runs": [run(299, RED, "2026-09-29T09:00:00Z"),
                                             run(298, GREEN, "2026-09-29T08:00:00Z")]},
                    jobs_path(299): jobs(conclusion),
                    jobs_path(298): jobs("success"),
                })
                self.assertEqual(self.base(gh), GREEN)

    def test_a_green_deploy_counts_even_when_another_job_went_red(self):
        # The drift gate failing turns the RUN red; the deploy still shipped.
        gh = FakeGh({
            RUNS: {"workflow_runs": [run(299, GREEN, "2026-09-29T09:00:00Z",
                                         conclusion="failure")]},
            jobs_path(299): jobs("success", [{"name": "Terraform plan (drift gate)",
                                              "conclusion": "failure"}]),
        })
        self.assertEqual(self.base(gh), GREEN)

    def test_a_rerun_that_went_green_last_wins_over_a_newer_commit(self):
        # Run 297 was created first but re-run (and deployed) after run 298: ordering
        # by created_at would pick 298, a newer commit than the last one deployed, and
        # hide whatever lies between the two.
        gh = FakeGh({
            RUNS: {"workflow_runs": [run(298, GREEN, "2026-09-29T08:00:00Z"),
                                     run(297, OLDER_GREEN, "2026-09-29T09:30:00Z",
                                         created="2026-09-29T07:00:00Z")]},
            jobs_path(298): jobs("success"),
            jobs_path(297): jobs("success"),
        })
        self.assertEqual(self.base(gh), OLDER_GREEN)

    def test_only_deploy_events_on_the_branch_count(self):
        gh = FakeGh({
            RUNS: {"workflow_runs": [
                run(299, RED, "2026-09-29T09:00:00Z", event="pull_request"),
                run(298, RED, "2026-09-29T08:30:00Z", branch="feature"),
                run(297, GREEN, "2026-09-29T08:00:00Z", event="workflow_dispatch"),
            ]},
            jobs_path(299): jobs("success"),
            jobs_path(298): jobs("success"),
            jobs_path(297): jobs("success"),
        })
        self.assertEqual(self.base(gh), GREEN)

    def test_the_runs_listing_is_not_filtered_by_branch_server_side(self):
        # `?branch=main` dropped the two newest main runs on 2026-09-29; the branch
        # has to be filtered from the unfiltered listing, as the test above does.
        gh = FakeGh({RUNS: {"workflow_runs": [run(299, GREEN, "2026-09-29T09:00:00Z")]},
                     jobs_path(299): jobs("success")})
        self.assertEqual(self.base(gh), GREEN)
        listing = [c for c in gh.calls if c.startswith(RUNS)]
        self.assertEqual(len(listing), 1)
        self.assertNotIn("branch=", listing[0])

    def test_gh_failing_means_apply(self):
        self.assertEqual(self.base(FakeGh(fail=True)), lgd.APLICAR)

    def test_a_failed_jobs_call_means_apply(self):
        gh = FakeGh({RUNS: {"workflow_runs": [run(299, GREEN, "2026-09-29T09:00:00Z")]}})
        self.assertEqual(self.base(gh), lgd.APLICAR)

    def test_no_green_deploy_means_apply(self):
        for runs in ([], [run(299, RED, "2026-09-29T09:00:00Z")]):
            with self.subTest(runs=len(runs)):
                gh = FakeGh({RUNS: {"workflow_runs": runs}, jobs_path(299): jobs("failure")})
                self.assertEqual(self.base(gh), lgd.APLICAR)

    def test_only_the_asking_run_means_apply(self):
        gh = FakeGh({RUNS: {"workflow_runs": [run(300, GREEN, "2026-09-29T10:00:00Z")]},
                     jobs_path(300): jobs("success")})
        self.assertEqual(self.base(gh, exclude_run="300"), lgd.APLICAR)

    def test_an_answer_that_is_not_a_sha_means_apply(self):
        # The SHA lands in a shell command in ci.yaml; anything else is refused.
        for sha in ("", "HEAD", "a" * 39, GREEN.upper(), f"{GREEN}; true", None):
            with self.subTest(sha=sha):
                gh = FakeGh({RUNS: {"workflow_runs": [run(299, sha, "2026-09-29T09:00:00Z")]},
                             jobs_path(299): jobs("success")})
                self.assertEqual(self.base(gh), lgd.APLICAR)

    def test_a_malformed_answer_means_apply(self):
        for answer in ({}, {"workflow_runs": None}, {"workflow_runs": [{"id": 1}]}, []):
            with self.subTest(answer=answer):
                self.assertEqual(self.base(FakeGh({RUNS: answer})), lgd.APLICAR)

    def test_a_manual_run_always_applies(self):
        # workflow_dispatch is the documented resync path for a drifted Lambda.
        gh = FakeGh({RUNS: {"workflow_runs": [run(299, GREEN, "2026-09-29T09:00:00Z")]},
                     jobs_path(299): jobs("success")})
        for event in ("workflow_dispatch", "", "pull_request"):
            with self.subTest(event=event):
                self.assertEqual(self.base(gh, event=event), lgd.APLICAR)

    def test_no_repository_means_apply(self):
        self.assertEqual(lgd.compare_base(FakeGh(), repo="", event="push"), lgd.APLICAR)


@unittest.skipIf(os.name == "nt", "the fake gh is a POSIX executable")
class CommandLine(unittest.TestCase):
    """The real subprocess path, with a fake `gh` first on PATH."""

    FAKE_GH = (
        "#!/usr/bin/env python3\n"
        "import json, os, sys\n"
        "table = json.load(open(os.environ['FAKE_GH_TABLE']))\n"
        "path = sys.argv[2] if len(sys.argv) > 2 else ''\n"
        "for prefix, answer in table.items():\n"
        "    if path.startswith(prefix):\n"
        "        print(json.dumps(answer))\n"
        "        sys.exit(0)\n"
        "print('HTTP 404: Not Found', file=sys.stderr)\n"
        "sys.exit(1)\n"
    )

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.bin = Path(self.tmp.name)
        gh = self.bin / "gh"
        gh.write_text(self.FAKE_GH, encoding="utf-8")
        gh.chmod(gh.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        self.table = self.bin / "table.json"

    def cli(self, table, event="push"):
        self.table.write_text(json.dumps(table), encoding="utf-8")
        env = dict(os.environ,
                   PATH=f"{self.bin}{os.pathsep}{os.environ.get('PATH', '')}",
                   FAKE_GH_TABLE=str(self.table),
                   GITHUB_REPOSITORY=OWNER_REPO, GITHUB_EVENT_NAME=event,
                   GITHUB_RUN_ID="300")
        return subprocess.run([sys.executable, str(SCRIPT)], env=env,
                              capture_output=True, text=True, timeout=60)

    def test_prints_the_sha_of_the_last_green_deploy(self):
        out = self.cli({RUNS: {"workflow_runs": [run(299, GREEN, "2026-09-29T09:00:00Z")]},
                        jobs_path(299): jobs("success")})
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout, GREEN + "\n", "stdout must carry the answer and nothing else")

    def test_prints_aplicar_when_gh_fails(self):
        out = self.cli({})
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout, lgd.APLICAR + "\n")
        self.assertIn("applying", out.stderr)

    def test_prints_aplicar_when_gh_is_not_installed(self):
        env = dict(os.environ, PATH=str(self.bin / "nowhere"), GITHUB_REPOSITORY=OWNER_REPO,
                   GITHUB_EVENT_NAME="push", GITHUB_RUN_ID="300")
        out = subprocess.run([sys.executable, str(SCRIPT)], env=env,
                             capture_output=True, text=True, timeout=60)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout, lgd.APLICAR + "\n")


if __name__ == "__main__":
    unittest.main()
