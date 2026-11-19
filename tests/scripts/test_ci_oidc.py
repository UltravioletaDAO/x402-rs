"""The CI reaches AWS by GitHub OIDC, each job with its own role (X402-SEG-CI, 2026-11-18).

Before this, plan and deploy authenticated with the access key of the IAM user
github-actions-facilitator-deploy, stored as repository secrets of a public repository; the
drift gate ran with that write-capable key on every pull request. Now:

  - plan   assumes facilitator-github-plan, read-only, trusted for pull_request and main;
  - deploy assumes facilitator-github-deploy, trusted for the `production` environment alone.

What the roles trust and grant is held by terraform/environments/production/tests/github_oidc.tftest.hcl.
This file holds the workflow's half, and that the two halves name the same roles and environment;
and that every workflow's GitHub token defaults to reading the repository, the jobs that need more
naming it in their own block.

Offline: the workflows through the same PyYAML loader as scripts/ci_paths_selftest.py, the
Terraform as text.

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import ci_paths_selftest  # noqa: E402

WORKFLOWS = ci_paths_selftest.load_workflows()
CI = WORKFLOWS["ci.yaml"]
TF = REPO / "terraform" / "environments" / "production"
OIDC_TF = TF / "cicd-oidc.tf"

CREDENTIALS_ACTION = "aws-actions/configure-aws-credentials"
ROLE_FOR_JOB = {"plan": "facilitator-github-plan", "deploy": "facilitator-github-deploy"}
DEPLOY_ENVIRONMENT = "production"
# `${{ secrets.AWS_ACCOUNT_ID }}`, however it is spaced.
ACCOUNT_SECRET = r"\$\{\{\s*secrets\.AWS_ACCOUNT_ID\s*\}\}"
# The static key, by secret or by input, wherever it appears.
STATIC_KEY = re.compile(r"AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|aws-access-key-id|aws-secret-access-key", re.I)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def credential_steps():
    """(workflow, job, step) for every configure-aws-credentials step."""
    for wf_name, wf in WORKFLOWS.items():
        for job_name, job in (wf.get("jobs") or {}).items():
            for step in job.get("steps") or []:
                if str(step.get("uses", "")).split("@", 1)[0].lower() == CREDENTIALS_ACTION:
                    yield wf_name, job_name, step


def environment_name(job: dict) -> str | None:
    env = job.get("environment")
    if isinstance(env, dict):
        env = env.get("name")
    return None if env is None else str(env)


class NoStaticKeyLeft(unittest.TestCase):
    def test_no_workflow_names_the_access_key(self):
        for wf_name, wf in WORKFLOWS.items():
            with self.subTest(workflow=wf_name):
                self.assertIsNone(STATIC_KEY.search(json.dumps(wf, default=str)),
                                  f"{wf_name} still reads a static AWS key; the CI assumes roles by OIDC")

    def test_preflight_asks_for_the_account_only(self):
        env = {k: " ".join(str(v).split()) for k, v in (CI["jobs"]["preflight"]["steps"][0].get("env") or {}).items()}
        self.assertEqual(sorted(env), ["FORK_PR", "HAS_ACCOUNT"], f"preflight's check reads {sorted(env)}")
        self.assertRegex(env["HAS_ACCOUNT"], r"^\$\{\{\s*secrets\.AWS_ACCOUNT_ID\s*!=\s*''\s*\}\}$")
        self.assertRegex(env["FORK_PR"],
                         r"^\$\{\{\s*github\.event_name\s*==\s*'pull_request'\s*&&\s*"
                         r"github\.event\.pull_request\.head\.repo\.full_name\s*!=\s*github\.repository\s*\}\}$")

    def test_no_workflow_runs_on_an_event_that_hands_a_fork_the_base_repos_identity(self):
        # pull_request_target and workflow_run run the BASE repository's workflow with its secrets
        # and OIDC token, on behalf of a fork: the plan role trusts pull_request and main's ref.
        for wf_name, wf in WORKFLOWS.items():
            on = wf.get("on", wf.get(True)) or {}
            events = set(on) if isinstance(on, (dict, list)) else {str(on)}
            with self.subTest(workflow=wf_name):
                self.assertFalse(events & {"pull_request_target", "workflow_run"}, f"{wf_name} runs on {sorted(events)}")


class APreflightWithoutTheAccount(unittest.TestCase):
    """Before the switch-on, plan and deploy are skipped. Green would then read as checked or shipped,
    so every run fails -- push, release, a pull request from this repository -- except a pull request
    from a fork, which never gets the repository's secrets."""

    def run_check(self, has_account: str, fork_pr: str):
        if os.name != "posix" or shutil.which("bash") is None:
            self.skipTest("needs a POSIX bash, as the runner has")
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out"
            out.write_text("", encoding="utf-8")
            env = {"PATH": os.environ.get("PATH", ""), "HAS_ACCOUNT": has_account, "FORK_PR": fork_pr,
                   "GITHUB_OUTPUT": str(out)}
            done = subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c",
                                   str(CI["jobs"]["preflight"]["steps"][0]["run"])],
                                  env=env, capture_output=True, text=True, check=False)
            return done, out.read_text(encoding="utf-8")

    def test_with_the_account_every_run_plans_and_may_deploy(self):
        for fork in ("false", "true"):
            with self.subTest(fork_pr=fork):
                done, out = self.run_check("true", fork)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertIn("deploy=true", out)

    def test_without_it_a_fork_pull_request_skips_quietly(self):
        done, out = self.run_check("false", "true")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("deploy=false", out)
        self.assertNotIn("::error", done.stdout)

    def test_without_it_every_other_run_fails(self):
        done, out = self.run_check("false", "false")
        self.assertNotEqual(done.returncode, 0, "a run with no AWS account went green with nothing checked or shipped")
        self.assertIn("::error", done.stdout)
        self.assertNotIn("deploy=true", out)


