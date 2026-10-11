#!/usr/bin/env python3
"""Tests for scripts/drift_gate_iam.py, against the CI policy as DECLARED in cicd-iam-policy.tf.

No AWS and no Terraform: the plans are the JSON `terraform show -json` prints, built here.

    python3 -m unittest discover -s tests/scripts -p 'test_drift_gate*.py'
"""

import contextlib
import io
import json
import os
import re
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
CLUSTER_ARN = "arn:aws:ecs:us-east-2:" + ACCOUNT + ":cluster/facilitator-production"
SERVICE_ARN = "arn:aws:ecs:us-east-2:" + ACCOUNT + ":service/facilitator-production/facilitator-production"
TASK_DEFINITION_ARN = "arn:aws:ecs:us-east-2:" + ACCOUNT + ":task-definition/facilitator-production"


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

    def test_the_bazaar_token_key_cannot_be_used_to_decrypt(self):
        # Owner's decision 230: BAZAAR_ADMIN_TOKEN is an SSM SecureString on its own KMS key, and the
        # CI user reads only ciphertext. Only that key: a deny on every key would also refuse the
        # AWS-managed keys the deploy reads through.
        key = f"arn:aws:kms:us-east-2:{ACCOUNT}:key/0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e"
        other = f"arn:aws:kms:us-east-2:{ACCOUNT}:key/1f1f1f1f-2e2e-3d3d-4c4c-5b5b5b5b5b5b"
        doc = json.loads(json.dumps(self.doc).replace("${aws_kms_key.bazaar_admin_token.arn}", key))
        self.assertEqual(gate.evaluate(doc, "kms:Decrypt", key), "explicit_deny")
        self.assertEqual(gate.evaluate(doc, "kms:Decrypt", other), "no_allow")
        deny = [s for s in doc["Statement"] if s.get("Sid") == "DenyBazaarTokenDecrypt"]
        self.assertEqual(len(deny), 1)
        self.assertNotIn("Condition", deny[0], "a condition can switch the deny off")
        for s in doc["Statement"]:
            actions = s.get("Action") if isinstance(s.get("Action"), list) else [s.get("Action")]
            if s.get("Effect") == "Allow":
                self.assertFalse(any(a in ("kms:Decrypt", "kms:*", "*") for a in actions), s.get("Sid"))

    def test_policies_cannot_be_versioned(self):
        arn = f"arn:aws:iam::{ACCOUNT}:policy/facilitator-cicd-infra"
        self.assertEqual(gate.evaluate(self.doc, "iam:CreatePolicyVersion", arn), "explicit_deny")

    def test_what_nothing_mentions_is_not_allowed(self):
        self.assertEqual(gate.evaluate(self.doc, "iam:DeleteRole", self.arn(TASK_ROLE)), "no_allow")

    def test_ci_may_update_the_cluster_every_image_deploy_drags_in(self):
        # The deploy of #115 died on this call (2026-10-07): the cluster is a dependency of
        # the service every image deploy targets, so its pending changes ride along.
        self.assertEqual(gate.evaluate(self.doc, "ecs:UpdateCluster", CLUSTER_ARN), "allow")

    def statement(self, sid: str) -> dict:
        found = [s for s in self.doc["Statement"] if s.get("Sid") == sid]
        self.assertEqual(len(found), 1, sid)
        return found[0]

    def test_the_cluster_statement_is_one_call_on_one_cluster(self):
        self.assertEqual(self.statement("EcsClusterUpdate"),
                         {"Sid": "EcsClusterUpdate", "Effect": "Allow", "Action": ["ecs:UpdateCluster"],
                          "Resource": CLUSTER_ARN})

    def test_the_untag_statement_is_one_call_on_the_three_deploy_resources(self):
        self.assertEqual(self.statement("EcsDeployUntag"),
                         {"Sid": "EcsDeployUntag", "Effect": "Allow", "Action": ["ecs:UntagResource"],
                          "Resource": [CLUSTER_ARN, SERVICE_ARN, TASK_DEFINITION_ARN + ":*"]})

    def test_the_cluster_grant_reaches_nothing_else(self):
        for arn in (CLUSTER_ARN.replace(":cluster/facilitator-production", ":cluster/facilitator-staging"),
                    CLUSTER_ARN + "-x",
                    CLUSTER_ARN.replace(":us-east-2:", ":us-east-1:"),
                    CLUSTER_ARN.replace(":cluster/", ":service/") + "/facilitator-production"):
            self.assertEqual(gate.evaluate(self.doc, "ecs:UpdateCluster", arn), "no_allow", arn)
        for action in ("ecs:DeleteCluster", "ecs:PutClusterCapacityProviders"):
            self.assertEqual(gate.evaluate(self.doc, action, CLUSTER_ARN), "no_allow", action)

    def test_ci_may_untag_what_an_image_deploy_reaches(self):
        # TagResource lives in the inline policy on "*"; its inverse lives here, on the three ECS resources.
        for arn in (CLUSTER_ARN, SERVICE_ARN, TASK_DEFINITION_ARN + ":475"):
            self.assertEqual(gate.evaluate(self.doc, "ecs:UntagResource", arn), "allow", arn)

    def test_the_untag_grant_reaches_nothing_else(self):
        for arn in (SERVICE_ARN.replace("/facilitator-production/facilitator-production",
                                        "/facilitator-production/observability-production"),
                    TASK_DEFINITION_ARN.replace("facilitator-production", "observability-production") + ":3",
                    TASK_DEFINITION_ARN.replace("facilitator-production", "facilitator-production-x") + ":1",
                    CLUSTER_ARN.replace(":cluster/facilitator-production", ":cluster/facilitator-staging"),
                    CLUSTER_ARN.replace(":us-east-2:", ":us-east-1:")):
            self.assertEqual(gate.evaluate(self.doc, "ecs:UntagResource", arn), "no_allow", arn)

    def test_the_granted_cluster_is_the_one_the_deploy_reaches(self):
        prod = REPO / "terraform" / "environments" / "production"
        main_tf = (prod / "main.tf").read_text(encoding="utf-8")
        cluster = main_tf.split('resource "aws_ecs_cluster" "main" {', 1)[1].split("\n}\n", 1)[0]
        self.assertIn('name = "facilitator-${var.environment}"', cluster)
        service = main_tf.split('resource "aws_ecs_service" "facilitator" {', 1)[1].split("\n}\n", 1)[0]
        self.assertRegex(service, r"\n\s*cluster\s*=\s*aws_ecs_cluster\.main\.")
        # The untag grant names the service and the task-definition family the same way.
        self.assertRegex(service, r'\n\s*name\s*=\s*"facilitator-\$\{var\.environment\}"\n')
        task_def = main_tf.split('resource "aws_ecs_task_definition" "facilitator" {', 1)[1].split("\n}\n", 1)[0]
        self.assertRegex(task_def, r'\n\s*family\s*=\s*"facilitator-\$\{var\.environment\}"\n')
        targets = re.findall(r"[-]target=([A-Za-z0-9_.]+)", (REPO / ".github" / "workflows" / "ci.yaml").read_text(encoding="utf-8"))
        self.assertIn("aws_ecs_service.facilitator", targets)
        self.assertIn("aws_ecs_task_definition.facilitator", targets)
        self.assertRegex((prod / "variables.tf").read_text(encoding="utf-8"),
                         r'variable "environment" \{[^}]*default\s*=\s*"production"')
        self.assertRegex((prod / "production.auto.tfvars").read_text(encoding="utf-8"),
                         r'(?m)^environment\s*=\s*"production"\s*$')


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

    def test_a_plan_that_errored_is_red_not_clean(self):
        """Terraform 1.9 still writes the plan file when planning fails (PR #122's run)."""
        errored = plan([])
        errored["errored"] = True
        code, out, summary = run(errored)
        self.assertEqual(code, 1, summary)
        self.assertNotIn("Clean", summary)
        self.assertIn("Not evaluated", summary)
        self.assertIn("::error title=IAM changes not evaluated::", out)
        code, _, summary = run(plan([]))
        self.assertEqual(code, 0, summary)
        self.assertIn("Clean", summary)

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


