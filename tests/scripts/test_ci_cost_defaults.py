"""What the CI deploy applies on its own defaults, for the COSTO-X402 cost cuts.

terraform.tfvars is gitignored, so the deploy job applies on the defaults in
variables.tf. A default that disagrees with production.auto.tfvars undoes the
operator's apply on the next release (ci.yaml, "Terraform apply"). Each check skips
a variable that is not declared, so reverting one cut's commit does not turn this red.
"""
import re
import unittest
from pathlib import Path

TF = Path(__file__).resolve().parents[2] / "terraform/environments/production"
COST_VARIABLES = (
    "task_cpu",
    "task_memory",
    "enable_nat_gateway",
    "ecs_tasks_in_public_subnets",
    "enable_container_insights",
    "enable_facilitator_ecr_lifecycle",
)
# Fargate's valid memory (MiB) for each CPU size.
FARGATE_MEMORY = {
    256: {512, 1024, 2048},
    512: set(range(1024, 4097, 1024)),
    1024: set(range(2048, 8193, 1024)),
    2048: set(range(4096, 16385, 1024)),
    4096: set(range(8192, 30721, 1024)),
}


def defaults() -> dict[str, str]:
    text = "\n".join(p.read_text(encoding="utf-8") for p in sorted(TF.glob("*.tf")))
    found = {}
    for block in re.finditer(r'^variable "([a-z0-9_]+)" \{(.*?)^\}', text, re.S | re.M):
        value = re.search(r"^\s*default\s*=\s*(\S+)\s*$", block.group(2), re.M)
        if value:
            found[block.group(1)] = value.group(1)
    return found


def tfvars() -> dict[str, str]:
    found = {}
    for line in (TF / "production.auto.tfvars").read_text(encoding="utf-8").splitlines():
        match = re.match(r"^([a-z0-9_]+)\s*=\s*([^\s#]+)", line)
        if match:
            found[match.group(1)] = match.group(2)
    return found


class CostDefaultsTest(unittest.TestCase):
    def test_defaults_match_tfvars(self):
        d, t = defaults(), tfvars()
        for name in COST_VARIABLES:
            if name in d or name in t:
                self.assertEqual(d.get(name), t.get(name), name)

    def test_task_size_is_a_fargate_pair(self):
        d = defaults()
        cpu, memory = int(d["task_cpu"]), int(d["task_memory"])
        self.assertIn(memory, FARGATE_MEMORY[cpu])

    def test_no_nat_means_public_subnets(self):
        d = defaults()
        if d.get("enable_nat_gateway") == "false":
            self.assertEqual(d.get("ecs_tasks_in_public_subnets"), "true")

    def test_no_alarm_reads_container_insights_while_it_is_off(self):
        if defaults()["enable_container_insights"] != "false":
            self.skipTest("Container Insights is on")
        for path in sorted(TF.glob("*.tf")):
            self.assertNotIn('"ECS/ContainerInsights"', path.read_text(encoding="utf-8"), path.name)


if __name__ == "__main__":
    unittest.main()