class TheEnvironmentGuard(unittest.TestCase):
    """The plan job's step that reads, read-only, whether GitHub keeps branches out of the production
    environment the deploy role trusts. Run as the runner's bash runs it, with a stand-in `gh` that
    answers each endpoint from a fixture."""

    STEP = "The production environment admits main alone"
    REPO = "owner/repo"

    def run_guard(self, env_json=None, env_error=None, policies=None, protected=True):
        if os.name != "posix" or shutil.which("bash") is None or shutil.which("jq") is None:
            self.skipTest("needs a POSIX bash and jq, as the runner has")
        step = [s for s in CI["jobs"]["plan"]["steps"] if s.get("name") == self.STEP]
        self.assertEqual(len(step), 1, f"the plan job has no step named {self.STEP!r}")
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "env.json").write_text(json.dumps(env_json or {}), encoding="utf-8")
            (d / "pol.json").write_text(json.dumps({"branch_policies": policies or []}), encoding="utf-8")
            (d / "main.json").write_text(json.dumps({"protected": protected}), encoding="utf-8")
            env_branch = (f'echo "{env_error}" >&2; exit 1' if env_error else f'cat "{d}/env.json"')
            (d / "gh").write_text(
                "#!/bin/sh\n"
                f'case "$*" in\n'
                f'  "api repos/{self.REPO}/environments/production") {env_branch} ;;\n'
                f'  "api repos/{self.REPO}/environments/production/deployment-branch-policies") cat "{d}/pol.json" ;;\n'
                f'  "api repos/{self.REPO}/branches/main") cat "{d}/main.json" ;;\n'
                '  *) echo "unexpected gh call: $*" >&2; exit 2 ;;\n'
                "esac\n", encoding="utf-8")
            (d / "gh").chmod(0o755)
            env = {"PATH": f"{d}{os.pathsep}{os.environ.get('PATH', '')}", "GH_TOKEN": "not-a-token",
                   "REPO": self.REPO}
            return subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", str(step[0]["run"])],
                                  env=env, capture_output=True, text=True, check=False)

    GOOD_ENV = {"deployment_branch_policy": {"protected_branches": False, "custom_branch_policies": True},
                "protection_rules": [{"type": "required_reviewers"}, {"type": "branch_policy"}]}
    MAIN = [{"name": "main", "type": "branch"}]

    def test_main_alone_with_a_reviewer_passes_clean(self):
        done = self.run_guard(self.GOOD_ENV, policies=self.MAIN)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertNotIn("::error", done.stdout)
        self.assertNotIn("::warning", done.stdout)

    def test_a_missing_environment_fails(self):
        done = self.run_guard(env_error="gh: Not Found (HTTP 404)")
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("::error title=No production environment", done.stdout)

    def test_an_environment_open_to_every_branch_fails(self):
        for policy in ({"protected_branches": False, "custom_branch_policies": False}, None):
            with self.subTest(policy=policy):
                done = self.run_guard({"deployment_branch_policy": policy, "protection_rules": []}, policies=self.MAIN)
                self.assertNotEqual(done.returncode, 0)
                self.assertIn("::error title=production admits any branch", done.stdout)

    def test_a_rule_beyond_main_fails(self):
        for policies in ([{"name": "main", "type": "branch"}, {"name": "dev", "type": "branch"}],
                         [{"name": "*", "type": "branch"}], [{"name": "main", "type": "tag"}], []):
            with self.subTest(policies=policies):
                done = self.run_guard(self.GOOD_ENV, policies=policies)
                self.assertNotEqual(done.returncode, 0)
                self.assertIn("::error title=production admits more than main", done.stdout)

    def test_no_reviewer_or_an_open_main_warns(self):
        no_reviewer = dict(self.GOOD_ENV, protection_rules=[{"type": "branch_policy"}])
        done = self.run_guard(no_reviewer, policies=self.MAIN, protected=False)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("::warning title=production has no required reviewer", done.stdout)
        self.assertIn("::warning title=main is not protected", done.stdout)

    def test_a_read_that_does_not_complete_warns_and_passes(self):
        done = self.run_guard(env_error="gh: Resource not accessible by integration (HTTP 403)")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("::warning title=Environment check inconclusive", done.stdout)


