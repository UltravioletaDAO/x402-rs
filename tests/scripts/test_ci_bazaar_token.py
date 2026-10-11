"""The drift gate's "Bazaar admin token parameter is in place" step (owner's decision 230).

BAZAAR_ADMIN_TOKEN is an SSM SecureString on its own KMS key, and Terraform composes the parameter's ARN
instead of reading it, so no plan fails while the parameter is missing. This step does: it is run here as
the runner's bash runs it, with a stand-in `aws` that answers only describe-parameters (metadata) and
refuses anything else -- get-parameter would return the value. The task definition, the key and the
execution role's grants are held by terraform/environments/production/tests/bazaar_admin_token.tftest.hcl;
the CI user's deny by test_drift_gate_iam.py.
"""

import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

REPO = Path(__file__).resolve().parents[2]
STEP = "Bazaar admin token parameter is in place"
NAME = "/facilitator/bazaar-admin-token"
ALIAS = "alias/facilitator-bazaar-admin-token"
# A fake account, assembled so no line of this file is an account-bearing ARN (.github/workflows/no-account-id.yml).
ACCOUNT = "1111" + "22223333"
OTHER_KEY_ARN = "arn:aws:kms:us-east-2:" + ACCOUNT + ":key/1f1f1f1f-2e2e-3d3d-4c4c-5b5b5b5b5b5b"
TWELVE_DIGITS = re.compile(r"(?<![0-9])[0-9]{12}(?![0-9])")
EXPECTED_CALL = (f"ssm describe-parameters --region us-east-2 --parameter-filters "
                 f"Key=Name,Option=Equals,Values={NAME} --query Parameters[0].[Type,KeyId] --output text")


def the_step() -> dict:
    ci = yaml.safe_load((REPO / ".github" / "workflows" / "ci.yaml").read_text(encoding="utf-8"))
    found = [s for s in ci["jobs"]["plan"]["steps"] if s.get("name") == STEP]
    if len(found) != 1:
        raise AssertionError(f"expected exactly one drift-gate step named {STEP!r}, found {len(found)}")
    return found[0]


class TheDriftGateChecksTheParameter(unittest.TestCase):
    def run_step(self, answer: str, code: int = 0) -> subprocess.CompletedProcess:
        if os.name != "posix" or shutil.which("bash") is None:
            self.skipTest("needs a POSIX bash, as the runner has")
        with tempfile.TemporaryDirectory() as tmp:
            aws = Path(tmp) / "aws"
            aws.write_text(
                "#!/bin/sh\n"
                f'[ "$*" = "{EXPECTED_CALL}" ] || {{ echo "unexpected aws call: $*" >&2; exit 97; }}\n'
                f"printf '%s\\n' '{answer}'\n"
                f"exit {code}\n",
                encoding="utf-8",
            )
            aws.chmod(0o755)
            env = {"PATH": f"{tmp}{os.pathsep}{os.environ.get('PATH', '')}", "AWS_REGION": "us-east-2"}
            return subprocess.run(
                ["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", str(the_step()["run"])],
                env=env, capture_output=True, text=True, check=False)

    def test_the_step_runs_whatever_happened_before_it(self):
        self.assertEqual(str(the_step().get("if")), "!cancelled()")

    def test_on_the_dedicated_key_it_is_green(self):
        for key in (ALIAS, "arn:aws:kms:us-east-2:" + ACCOUNT + ":" + ALIAS):
            with self.subTest(key_id=key[:24]):
                done = self.run_step(f"SecureString\t{key}")
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertNotIn("::error", done.stdout)
                self.assertIsNone(TWELVE_DIGITS.search(done.stdout + done.stderr))

    def test_a_missing_parameter_is_red(self):
        done = self.run_step("None")
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("::error title=Bazaar admin token parameter missing::", done.stdout)

    def test_a_parameter_that_is_not_a_secure_string_is_red(self):
        done = self.run_step(f"String\t{ALIAS}")
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("::error title=Bazaar admin token parameter is not secret::", done.stdout)

    def test_another_key_is_red_and_never_named(self):
        for key in ("alias/aws/ssm", OTHER_KEY_ARN, "alias/facilitator-bazaar-admin-token-old"):
            with self.subTest(key_id=key[:24]):
                done = self.run_step(f"SecureString\t{key}")
                self.assertNotEqual(done.returncode, 0, done.stdout)
                self.assertIn("::error title=Bazaar admin token on the wrong key::", done.stdout)
                self.assertNotIn(key, done.stdout, "the logs are public: the other key is not printed")

    def test_a_read_that_did_not_complete_warns_without_an_account_id(self):
        denied = ("An error occurred (AccessDeniedException): User: arn:aws:iam::" + ACCOUNT
                  + ":user/ci is not authorized to perform: ssm:DescribeParameters")
        done = self.run_step(denied, code=254)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("::warning title=Bazaar admin token check inconclusive::", done.stdout)
        self.assertIsNone(TWELVE_DIGITS.search(done.stdout), done.stdout)


TF = REPO / "terraform" / "environments" / "production"


def code_of_block(path: Path, header: str) -> list[str]:
    """The lines of the HCL block that opens with `header`, comments dropped."""
    lines = path.read_text(encoding="utf-8").splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith(header))
    depth, block = 0, []
    for line in lines[start:]:
        depth += line.count("{") - line.count("}")
        if not line.strip().startswith("#"):
            block.append(line)
        if depth == 0:
            break
    return block


class TheDeployNeverDependsOnTheKey(unittest.TestCase):
    """The image deploy applies -target=aws_iam_role_policy.secrets_access and the task definition. A
    reference to the KMS key in either would drag the key into every deploy as a dependency the CI user can
    neither create nor update -- the way aws_ecs_cluster.main broke #115's deploy."""

    def test_both_are_deploy_targets(self):
        ci = (REPO / ".github" / "workflows" / "ci.yaml").read_text(encoding="utf-8")
        for target in ("aws_iam_role_policy.secrets_access", "aws_ecs_task_definition.facilitator"):
            self.assertIn(f"-target={target}", ci)

    def test_neither_references_the_key(self):
        for header in ('resource "aws_iam_role_policy" "secrets_access" {',
                       'resource "aws_ecs_task_definition" "facilitator" {'):
            with self.subTest(block=header):
                code = "\n".join(code_of_block(TF / "main.tf", header))
                self.assertNotRegex(code, r"aws_kms_(key|alias)\.")

    def test_the_locals_they_read_are_plain_values(self):
        secrets = (TF / "secrets.tf").read_text(encoding="utf-8")
        for name in ("bazaar_admin_token_parameter_arn", "bazaar_admin_token_kms_alias",
                     "bazaar_admin_token_parameter"):
            with self.subTest(local=name):
                line = re.search(rf"^\s*{name}\s*=\s*(.+)$", secrets, re.MULTILINE)
                self.assertIsNotNone(line, f"local.{name} is gone")
                self.assertNotRegex(line.group(1), r"aws_kms_(key|alias)\.")

    def test_nothing_reads_the_parameter_into_the_state(self):
        # A data or resource "aws_ssm_parameter" would store the value (or its ciphertext) in the state.
        for path in TF.glob("*.tf"):
            with self.subTest(file=path.name):
                text = path.read_text(encoding="utf-8")
                self.assertNotRegex(text, r'(?m)^\s*(data|resource)\s+"aws_ssm_parameter"')
                self.assertNotIn('"aws_secretsmanager_secret" "bazaar_admin_token"', text)


if __name__ == "__main__":
    unittest.main()
