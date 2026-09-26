#!/usr/bin/env python3
"""Tests for scripts/drift_gate_iam.py, against the CI policy as DECLARED in cicd-iam-policy.tf.

No AWS and no Terraform: the plans are the JSON `terraform show -json` prints, built here.

    python3 -m unittest discover -s tests/scripts -p 'test_drift_gate*.py'
"""

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts"))

import drift_gate_iam as gate  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
# A fake account, assembled so no line of this file is an account-bearing ARN (.github/workflows/no-account-id.yml).
ACCOUNT = "1111" + "22223333"
TASK_ROLE = "facilitator-production-ecs-task"
EXECUTION_ROLE = "facilitator-production-ecs-execution"
LAMBDA_ROLE = "facilitator-production-balances-lambda"


def declared_ci_policy() -> dict:
    """The `jsonencode({...})` body of aws_iam_policy.cicd_infra, with the account interpolated."""
    source = (REPO / "terraform" / "environments" / "production" / "cicd-iam-policy.tf").read_text(encoding="utf-8")
    start = source.index("policy = jsonencode(") + len("policy = jsonencode(")
    depth = 0
    for i, ch in enumerate(source[start:], start):
        depth += ch == "{"
        depth -= ch == "}"
        if ch == "}" and depth == 0:
            body = source[start : i + 1]
            break
    body = "\n".join(line for line in body.split("\n") if not line.strip().startswith("#"))
    return json.loads(body.replace("${data.aws_caller_identity.current.account_id}", ACCOUNT))


def policy_resource(document: dict, with_arn: bool = True) -> dict:
    values = {"name": "facilitator-cicd-infra", "policy": json.dumps(document)}
    if with_arn:
        values["arn"] = "arn:aws:iam::" + ACCOUNT + ":policy/facilitator-cicd-infra"
    return {"address": gate.CI_POLICY_ADDRESS, "type": "aws_iam_policy", "values": values}


def allow_put_role_policy_on(role: str, condition: dict | None = None) -> dict:
    """A policy that grants PutRolePolicy on one role and nothing else, optionally under a Condition."""
    statement = {"Effect": "Allow", "Action": "iam:PutRolePolicy",
                 "Resource": "arn:aws:iam::" + ACCOUNT + ":role/" + role}
    if condition is not None:
        statement["Condition"] = condition
    return {"Version": "2012-10-17", "Statement": [statement]}


def role_policy_change(address: str, role: str, actions=("update",), unknown_role=False) -> dict:
    after = {"name": "secrets-access", "role": None if unknown_role else role, "policy": "{}"}
    return {
        "address": address,
        "type": "aws_iam_role_policy",
        "change": {
            "actions": list(actions),
            "before": None if "create" in actions and "delete" not in actions else {"name": "secrets-access", "role": role},
            "after": None if actions == ("delete",) else after,
            "after_unknown": {"role": True} if unknown_role else {},
        },
    }


def plan(changes, policy=True, where="prior_state") -> dict:
    doc = {"resource_changes": list(changes)}
    if policy:
        resources = [policy_resource(declared_ci_policy())]
        if where == "prior_state":
            doc["prior_state"] = {"values": {"root_module": {"resources": resources}}}
        else:
            doc["planned_values"] = {"root_module": {"resources": resources}}
    return doc


def run(targeted: dict, full: dict | None = None):
    with tempfile.TemporaryDirectory() as d:
        t = os.path.join(d, "targeted.json")
        Path(t).write_text(json.dumps(targeted))
        argv = ["--targeted", t, "--summary", os.path.join(d, "summary.md")]
        if full is not None:
            f = os.path.join(d, "full.json")
            Path(f).write_text(json.dumps(full))
            argv += ["--full", f]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = gate.main(argv)
        summary = Path(os.path.join(d, "summary.md")).read_text()
    return code, out.getvalue(), summary


SECRETS = "aws_iam_role_policy.secrets_access"
HEDERA = "aws_iam_role_policy.hedera_settlement_access"
LAMBDA = "aws_iam_role_policy.balances_lambda_secrets"