# The scopes a job may raise above `read`, and the jobs that may: plan and deploy exchange the job's
# OIDC token for their role; plan reads the production environment's rules (step "The production
# environment admits main alone") and deploy this workflow's run history
# (scripts/ci_last_green_deploy.py).
RAISED = {
    ("ci.yaml", "plan"): {"id-token": "write", "actions": "read"},
    ("ci.yaml", "deploy"): {"id-token": "write", "actions": "read"},
}


def scopes(block) -> dict:
    """A `permissions:` block as {scope: level}. The shorthand strings are refused: they grant every
    scope at once."""
    if not isinstance(block, dict):
        raise AssertionError(f"permissions: {block!r} -- list the scopes; read-all/write-all grant every one")
    return {str(k): str(v) for k, v in block.items()}


class EveryTokenStartsReadOnly(unittest.TestCase):
    """Every workflow sets `permissions: contents: read` at the top, so a job that does not ask for more
    (test, preflight, the account-id scan, the plugin's build) gets a token that can only read the
    repository, whatever the repository's default is. A job's own block replaces that one."""

    def test_every_workflow_defaults_to_reading_the_repository(self):
        for wf_name, wf in WORKFLOWS.items():
            with self.subTest(workflow=wf_name):
                self.assertIn("permissions", wf,
                              f"{wf_name} has no top-level permissions: its jobs get the repository's "
                              "default token, which may write")
                self.assertEqual(scopes(wf["permissions"]), {"contents": "read"}, wf_name)

    def test_no_job_raises_its_token_beyond_what_it_needs(self):
        for wf_name, wf in WORKFLOWS.items():
            for job_name, job in (wf.get("jobs") or {}).items():
                if "permissions" not in job:
                    continue
                with self.subTest(workflow=wf_name, job=job_name):
                    allowed = RAISED.get((wf_name, job_name), {})
                    for scope, level in scopes(job["permissions"]).items():
                        if (scope, level) == ("contents", "read") or level == "none":
                            continue
                        self.assertEqual(allowed.get(scope), level,
                                         f"{wf_name}:{job_name} asks for {scope}: {level}; only "
                                         f"{RAISED} may be raised, by those jobs")


