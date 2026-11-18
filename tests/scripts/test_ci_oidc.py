"""The CI reaches AWS by GitHub OIDC, each job with its own role (X402-SEG-CI, 2026-11-18).

Before this, plan and deploy authenticated with the access key of the IAM user
github-actions-facilitator-deploy, stored as repository secrets of a public repository; the
drift gate ran with that write-capable key on every pull request. Now:

  - plan   assumes facilitator-github-plan, read-only, trusted for pull_request and main;
  - deploy assumes facilitator-github-deploy, trusted for the `production` environment alone.

What the roles trust and grant is held by terraform/environments/production/tests/github_oidc.tftest.hcl.
This file holds the workflow's half, and that the two halves name the same roles and environment.

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
        self.assertEqual(sorted(env), ["EVENT", "HAS_ACCOUNT"], f"preflight's check reads {sorted(env)}")
        self.assertRegex(env["HAS_ACCOUNT"], r"^\$\{\{\s*secrets\.AWS_ACCOUNT_ID\s*!=\s*''\s*\}\}$")
        self.assertRegex(env["EVENT"], r"^\$\{\{\s*github\.event_name\s*\}\}$")


class APreflightWithoutTheAccount(unittest.TestCase):
    """Before the switch-on, plan and deploy are skipped. A push or a pull request stays green with a
    notice; a manual run -- a release -- fails, because green would read as shipped."""

    def run_check(self, has_account: str, event: str):
        if os.name != "posix" or shutil.which("bash") is None:
            self.skipTest("needs a POSIX bash, as the runner has")
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out"
            out.write_text("", encoding="utf-8")
            env = {"PATH": os.environ.get("PATH", ""), "HAS_ACCOUNT": has_account, "EVENT": event,
                   "GITHUB_OUTPUT": str(out)}
            done = subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c",
                                   str(CI["jobs"]["preflight"]["steps"][0]["run"])],
                                  env=env, capture_output=True, text=True, check=False)
            return done, out.read_text(encoding="utf-8")

    def test_with_the_account_every_event_plans_and_may_deploy(self):
        for event in ("push", "pull_request", "workflow_dispatch"):
            with self.subTest(event=event):
                done, out = self.run_check("true", event)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertIn("deploy=true", out)

    def test_without_it_a_push_or_pull_request_skips_quietly(self):
        for event in ("push", "pull_request"):
            with self.subTest(event=event):
                done, out = self.run_check("false", event)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertIn("deploy=false", out)

    def test_without_it_a_manual_run_fails(self):
        done, out = self.run_check("false", "workflow_dispatch")
        self.assertNotEqual(done.returncode, 0, "a release with no AWS account went green and shipped nothing")
        self.assertIn("::error", done.stdout)
        self.assertNotIn("deploy=true", out)


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