class TheDeclaredPolicy(unittest.TestCase):
    """What the real statements allow, so a change to cicd-iam-policy.tf moves these answers with it."""

    def setUp(self):
        self.doc = declared_ci_policy()

    def arn(self, role):
        return f"arn:aws:iam::{ACCOUNT}:role/{role}"

    def test_the_task_role_is_writable(self):
        self.assertEqual(gate.evaluate(self.doc, "iam:PutRolePolicy", self.arn(TASK_ROLE)), "allow")
        self.assertEqual(gate.evaluate(self.doc, "iam:DeleteRolePolicy", self.arn(TASK_ROLE)), "allow")

    def test_every_other_role_is_denied(self):
        for role in (EXECUTION_ROLE, LAMBDA_ROLE, "anything-else"):
            self.assertEqual(gate.evaluate(self.doc, "iam:PutRolePolicy", self.arn(role)), "explicit_deny", role)

    def test_policies_cannot_be_versioned(self):
        arn = f"arn:aws:iam::{ACCOUNT}:policy/facilitator-cicd-infra"
        self.assertEqual(gate.evaluate(self.doc, "iam:CreatePolicyVersion", arn), "explicit_deny")

    def test_what_nothing_mentions_is_not_allowed(self):
        self.assertEqual(gate.evaluate(self.doc, "iam:DeleteRole", self.arn(TASK_ROLE)), "no_allow")