class EachJobAssumesItsOwnRole(unittest.TestCase):
    def test_only_plan_and_deploy_configure_aws(self):
        jobs = sorted((wf, job) for wf, job, _ in credential_steps())
        self.assertEqual(jobs, [("ci.yaml", "deploy"), ("ci.yaml", "plan")])

    def test_each_job_assumes_its_role_by_oidc(self):
        for _, job, step in credential_steps():
            with self.subTest(job=job):
                inputs = {k: " ".join(str(v).split()) for k, v in (step.get("with") or {}).items()}
                self.assertRegex(inputs.get("role-to-assume", ""),
                                 rf"^arn:aws:iam::{ACCOUNT_SECRET}:role/{re.escape(ROLE_FOR_JOB[job])}$",
                                 f"the {job} job must assume {ROLE_FOR_JOB[job]}, by the account secret")
                self.assertEqual(inputs.get("mask-aws-account-id"), "True",
                                 "the logs of a public repository must not carry the account")
                for forbidden in ("aws-access-key-id", "aws-secret-access-key", "aws-session-token",
                                  "role-chaining", "web-identity-token-file", "audience"):
                    self.assertNotIn(forbidden, inputs, f"the {job} job passes {forbidden}")

    def test_only_those_two_jobs_get_an_oidc_token(self):
        def grants_id_token(perms) -> bool:
            # `write-all` grants id-token too; a mapping grants it only by naming it.
            if isinstance(perms, str):
                return perms.strip().lower() == "write-all"
            return isinstance(perms, dict) and str(perms.get("id-token", "")).lower() == "write"

        for wf_name, wf in WORKFLOWS.items():
            with self.subTest(workflow=wf_name, where="workflow"):
                self.assertFalse(grants_id_token(wf.get("permissions")),
                                 "a workflow-level id-token permission hands the token to every job")
            for job_name, job in (wf.get("jobs") or {}).items():
                perms = job.get("permissions")
                with self.subTest(workflow=wf_name, job=job_name):
                    if (wf_name, job_name) in {("ci.yaml", "plan"), ("ci.yaml", "deploy")}:
                        self.assertIsInstance(perms, dict, "name each permission; no write-all")
                        self.assertEqual(perms.get("id-token"), "write")
                    else:
                        self.assertFalse(grants_id_token(perms), f"{wf_name}:{job_name} can request an OIDC token")

    def test_the_release_runs_in_the_production_environment_and_the_gate_in_none(self):
        # The deploy role trusts environment:production; the plan role trusts pull_request and
        # main's ref, which is the subject a job WITHOUT an environment gets.
        self.assertEqual(environment_name(CI["jobs"]["deploy"]), DEPLOY_ENVIRONMENT)
        self.assertIsNone(environment_name(CI["jobs"]["plan"]),
                          "an environment changes the plan job's OIDC subject, and its role would refuse it")


class TheWorkflowAndTheTerraformAgree(unittest.TestCase):
    def setUp(self):
        self.tf = read(OIDC_TF)

    def test_every_role_the_workflow_assumes_is_declared(self):
        for job, role in ROLE_FOR_JOB.items():
            with self.subTest(job=job):
                self.assertRegex(self.tf, rf'(?m)^\s*name\s*=\s*"{re.escape(role)}"\s*$')

    def test_the_deploy_role_trusts_the_environment_the_job_names(self):
        self.assertIn(f':environment:{DEPLOY_ENVIRONMENT}"', self.tf)
        self.assertIsNotNone(re.search(r'"\$\{local\.github_sub_repo\}:pull_request"', self.tf))
        self.assertIsNotNone(re.search(r'"\$\{local\.github_sub_repo\}:ref:refs/heads/main"', self.tf))

    def test_each_role_holds_exactly_its_policies(self):
        # Another attachment or an inline policy on a CI role -- ReadOnlyAccess back on the deploy role,
        # a write on the plan role, which every pull request can assume -- would pass the tftest, which
        # checks the attachments it names. So count every way to reach the roles, as text: a reference
        # to the resource, the role's name written out, and the role's own policy arguments.
        text = "\n".join(re.sub(r"(?m)#.*$", "", read(p)) for p in sorted(TF.glob("*.tf")))
        expected = {"github_plan": 1, "github_deploy": 3}  # github_read | github_read, writes, cicd_infra
        for resource, count in expected.items():
            with self.subTest(role=resource):
                refs = re.findall(rf"\baws_iam_role\.{resource}\b", text)
                self.assertEqual(len(refs), count,
                                 f"aws_iam_role.{resource} is referenced {len(refs)} times; only its "
                                 f"{count} attachment(s) in cicd-oidc.tf may name it")
        for role in ROLE_FOR_JOB.values():
            with self.subTest(name=role):
                self.assertEqual(len(re.findall(rf'"{re.escape(role)}"', text)), 1,
                                 f'"{role}" may appear once, as the role\'s own name')
        for argument in ("inline_policy", "managed_policy_arns"):
            with self.subTest(argument=argument):
                self.assertNotIn(argument, read(OIDC_TF))

    def test_ci_never_applies_its_own_identities(self):
        # Same rule as cicd-iam-policy.tf: if a deploy step could apply them, CI could grant itself.
        ci_text = read(REPO / ".github" / "workflows" / "ci.yaml")
        for address in ("aws_iam_role.github_", "aws_iam_policy.github_", "aws_iam_role_policy_attachment.github_",
                        "aws_iam_policy.cicd_infra"):
            with self.subTest(address=address):
                self.assertNotIn(f"-target={address}", ci_text)


if __name__ == "__main__":
    unittest.main()