def ci_policy_without(sid: str) -> dict:
    """The declared policy minus one statement: what is live until a new statement is applied by hand."""
    doc = declared_ci_policy()
    doc["Statement"] = [s for s in doc["Statement"] if s.get("Sid") != sid]
    return doc


def cluster_change(actions=("update",), insights=("enabled", "disabled"), tags_after=None, unknown=None) -> dict:
    """aws_ecs_cluster.main as `terraform show -json` prints it; by default #115's change (Insights off)."""
    before = {"name": "facilitator-production", "arn": CLUSTER_ARN,
              "setting": [{"name": "containerInsights", "value": insights[0]}],
              "configuration": [], "service_connect_defaults": [], "tags": {"Name": "facilitator-production"}}
    after = json.loads(json.dumps(before))
    after["setting"][0]["value"] = insights[1]
    if tags_after is not None:
        after["tags"] = tags_after
    if "create" in actions:
        after.pop("arn")
    return {"address": "aws_ecs_cluster.main", "type": "aws_ecs_cluster",
            "change": {"actions": list(actions),
                       "before": None if actions == ("create",) else before,
                       "after": None if actions == ("delete",) else after,
                       "after_unknown": unknown if unknown is not None else
                       {"setting": [{}], "configuration": [], "service_connect_defaults": [], "tags": {}}}}


