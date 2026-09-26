#!/usr/bin/env python3
"""Drift gate, IAM half: fail when the deploy's targeted plan changes IAM the CI user cannot write.

The deploy applies an explicit -target list, and that list includes IAM role policies
(`aws_iam_role_policy.secrets_access`, `...hedera_settlement_access`, `...balances_lambda_secrets`). The CI user
may write exactly one role's inline policies: `IamRolePolicyForTaskRoleOnly` allows `iam:PutRolePolicy` on the
task role, and `DenyPrivilegeEscalation` denies it, and every other IAM write, everywhere else
(terraform/environments/production/cicd-iam-policy.tf). That is on purpose -- CI must not grant itself
permissions -- and it has a consequence the rest of the drift gate could not see: a PR whose targeted plan changes
one of the OTHER IAM resources plans clean, merges green, and then the deploy fails half-way, with the new task
definition registered and the service moved while the policy it needs was refused (2026-09-26: the execution
role's `secrets_access`; the new tasks died on AccessDenied until it was applied by hand).

So this reads the targeted plan (`terraform show -json`), finds every aws_iam_* it would change, works out the
IAM API calls that change needs, and evaluates them against the CI policy AS IT IS LIVE (the refreshed
`aws_iam_policy.cicd_infra` in the plan's prior state; its declared value only if it is not in state yet). An
explicit Deny, or no Allow at all, is red -- the inline `facilitator-cicd` policy grants no IAM write beyond
`iam:PassRole` (docs/CICD_SETUP.md), so "no Allow here" is "no Allow anywhere". Red comes with the exact command
to apply it by hand, before the merge, on Terraform 1.9.8.

Prints GitHub annotations and appends a section to --summary. Never prints an account ID or an ARN.

    python3 scripts/drift_gate_iam.py --targeted targeted.json [--full full.json] [--summary "$GITHUB_STEP_SUMMARY"]

Exit code: 0 = the deploy can apply every IAM change in its plan; 1 = it cannot (or the policy could not be
found to decide).
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import sys

CI_POLICY_ADDRESS = "aws_iam_policy.cicd_infra"
TERRAFORM_VERSION = "1.9.8"
WORKDIR = "terraform/environments/production"

# The IAM calls each change needs, by resource type and plan action. A type not listed here is unknown, and
# unknown is red: an IAM change nobody has classified is exactly the one to look at by hand.
CALLS = {
    "aws_iam_role_policy": {
        "create": ["iam:PutRolePolicy"], "update": ["iam:PutRolePolicy"], "delete": ["iam:DeleteRolePolicy"]},
    "aws_iam_role_policy_attachment": {
        "create": ["iam:AttachRolePolicy"], "update": ["iam:AttachRolePolicy", "iam:DetachRolePolicy"],
        "delete": ["iam:DetachRolePolicy"]},
    "aws_iam_role": {
        "create": ["iam:CreateRole"], "update": ["iam:UpdateRole", "iam:UpdateAssumeRolePolicy"],
        "delete": ["iam:DeleteRole"]},
    "aws_iam_policy": {
        "create": ["iam:CreatePolicy"], "update": ["iam:CreatePolicyVersion"], "delete": ["iam:DeletePolicy"]},
}


def _walk(module: dict | None):
    if not module:
        return
    yield from module.get("resources", [])
    for child in module.get("child_modules", []):
        yield from _walk(child)


def ci_policy(plans: list[dict]) -> tuple[dict | None, str | None, str]:
    """(policy document, account id, where it came from). Live state first, then the declared value."""
    for key, label in (("prior_state", "live"), ("planned_values", "declared, not yet in state")):
        for plan in plans:
            section = plan.get(key) or {}
            root = section.get("values", section).get("root_module") if key == "prior_state" else section.get(
                "root_module")
            for resource in _walk(root):
                if resource.get("address") != CI_POLICY_ADDRESS:
                    continue
                values = resource.get("values") or {}
                try:
                    document = json.loads(values.get("policy") or "")
                except ValueError:
                    continue
                arn = values.get("arn") or ""
                account = arn.split(":")[4] if arn.count(":") >= 5 else None
                return document, account, label
    return None, None, "not found"


def _as_list(value) -> list:
    return value if isinstance(value, list) else ([] if value is None else [value])


def _matches(patterns, value: str, case_sensitive: bool) -> bool:
    for pattern in _as_list(patterns):
        if case_sensitive and fnmatch.fnmatchcase(value, pattern):
            return True
        if not case_sensitive and fnmatch.fnmatchcase(value.lower(), str(pattern).lower()):
            return True
    return False


def evaluate(document: dict, action: str, resource: str | None) -> str:
    """'allow', 'explicit_deny' or 'no_allow' for one IAM call. An unknown resource ARN never matches an Allow,
    and a statement with a Condition never grants (and always denies): no guessing in the permissive direction."""
    allowed = False
    for statement in _as_list(document.get("Statement")):
        effect = statement.get("Effect")
        if "Action" in statement:
            acts = _matches(statement["Action"], action, case_sensitive=False)
        else:
            acts = not _matches(statement.get("NotAction"), action, case_sensitive=False)
        if not acts:
            continue
        if resource is None:
            on_resource = effect == "Deny"
        elif "Resource" in statement:
            on_resource = _matches(statement["Resource"], resource, case_sensitive=True)
        else:
            on_resource = not _matches(statement.get("NotResource"), resource, case_sensitive=True)
        if not on_resource:
            continue
        if effect == "Deny":
            return "explicit_deny"
        if effect == "Allow" and "Condition" not in statement:
            allowed = True
    return "allow" if allowed else "no_allow"


def _value(change: dict, name: str):
    for side in ("after", "before"):
        values = change.get(side) or {}
        if values.get(name):
            return values[name]
    return None


def target_arn(rtype: str, change: dict, account: str | None) -> tuple[str | None, str]:
    """(ARN the call acts on, short label for humans). The label names the role or policy, never the account."""
    if rtype in ("aws_iam_role_policy", "aws_iam_role_policy_attachment"):
        role = _value(change, "role")
        if not role or (change.get("after_unknown") or {}).get("role"):
            return None, "role not known until apply"
        return (f"arn:aws:iam::{account}:role/{role}" if account else None), f"role {role}"
    if rtype == "aws_iam_role":
        name = _value(change, "name")
        return (f"arn:aws:iam::{account}:role/{name}" if account and name else None), f"role {name}"
    if rtype == "aws_iam_policy":
        name = _value(change, "name")
        arn = _value(change, "arn") or (f"arn:aws:iam::{account}:policy/{name}" if account and name else None)
        return arn, f"policy {name}"
    return None, rtype


def verdicts(targeted: dict, document: dict | None, account: str | None) -> list[dict]:
    """One row per aws_iam_* the targeted plan changes: the calls it needs and whether CI may make them."""
    rows = []
    for rc in targeted.get("resource_changes") or []:
        rtype = rc.get("type", "")
        actions = [a for a in rc.get("change", {}).get("actions", []) if a not in ("no-op", "read")]
        if not rtype.startswith("aws_iam_") or not actions:
            continue
        arn, label = target_arn(rtype, rc.get("change", {}), account)
        needed = [c for a in actions for c in CALLS.get(rtype, {}).get(a, [])]
        if rtype not in CALLS:
            outcome, why = "unmapped", "an IAM resource type this gate does not classify"
        elif document is None:
            outcome, why = "unknown", f"{CI_POLICY_ADDRESS} is not in the plan, so nothing says CI may"
        else:
            results = {call: evaluate(document, call, arn) for call in needed}
            refused = [c for c, r in results.items() if r != "allow"]
            if not refused:
                outcome, why = "allow", ""
            elif any(results[c] == "explicit_deny" for c in refused):
                outcome, why = "explicit_deny", "explicitly denied to CI: " + ", ".join(refused)
            else:
                outcome, why = "no_allow", "no statement grants CI " + ", ".join(refused)
        rows.append({"address": rc["address"], "actions": "+".join(actions), "on": label,
                     "outcome": outcome, "why": why})
    return rows


def command(addresses: list[str]) -> str:
    targets = " \\\n  ".join(f"-target={a}" for a in addresses)
    return "\n".join([
        f"cd {WORKDIR}",
        f"terraform version        # must print Terraform v{TERRAFORM_VERSION}: a newer binary upgrades the state",
        "terraform init -input=false",
        f"terraform apply -input=false \\\n  {targets}",
    ])


def report(rows: list[dict], source: str) -> tuple[str, list[str]]:
    blocked = [r for r in rows if r["outcome"] != "allow"]
    lines = ["## IAM changes the deploy cannot apply", ""]
    if not rows:
        lines.append("**Clean.** The deploy's targeted plan changes no IAM resource.")
        return "\n".join(lines) + "\n", []
    lines += [f"CI policy evaluated: `{CI_POLICY_ADDRESS}` ({source}).", "",
              "| Action | Resource | On | CI can apply it? |", "|---|---|---|---|"]
    for r in rows:
        verdict = "yes" if r["outcome"] == "allow" else f"**NO** - {r['why']}"
        lines.append(f"| `{r['actions']}` | `{r['address']}` | {r['on']} | {verdict} |")
    lines.append("")
    if not blocked:
        lines.append("**Clean.** Every IAM change in the deploy's reach is one CI is allowed to make.")
        return "\n".join(lines) + "\n", []
    lines += [
        "**Merging this as it stands breaks the deploy half-way**: the task definition and the service move,",
        "the IAM change is refused. Apply it by hand BEFORE merging, with a human's credentials, from a",
        f"checkout of this PR's head, on Terraform {TERRAFORM_VERSION}. Read the plan it prints before typing `yes`:",
        "", "```sh", command([r["address"] for r in blocked]), "```", "",
        "Then re-run this job: it goes green once the plan no longer carries the change.",
    ]
    return "\n".join(lines) + "\n", [r["address"] for r in blocked]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--targeted", required=True, help="terraform show -json of the targeted plan")
    ap.add_argument("--full", help="terraform show -json of the full plan (a fresher read of the CI policy)")
    ap.add_argument("--summary", help="file to append the markdown section to (GITHUB_STEP_SUMMARY)")
    a = ap.parse_args(argv)

    with open(a.targeted, encoding="utf-8") as f:
        targeted = json.load(f)
    plans = []
    if a.full:
        with open(a.full, encoding="utf-8") as f:
            plans.append(json.load(f))
    plans.append(targeted)

    document, account, source = ci_policy(plans)
    rows = verdicts(targeted, document, account)
    markdown, blocked = report(rows, source)
    if a.summary:
        with open(a.summary, "a", encoding="utf-8") as f:
            f.write(markdown)
    print(markdown)
    for r in rows:
        if r["outcome"] != "allow":
            print(f"::error title=IAM change CI cannot apply::{r['address']} ({r['actions']}, {r['on']}): "
                  f"{r['why']}. Apply it by hand before merging; the command is in the job summary.")
    return 1 if blocked else 0


if __name__ == "__main__":
    sys.exit(main())