class TheGate(unittest.TestCase):
    def test_the_2026_09_26_shape_is_red_with_the_command(self):
        code, out, summary = run(plan([role_policy_change(SECRETS, EXECUTION_ROLE)]))
        self.assertEqual(code, 1)
        self.assertIn(f"::error title=IAM change CI cannot apply::{SECRETS}", out)
        self.assertIn(f"-target={SECRETS}", summary)
        self.assertIn("terraform apply -input=false", summary)
        self.assertIn("v1.9.8", summary)
        self.assertIn("BEFORE merging", summary)
        self.assertIn(f"role {EXECUTION_ROLE}", summary)

    def test_a_task_role_policy_is_green(self):
        code, out, summary = run(plan([role_policy_change(HEDERA, TASK_ROLE)]))
        self.assertEqual(code, 0, summary)
        self.assertNotIn("::error", out)
        self.assertIn("| yes |", summary)

    def test_only_the_blocked_ones_go_in_the_command(self):
        code, _, summary = run(plan([
            role_policy_change(HEDERA, TASK_ROLE),
            role_policy_change(SECRETS, EXECUTION_ROLE),
            role_policy_change(LAMBDA, LAMBDA_ROLE, actions=("create",)),
        ]))
        self.assertEqual(code, 1)
        command = summary.split("```sh", 1)[1].split("```", 1)[0]
        self.assertIn(f"-target={SECRETS}", command)
        self.assertIn(f"-target={LAMBDA}", command)
        self.assertNotIn(HEDERA, command)

    def test_a_role_not_known_until_apply_is_red(self):
        code, _, summary = run(plan([role_policy_change(SECRETS, TASK_ROLE, unknown_role=True)]))
        self.assertEqual(code, 1)
        self.assertIn("role not known until apply", summary)

    def test_a_replacement_needs_both_calls(self):
        # Replacing a task-role policy is a delete + a create, both allowed.
        code, _, _ = run(plan([role_policy_change(HEDERA, TASK_ROLE, actions=("delete", "create"))]))
        self.assertEqual(code, 0)

    def test_other_iam_types(self):
        role = {"address": "aws_iam_role.ecs_task_execution", "type": "aws_iam_role",
                "change": {"actions": ["update"], "before": {"name": EXECUTION_ROLE}, "after": {"name": EXECUTION_ROLE}}}
        profile = {"address": "aws_iam_instance_profile.x", "type": "aws_iam_instance_profile",
                   "change": {"actions": ["create"], "before": None, "after": {"name": "x"}}}
        for change in (role, profile):
            code, _, _ = run(plan([change]))
            self.assertEqual(code, 1, change["address"])

    def test_no_ops_reads_and_non_iam_changes_are_ignored(self):
        noop = role_policy_change(SECRETS, EXECUTION_ROLE, actions=("no-op",))
        task_def = {"address": "aws_ecs_task_definition.facilitator", "type": "aws_ecs_task_definition",
                    "change": {"actions": ["create"], "before": None, "after": {}}}
        code, out, summary = run(plan([noop, task_def]))
        self.assertEqual(code, 0)
        self.assertIn("changes no IAM resource", summary)

    def test_without_the_ci_policy_nothing_is_assumed_writable(self):
        code, _, summary = run(plan([role_policy_change(HEDERA, TASK_ROLE)], policy=False))
        self.assertEqual(code, 1)
        self.assertIn("is not in the plan", summary)

    def test_the_live_policy_comes_from_the_full_plan_first(self):
        # The targeted plan carries no policy; the full plan's refreshed state does.
        code, _, summary = run(plan([role_policy_change(HEDERA, TASK_ROLE)], policy=False),
                               full=plan([], policy=True))
        self.assertEqual(code, 0, summary)
        self.assertIn("(live)", summary)

    def test_a_declared_but_unapplied_policy_says_so(self):
        code, _, summary = run(plan([role_policy_change(HEDERA, TASK_ROLE)], where="planned_values"))
        self.assertEqual(code, 0)
        self.assertIn("declared, not yet in state", summary)

    def test_the_live_policy_wins_over_the_declared_one(self):
        # Live (state) still denies the execution role; the PR declares a policy that would allow it. The deploy
        # runs against the LIVE one, so this is red until the new policy is applied by hand.
        targeted = {
            "resource_changes": [role_policy_change(SECRETS, EXECUTION_ROLE)],
            "prior_state": {"values": {"root_module": {"resources": [policy_resource(declared_ci_policy())]}}},
            "planned_values": {"root_module": {"resources": [
                policy_resource(allow_put_role_policy_on(EXECUTION_ROLE))]}},
        }
        code, _, summary = run(targeted)
        self.assertEqual(code, 1, summary)
        self.assertIn("(live)", summary)

    def test_an_allow_under_a_condition_does_not_grant(self):
        doc = allow_put_role_policy_on(EXECUTION_ROLE, condition={"Bool": {"aws:MultiFactorAuthPresent": "true"}})
        targeted = {"resource_changes": [role_policy_change(SECRETS, EXECUTION_ROLE)],
                    "prior_state": {"values": {"root_module": {"resources": [policy_resource(doc)]}}}}
        code, _, summary = run(targeted)
        self.assertEqual(code, 1, summary)
        self.assertIn("no statement grants CI", summary)
        # The same Allow without the Condition does grant: the Condition is what refused it.
        plain = {"resource_changes": [role_policy_change(SECRETS, EXECUTION_ROLE)],
                 "prior_state": {"values": {"root_module": {"resources": [
                     policy_resource(allow_put_role_policy_on(EXECUTION_ROLE))]}}}}
        self.assertEqual(run(plain)[0], 0)

    def test_an_arn_without_an_account_is_never_granted(self):
        # The CI policy's own ARN is what tells the gate the account. Without it the role ARN is unknown, and even
        # a wildcard Allow must not grant a change on it.
        wildcard = {"Version": "2012-10-17",
                    "Statement": [{"Effect": "Allow", "Action": "iam:PutRolePolicy", "Resource": "*"}]}
        targeted = {"resource_changes": [role_policy_change(HEDERA, TASK_ROLE)],
                    "prior_state": {"values": {"root_module": {"resources": [
                        policy_resource(wildcard, with_arn=False)]}}}}
        code, _, summary = run(targeted)
        self.assertEqual(code, 1, summary)

    def test_no_account_id_or_arn_is_ever_printed(self):
        _, out, summary = run(plan([role_policy_change(SECRETS, EXECUTION_ROLE),
                                    role_policy_change(HEDERA, TASK_ROLE)]))
        for text in (out, summary):
            self.assertNotIn(ACCOUNT, text)
            self.assertNotIn("arn:aws", text)


class TheWorkflow(unittest.TestCase):
    """The step exists in the drift-gate job and runs even when an earlier step went red."""

    def test_ci_runs_the_gate_on_the_targeted_plan(self):
        ci = (REPO / ".github" / "workflows" / "ci.yaml").read_text(encoding="utf-8")
        plan_job = ci.split("  plan:\n", 1)[1].split("\n  deploy:\n", 1)[0]
        self.assertIn("scripts/drift_gate_iam.py", plan_job)
        step = plan_job.split("scripts/drift_gate_iam.py", 1)[0].rsplit("- name:", 1)[1]
        self.assertIn("if: '!cancelled()'", step)
        self.assertIn("terraform show -json targeted.tfplan", step)


if __name__ == "__main__":
    unittest.main()
