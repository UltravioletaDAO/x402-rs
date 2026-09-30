#!/usr/bin/env python3
"""Pick the base the balances-Lambda deploy step diffs against.

The step applies the Lambda only when its sources changed, and "changed" used
to mean "changed in this push" (`github.event.before...github.sha`). That loses
a change whenever the deploy that carried it failed before reaching the step:
the next push compares against its own parent, finds nothing, and skips. The
change sits in main and never reaches AWS -- measured 2026-09-10 on the
observability step, which had the same shape.

So the base is the commit of the last deploy that actually went green: the
head SHA of the most recent run of this workflow on main whose deploy job
concluded `success`, read from the Actions runs and jobs APIs.

Prints exactly one line on stdout: that SHA, or the word `aplicar` whenever
the answer cannot be trusted -- gh missing or failing, no green deploy among
the recent runs, an answer that does not look like a SHA, or a run that is not
a push (a manual "Run workflow" is the escape hatch that re-applies the Lambda
unconditionally). Applying when unsure is the rule the step already followed:
a redundant UpdateFunctionCode is cheap, a silently unapplied change is not.
Diagnostics go to stderr. The exit code is 0 either way.

Usage (inside the deploy job, where the defaults come from the runner):
    python3 scripts/ci_last_green_deploy.py
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from typing import Any, Callable

APLICAR = "aplicar"

WORKFLOW_FILE = "ci.yaml"
BRANCH = "main"
# The deploy job's `name:` in ci.yaml, which is what the jobs API reports.
# tests/scripts/test_ci_deploy_job.py fails if the two ever disagree.
DEPLOY_JOB_NAME = "Push to ECR & deploy to ECS (production)"
# The events the deploy job runs on (its `if:` in ci.yaml).
DEPLOY_EVENTS = ("push", "workflow_dispatch")
# How far back to look, pull requests included. Past this many runs without a
# green deploy the answer is `aplicar`, which is also the right answer after
# that long a red streak.
MAX_RUNS = 100
GH_TIMEOUT_SECS = 30

_SHA = re.compile(r"[0-9a-f]{40}")

Gh = Callable[[str], Any]


def gh_api(path: str) -> Any:
    """`gh api <path>`, parsed. Raises on any failure; the caller degrades it."""
    out = subprocess.run(
        ["gh", "api", path],
        capture_output=True,
        text=True,
        timeout=GH_TIMEOUT_SECS,
        check=True,
    )
    return json.loads(out.stdout)


def _say(msg: str) -> None:
    print(f"ci_last_green_deploy: {msg}", file=sys.stderr)


def compare_base(
    gh: Gh,
    *,
    repo: str,
    event: str,
    exclude_run: str = "",
    workflow: str = WORKFLOW_FILE,
    branch: str = BRANCH,
    job_name: str = DEPLOY_JOB_NAME,
    max_runs: int = MAX_RUNS,
) -> str:
    """The SHA of the last green deploy, or APLICAR when that is not certain."""
    if event != "push":
        _say(f"event is {event or 'unknown'!r}, not a push; applying.")
        return APLICAR
    if not repo:
        _say("no repository given; applying.")
        return APLICAR
    try:
        # Branch and event are filtered HERE, not with the API's `branch=` query:
        # measured 2026-09-29, `?branch=main` left out the two newest runs on main
        # (both green deploys) while the unfiltered listing had them. A base older
        # than the truth only widens the diff, but it is still the wrong answer.
        runs = gh(f"repos/{repo}/actions/workflows/{workflow}/runs"
                  f"?per_page={max_runs}")["workflow_runs"]
        candidates = [
            r for r in runs
            if str(r["id"]) != str(exclude_run)
            and r.get("head_branch") == branch
            and r.get("event") in DEPLOY_EVENTS
        ]
        # Most recently STARTED first, not most recently created: a re-run of an
        # older run is the deploy that ran last, and taking a newer commit as the
        # base would hide whatever lies between the two.
        candidates.sort(key=lambda r: r.get("run_started_at") or r.get("created_at") or "",
                        reverse=True)
        for run in candidates:
            jobs = gh(f"repos/{repo}/actions/runs/{run['id']}/jobs"
                      f"?filter=latest&per_page=100")["jobs"]
            if any(j.get("name") == job_name and j.get("conclusion") == "success"
                   for j in jobs):
                sha = run["head_sha"]
                if not isinstance(sha, str) or not _SHA.fullmatch(sha):
                    _say(f"run {run['id']} reports head_sha {sha!r}; applying.")
                    return APLICAR
                _say(f"last green deploy: run {run['id']} at {sha}.")
                return sha
    except Exception as exc:  # any doubt at all resolves to applying
        _say(f"could not read the run history ({type(exc).__name__}: {exc}); applying.")
        return APLICAR
    _say(f"no green deploy among the last {max_runs} runs on {branch}; applying.")
    return APLICAR


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME", ""))
    parser.add_argument("--exclude-run", default=os.environ.get("GITHUB_RUN_ID", ""),
                        help="the run asking; its own deploy is still in progress")
    args = parser.parse_args(argv)
    print(compare_base(gh_api, repo=args.repo, event=args.event,
                       exclude_run=args.exclude_run))
    return 0


if __name__ == "__main__":
    sys.exit(main())