def live_plan(changes, live_doc: dict, declared_doc: dict | None = None) -> dict:
    doc = {"resource_changes": list(changes),
           "prior_state": {"values": {"root_module": {"resources": [policy_resource(live_doc)]}}}}
    if declared_doc is not None:
        doc["planned_values"] = {"root_module": {"resources": [policy_resource(declared_doc)]}}
    return doc


CLUSTER = "aws_ecs_cluster.main"
NO_CLUSTER_GRANT = ci_policy_without("EcsClusterUpdate")


class TheClusterTheDeployDragsIn(unittest.TestCase):
    """aws_ecs_cluster.main rides into every image deploy as a dependency of the service. On 2026-10-07 the deploy
    of #115 died on ecs:UpdateCluster; this half of the gate turns that red before the merge."""

    def test_the_2026_10_07_shape_is_red_with_the_command(self):
        code, out, summary = run(live_plan([cluster_change()], NO_CLUSTER_GRANT))
        self.assertEqual(code, 1, summary)
        self.assertIn(f"::error title=IAM change CI cannot apply::{CLUSTER}", out)
        self.assertIn("no statement grants CI ecs:UpdateCluster", summary)
        self.assertIn("cluster facilitator-production", summary)
        command = summary.split("```sh", 1)[1].split("```", 1)[0]
        self.assertIn(f"-target={CLUSTER}", command)

    def test_with_the_grant_live_the_cluster_change_is_green(self):
        code, out, summary = run(live_plan([cluster_change()], declared_ci_policy()))
        self.assertEqual(code, 0, summary)
        self.assertNotIn("::error", out)
        self.assertIn(f"| `update` | `{CLUSTER}` | cluster facilitator-production | yes |", summary)

    def test_the_live_policy_decides_not_the_declared_one(self):
        # The PR declares the grant; until it is applied by hand the deploy runs against the live policy.
        code, _, summary = run(live_plan([cluster_change()], NO_CLUSTER_GRANT, declared_doc=declared_ci_policy()))
        self.assertEqual(code, 1, summary)
        self.assertIn("(live)", summary)

    def test_a_tags_only_change_needs_no_cluster_grant(self):
        # Same setting on both sides and after_unknown's `[{}]` for a known block list: no UpdateCluster.
        change = cluster_change(insights=("disabled", "disabled"), tags_after={"Name": "facilitator-production",
                                                                               "Team": "x"})
        code, out, summary = run(live_plan([change], NO_CLUSTER_GRANT))
        self.assertEqual(code, 0, summary)
        self.assertNotIn("::error", out)
        self.assertIn("changes no IAM resource", summary)

    def test_a_setting_unknown_until_apply_needs_the_grant(self):
        change = cluster_change(insights=("disabled", "disabled"),
                                unknown={"setting": [{"value": True}], "configuration": [],
                                         "service_connect_defaults": [], "tags": {}})
        code, _, summary = run(live_plan([change], NO_CLUSTER_GRANT))
        self.assertEqual(code, 1, summary)
        self.assertIn("ecs:UpdateCluster", summary)

    def test_every_attribute_that_goes_through_update_cluster_counts(self):
        for attr, value in (("configuration", [{"execute_command_configuration": [{"logging": "DEFAULT"}]}]),
                            ("service_connect_defaults", [{"namespace": "x"}])):
            with self.subTest(attr=attr):
                change = cluster_change(insights=("disabled", "disabled"))
                change["change"]["after"][attr] = value
                self.assertEqual(run(live_plan([change], NO_CLUSTER_GRANT))[0], 1, attr)

    def test_a_create_or_a_replacement_is_red_even_with_the_grant(self):
        for actions, calls in ((("delete", "create"), ("ecs:DeleteCluster", "ecs:CreateCluster")),
                               (("create",), ("ecs:CreateCluster",)),
                               (("delete",), ("ecs:DeleteCluster",))):
            with self.subTest(actions=actions):
                code, _, summary = run(live_plan([cluster_change(actions=actions)], declared_ci_policy()))
                self.assertEqual(code, 1, summary)
                for call in calls:
                    self.assertIn(call, summary)
                # Replacing the production cluster under the running service is never a command to paste.
                self.assertIn("**STOP**", summary)
                self.assertNotIn(f"-target={CLUSTER}", summary)

    def test_a_stop_does_not_swallow_the_command_for_the_other_rows(self):
        code, _, summary = run(live_plan([cluster_change(actions=("delete", "create")),
                                          role_policy_change(SECRETS, EXECUTION_ROLE)], declared_ci_policy()))
        self.assertEqual(code, 1, summary)
        self.assertIn("**STOP**", summary)
        command = summary.split("```sh", 1)[1].split("```", 1)[0]
        self.assertIn(f"-target={SECRETS}", command)
        self.assertNotIn(CLUSTER, command)

    def test_null_and_an_empty_block_list_are_the_same_value(self):
        # The SDK reads null and [] alike and sends no UpdateCluster between them; an added tag is not evaluated.
        change = cluster_change(insights=("disabled", "disabled"),
                                tags_after={"Name": "facilitator-production", "Team": "x"})
        change["change"]["before"]["service_connect_defaults"] = None
        code, _, summary = run(live_plan([change], NO_CLUSTER_GRANT))
        self.assertEqual(code, 0, summary)

    def test_service_and_task_definition_changes_are_not_evaluated(self):
        # Their calls (UpdateService, RegisterTaskDefinition) live in the inline policy, outside the plan:
        # evaluating them against the managed one would turn every image deploy red.
        service = {"address": "aws_ecs_service.facilitator", "type": "aws_ecs_service",
                   "change": {"actions": ["update"], "before": {"name": "facilitator-production"},
                              "after": {"name": "facilitator-production"}}}
        task_def = {"address": "aws_ecs_task_definition.facilitator", "type": "aws_ecs_task_definition",
                    "change": {"actions": ["delete", "create"], "before": {}, "after": {}}}
        code, _, summary = run(live_plan([service, task_def], NO_CLUSTER_GRANT))
        self.assertEqual(code, 0, summary)

    def test_no_account_id_or_arn_is_printed_for_the_cluster(self):
        _, out, summary = run(live_plan([cluster_change()], NO_CLUSTER_GRANT))
        for text in (out, summary):
            self.assertNotIn(ACCOUNT, text)
            self.assertNotIn("arn:aws", text)


