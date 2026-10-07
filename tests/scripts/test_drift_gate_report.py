#!/usr/bin/env python3
"""Runs the drift gate's two bash steps out of .github/workflows/ci.yaml against fixture plans.

The `Report changes the deploy will never apply` step died on #115 at the first
resource deleted from the configuration: under the runner's `bash -e` plus
pipefail, the grep for the declaring .tf exited 1 and took the step with it, with
no annotation, no list and half a summary. These tests execute the step text
exactly as the workflow file holds it, with `bash -e` like the runner
(`shell: /usr/bin/bash -e {0}`), a stand-in `terraform` that prints fixture
plans, and the real jq/grep/comm. No AWS and no Terraform.

    python3 -m unittest discover -s tests/scripts -p 'test_drift_gate_*.py'
"""

import json
import os
import shutil
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CI_YAML = REPO / ".github" / "workflows" / "ci.yaml"
REPORT_STEP = "Report changes the deploy will never apply"
PLAN_STEP = "Plan"

FAKE_TERRAFORM = """#!/usr/bin/env bash
case "$1" in
  init) exit 0 ;;
  show) cat "${3%.tfplan}.plan.json" ;;
  plan)
    out=""
    for a in "$@"; do case "$a" in -out=*) out="${a#-out=}" ;; esac; done
    : > "$out"
    if [ "$out" = "full.tfplan" ]; then
      echo "Error: fake full plan output"
      exit "${FAKE_FULL_PLAN_EXIT:-0}"
    fi
    exit 0 ;;
esac
"""

DECLARED_TF = """resource "aws_vpc_endpoint" "s3" {
}

resource "aws_nat_gateway" "main" {
  count = 0
}

resource "aws_ecs_service" "facilitator" {
}

resource "aws_ecr_repository" "kept" {
}
"""


