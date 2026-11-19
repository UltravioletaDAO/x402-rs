"""Every inline policy of the facilitator's task role is checked against its permissions boundary.

The task role carries a permissions boundary (terraform/environments/production/main.tf,
aws_iam_policy.ecs_task_boundary): whatever its inline policies grant, the running task
gets no more than that. A new inline policy that asks for an action or a resource the
boundary does not cover is cut to it in production -- an AccessDenied in the task, found
by a payment. tests/ecs_task_boundary.tftest.hcl checks each policy it lists against the
boundary; terraform test cannot enumerate resources by type, so this lists them from the
.tf files and fails when one is missing there.

Offline: the .tf files are read as text.

Run:  python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PRODUCTION = REPO / "terraform" / "environments" / "production"
BOUNDARY_TEST = PRODUCTION / "tests" / "ecs_task_boundary.tftest.hcl"

POLICY_BLOCK = re.compile(r'^resource\s+"aws_iam_role_policy"\s+"(?P<name>[A-Za-z0-9_-]+)"\s*\{', re.M)
# The task role, by reference or by the composed name the deploy uses.
TASK_ROLE = re.compile(r"^\s*role\s*=\s*(aws_iam_role\.ecs_task\.(id|name)|local\.ecs_task_role_name)\s*$", re.M)
ATTACHMENT = re.compile(r'^resource\s+"aws_iam_role_policy_attachment"\s+"[^"]+"\s*\{', re.M)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def blocks(text: str, start: re.Pattern):
    """(match, body) for each top-level block opened by `start`, up to its closing brace at column 0."""
    for m in start.finditer(text):
        end = text.find("\n}\n", m.end())
        yield m, text[m.end(): end if end != -1 else len(text)]


def task_role_policies() -> set[str]:
    found = set()
    for tf in sorted(PRODUCTION.glob("*.tf")):
        for m, body in blocks(read(tf), POLICY_BLOCK):
            if TASK_ROLE.search(body):
                found.add(f"aws_iam_role_policy.{m.group('name')}")
    return found


class EveryTaskRolePolicyIsCheckedAgainstTheBoundary(unittest.TestCase):
    def test_there_are_task_role_policies_to_check(self):
        self.assertGreaterEqual(len(task_role_policies()), 1, "no inline policy of the task role found")

    def test_each_is_in_the_coverage_assertion(self):
        listed = set(re.findall(r"aws_iam_role_policy\.([A-Za-z0-9_-]+)\.policy", read(BOUNDARY_TEST)))
        for address in sorted(task_role_policies()):
            with self.subTest(policy=address):
                self.assertIn(address.split(".", 1)[1], listed,
                              f"{address} is an inline policy of the task role and "
                              f"{BOUNDARY_TEST.name} does not check it against the boundary")

    def test_the_role_gets_no_managed_policy(self):
        # A managed policy attached to the task role is checked by nobody here.
        for tf in sorted(PRODUCTION.glob("*.tf")):
            for _m, body in blocks(read(tf), ATTACHMENT):
                with self.subTest(file=tf.name):
                    self.assertIsNone(TASK_ROLE.search(body),
                                      f"{tf.name} attaches a managed policy to the task role; "
                                      "check it against the boundary first")


if __name__ == "__main__":
    unittest.main()