NO_UNTAG_GRANT = ci_policy_without("EcsDeployUntag")
DEFAULT_TAGS = {"Project": "facilitator", "Environment": "production", "ManagedBy": "terraform",
                "Owner": "ultravioleta-dao"}


def ecs_tag_change(rtype: str, address: str, identity: dict, drop: tuple = ("Owner",), add: dict | None = None,
                   unknown_tags=None) -> dict:
    """An in-place update of an ECS resource whose tags_all loses `drop` and gains `add`."""
    before = dict(identity, tags_all=dict(DEFAULT_TAGS, Name="facilitator-production"))
    after_tags = {k: v for k, v in before["tags_all"].items() if k not in drop}
    after_tags.update(add or {})
    after = dict(identity, tags_all=after_tags)
    return {"address": address, "type": rtype,
            "change": {"actions": ["update"], "before": before, "after": after,
                       "after_unknown": {"tags_all": unknown_tags if unknown_tags is not None else {}}}}


SERVICE_IDENTITY = {"name": "facilitator-production", "id": SERVICE_ARN}
TASK_DEFINITION_IDENTITY = {"family": "facilitator-production", "arn": TASK_DEFINITION_ARN + ":475"}
CLUSTER_IDENTITY = {"name": "facilitator-production", "arn": CLUSTER_ARN}
DEPLOY_ECS = (("aws_ecs_service", "aws_ecs_service.facilitator", SERVICE_IDENTITY, "service facilitator-production"),
              ("aws_ecs_task_definition", "aws_ecs_task_definition.facilitator", TASK_DEFINITION_IDENTITY,
               "task definition facilitator-production"),
              ("aws_ecs_cluster", CLUSTER, CLUSTER_IDENTITY, "cluster facilitator-production"))