def step_script(name: str) -> str:
    """The `run: |` body of the plan job's step called `name`, read as text (no PyYAML on that CI step)."""
    lines = CI_YAML.read_text(encoding="utf-8").split("\n")
    start = next(i for i, line in enumerate(lines) if line.strip() == "- name: " + name)
    run_at = next(i for i in range(start + 1, len(lines)) if lines[i].strip() == "run: |")
    run_indent = len(lines[run_at]) - len(lines[run_at].lstrip())
    body = []
    for line in lines[run_at + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= run_indent:
            break
        body.append(line)
    while body and not body[-1].strip():
        body.pop()
    return textwrap.dedent("\n".join(body)) + "\n"


def targets() -> set:
    import re
    return set(re.findall(r"[-]target=([A-Za-z0-9_.]+)", CI_YAML.read_text(encoding="utf-8")))


def change(address: str, actions, before=None, after=None) -> dict:
    rtype, name = address.split("[")[0].split(".", 1)
    return {"address": address, "type": rtype, "name": name,
            "change": {"actions": list(actions), "before": before, "after": after}}


def deleted(address: str) -> dict:
    return change(address, ["delete"], before={"name": address, "tags": {}}, after=None)


def updated(address: str, attr: str = "route_table_ids") -> dict:
    return change(address, ["update"], before={"id": "x", attr: ["a"]}, after={"id": "x", attr: ["a", "b"]})


class StepHarness(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        missing = [t for t in ("bash", "jq", "comm", "grep", "sort") if shutil.which(t) is None]
        if missing:
            msg = "drift gate step tests need " + ", ".join(missing)
            if os.environ.get("GITHUB_ACTIONS"):
                raise AssertionError(msg)
            raise unittest.SkipTest(msg)

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.work = self.tmp / "production"
        self.work.mkdir()
        (self.work / "main.tf").write_text(DECLARED_TF, encoding="utf-8")
        bindir = self.tmp / "bin"
        bindir.mkdir()
        fake = bindir / "terraform"
        fake.write_text(FAKE_TERRAFORM, encoding="utf-8")
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        self.bindir = bindir
        self.summary = self.tmp / "summary.md"
        self.summary.write_text("", encoding="utf-8")

    def run_step(self, name: str, full=(), targeted=(), extra_env=None):
        (self.work / "full.plan.json").write_text(json.dumps({"resource_changes": list(full)}), encoding="utf-8")
        (self.work / "targeted.plan.json").write_text(json.dumps({"resource_changes": list(targeted)}), encoding="utf-8")
        (self.work / "full.tfplan").write_text("", encoding="utf-8")
        (self.work / "targeted.tfplan").write_text("", encoding="utf-8")
        (self.work / "full.log").write_text("Plan: fixture full plan log\n", encoding="utf-8")
        script = self.tmp / "step.sh"
        script.write_text(step_script(name), encoding="utf-8")
        env = dict(os.environ)
        env.update({"PATH": str(self.bindir) + os.pathsep + env.get("PATH", ""),
                    "GITHUB_WORKSPACE": str(REPO), "GITHUB_STEP_SUMMARY": str(self.summary)})
        env.update(extra_env or {})
        proc = subprocess.run(["bash", "-e", str(script)], cwd=self.work, env=env,
                              capture_output=True, text=True, timeout=120)
        return proc, self.summary.read_text(encoding="utf-8")


class ReportStep(StepHarness):
    def test_fixture_addresses_are_not_deploy_targets(self):
        for addr in ("aws_cloudwatch_dashboard.near_operations", "aws_vpc_endpoint.s3",
                     "aws_nat_gateway.main", "aws_ecr_repository.kept", "aws_ecr_repository.grafana"):
            self.assertNotIn(addr, targets())
        self.assertIn("aws_ecs_service.facilitator", targets())

    def test_a_destroyed_resource_is_a_row_not_a_crash(self):
        proc, summary = self.run_step(
            REPORT_STEP,
            full=[deleted("aws_cloudwatch_dashboard.near_operations"),
                  updated("aws_vpc_endpoint.s3"),
                  updated("aws_ecs_service.facilitator", "network_configuration")],
            targeted=[updated("aws_ecs_service.facilitator", "network_configuration")])
        out = proc.stdout
        self.assertEqual(proc.returncode, 1, out + proc.stderr)
        self.assertIn("::error title=Unapplied infrastructure::aws_cloudwatch_dashboard.near_operations (delete)", out)
        self.assertIn("::error title=Unapplied infrastructure::aws_vpc_endpoint.s3 (update)", out)
        listed = out.split("Drift the pipeline will never apply:\n", 1)[1].split("\n")
        self.assertEqual(listed[:2], ["delete aws_cloudwatch_dashboard.near_operations", "update aws_vpc_endpoint.s3"])
        self.assertIn("| `delete` | `aws_cloudwatch_dashboard.near_operations` | (removed from the configuration) | (whole resource) |",
                      summary)
        # The row AFTER the destroyed one is written too, and the table runs to its end.
        self.assertIn("| `update` | `aws_vpc_endpoint.s3` | main.tf | route_table_ids |", summary)
        self.assertIn("| `update` | `aws_ecs_service.facilitator` | main.tf | network_configuration |", summary)
        self.assertIn("<summary>Full plan output</summary>", summary)
        self.assertIn("Plan: fixture full plan log", summary)
        self.assertIn("</details>", summary)

    def test_only_destroys_still_go_red(self):
        proc, summary = self.run_step(
            REPORT_STEP,
            full=[deleted("aws_ecr_repository.grafana"), deleted("aws_ecr_repository.prometheus")])
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertNotIn("No unapplied infrastructure.", proc.stdout)
        self.assertIn("delete aws_ecr_repository.grafana\ndelete aws_ecr_repository.prometheus", proc.stdout)
        self.assertEqual(summary.count("(removed from the configuration)"), 2)

    def test_a_destroy_inside_the_deploys_reach_is_a_pending_row(self):
        gone = deleted("aws_cloudwatch_metric_alarm.gone")
        proc, summary = self.run_step(REPORT_STEP, full=[gone], targeted=[gone])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("No unapplied infrastructure.", proc.stdout)
        self.assertIn("| `delete` | `aws_cloudwatch_metric_alarm.gone` | (removed from the configuration) | (whole resource) |",
                      summary)
        self.assertIn("**Clean.**", summary)

    def test_a_count_zero_destroy_names_the_file_that_still_declares_it(self):
        proc, summary = self.run_step(REPORT_STEP, full=[deleted("aws_nat_gateway.main[0]")])
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn("| `delete` | `aws_nat_gateway.main[0]` | main.tf | (whole resource) |", summary)

    def test_a_replace_lists_its_attributes(self):
        replace = change("aws_ecr_repository.kept", ["delete", "create"],
                         before={"name": "kept", "image_tag_mutability": "MUTABLE"},
                         after={"name": "kept", "image_tag_mutability": "IMMUTABLE"})
        proc, summary = self.run_step(REPORT_STEP, full=[replace])
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn("| `delete+create` | `aws_ecr_repository.kept` | main.tf | image_tag_mutability |", summary)

    def test_an_undeclared_change_that_is_not_a_destroy_says_unknown(self):
        proc, summary = self.run_step(REPORT_STEP, full=[updated("aws_vpc_endpoint.nowhere")])
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn("| `update` | `aws_vpc_endpoint.nowhere` | ? | route_table_ids |", summary)

    def test_a_clean_plan_is_green(self):
        noop = change("aws_vpc_endpoint.s3", ["no-op"], before={"id": "x"}, after={"id": "x"})
        read = change("data.aws_caller_identity.current", ["read"])
        proc, summary = self.run_step(REPORT_STEP, full=[noop, read])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("No unapplied infrastructure.", proc.stdout)
        self.assertNotIn("::error", proc.stdout)
        self.assertIn("**Clean.**", summary)


class PlanStep(StepHarness):
    def test_a_full_plan_with_changes_does_not_end_the_step(self):
        proc, _ = self.run_step(PLAN_STEP, extra_env={"FAKE_FULL_PLAN_EXIT": "2"})
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("Full plan exit code: 2", proc.stdout)

    def test_a_broken_full_plan_prints_its_log(self):
        proc, _ = self.run_step(PLAN_STEP, extra_env={"FAKE_FULL_PLAN_EXIT": "1"})
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn("::error title=Drift gate failed::terraform plan errored", proc.stdout)
        self.assertIn("Error: fake full plan output", proc.stdout)

    def test_an_empty_full_plan_reports_zero(self):
        proc, _ = self.run_step(PLAN_STEP, extra_env={"FAKE_FULL_PLAN_EXIT": "0"})
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("Full plan exit code: 0", proc.stdout)


class Extraction(unittest.TestCase):
    def test_the_text_extraction_is_what_yaml_reads(self):
        try:
            import yaml
        except ImportError:
            raise unittest.SkipTest("PyYAML not installed")
        steps = yaml.safe_load(CI_YAML.read_text(encoding="utf-8"))["jobs"]["plan"]["steps"]
        by_name = {s.get("name"): s for s in steps}
        for name in (REPORT_STEP, PLAN_STEP):
            self.assertEqual(step_script(name).rstrip("\n"), by_name[name]["run"].rstrip("\n"))
            self.assertEqual(by_name[name].get("shell"), None, "the tests assume the default bash -e")


if __name__ == "__main__":
    unittest.main()