class TheUntagHalf(unittest.TestCase):
    """EcsDeployUntag lives in the managed policy, so the gate can read it: an update that drops a tag key from
    the cluster, the service or the task definition calls ecs:UntagResource, and without the grant the deploy
    stops half-way, the service already updated."""

    def test_a_dropped_tag_is_red_without_the_grant(self):
        for rtype, address, identity, label in DEPLOY_ECS:
            with self.subTest(resource=address):
                code, out, summary = run(live_plan([ecs_tag_change(rtype, address, identity)], NO_UNTAG_GRANT))
                self.assertEqual(code, 1, summary)
                self.assertIn(f"::error title=IAM change CI cannot apply::{address}", out)
                self.assertIn("no statement grants CI ecs:UntagResource", summary)
                self.assertIn(label, summary)
                command = summary.split("```sh", 1)[1].split("```", 1)[0]
                self.assertIn(f"-target={address}", command)

    def test_with_the_grant_live_a_dropped_tag_is_green(self):
        for rtype, address, identity, label in DEPLOY_ECS:
            with self.subTest(resource=address):
                code, _, summary = run(live_plan([ecs_tag_change(rtype, address, identity)], declared_ci_policy()))
                self.assertEqual(code, 0, summary)
                self.assertIn(f"| `update` | `{address}` | {label} | yes |", summary)

    def test_an_added_tag_is_not_evaluated(self):
        # TagResource lives in the inline policy, outside the plan.
        for rtype, address, identity, _ in DEPLOY_ECS:
            with self.subTest(resource=address):
                change = ecs_tag_change(rtype, address, identity, drop=(), add={"Team": "x"})
                code, _, summary = run(live_plan([change], NO_UNTAG_GRANT))
                self.assertEqual(code, 0, summary)

    def test_a_tag_map_unknown_until_apply_needs_the_grant(self):
        change = ecs_tag_change("aws_ecs_service", "aws_ecs_service.facilitator", SERVICE_IDENTITY, drop=(),
                                unknown_tags=True)
        self.assertEqual(run(live_plan([change], NO_UNTAG_GRANT))[0], 1)

    def test_a_value_unknown_until_apply_is_a_retag_not_an_untag(self):
        change = ecs_tag_change("aws_ecs_service", "aws_ecs_service.facilitator", SERVICE_IDENTITY, drop=(),
                                unknown_tags={"Owner": True})
        code, _, summary = run(live_plan([change], NO_UNTAG_GRANT))
        self.assertEqual(code, 0, summary)

    def test_a_replaced_task_definition_untags_nothing(self):
        # A new revision is a create; the old one is deregistered, not untagged.
        change = ecs_tag_change("aws_ecs_task_definition", "aws_ecs_task_definition.facilitator",
                                TASK_DEFINITION_IDENTITY)
        change["change"]["actions"] = ["delete", "create"]
        code, _, summary = run(live_plan([change], NO_UNTAG_GRANT))
        self.assertEqual(code, 0, summary)


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
