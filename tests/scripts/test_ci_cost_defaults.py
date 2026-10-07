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

    def test_tasks_in_public_subnets_keep_the_nat_guard(self):
        if "ecs_tasks_in_public_subnets" not in defaults():
            self.skipTest("B5 not in this tree")
        main = (TF / "main.tf").read_text(encoding="utf-8")
        service = block(main, 'resource "aws_ecs_service" "facilitator"')
        self.assertIn("var.ecs_tasks_in_public_subnets ? aws_subnet.public[*].id : aws_subnet.private[*].id", service)
        self.assertRegex(service, r"assign_public_ip\s*=\s*var\.ecs_tasks_in_public_subnets\n")
        self.assertRegex(service, r"condition\s*=\s*var\.enable_nat_gateway \|\| var\.ecs_tasks_in_public_subnets\n")
        self.assertRegex(main, r"nat_gateway_count\s*=\s*var\.enable_nat_gateway \? local\.nat_count : 0\n")
        for header in ('resource "aws_eip" "nat"', 'resource "aws_nat_gateway" "main"'):
            self.assertRegex(block(main, header), r"count\s*=\s*local\.nat_gateway_count\n", header)
        for name in ("dynamodb", "s3"):
            self.assertIn("aws_route_table.public.id", block(main, f'resource "aws_vpc_endpoint" "{name}"'), name)

    def test_nat_stays_on_in_the_change_that_moves_the_tasks(self):
        # REF-X402-115 P2-1: the service precondition reads variables, not where the tasks
        # run, and no graph edge orders the NAT destroy after the rolling deployment. The
        # NAT goes off in its own change, after every task is verified in a public subnet;
        # that change flips this assertion together with the two values.
        d, t = defaults(), tfvars()
        if "enable_nat_gateway" not in d:
            self.skipTest("B5 not in this tree")
        self.assertEqual(d["enable_nat_gateway"], "true", "variables.tf default")
        self.assertEqual(t.get("enable_nat_gateway"), "true", "production.auto.tfvars")

    def test_writer_lease_egress_is_by_security_group(self):
        # The tasks now sit in the public subnets: a CIDR of the private subnets here would
        # drop the forward between peers on the way out.
        sg = block((TF / "main.tf").read_text(encoding="utf-8"), 'resource "aws_security_group" "ecs_tasks"')
        egress = [r for r in re.findall(r"^  egress \{(.*?)^  \}", sg, re.S | re.M)
                  if re.search(r"from_port\s*=\s*8080\n", r)]
        self.assertEqual(len(egress), 1)
        self.assertRegex(egress[0], r"to_port\s*=\s*8080\n")
        self.assertRegex(egress[0], r'protocol\s*=\s*"tcp"\n')
        self.assertRegex(egress[0], r"self\s*=\s*true\n")
        self.assertNotIn("cidr_blocks", egress[0])

    def test_secrets_manager_endpoint_resolves_privately(self):
        # Private DNS is what sends the tasks' secretsmanager calls to the endpoint ENI from
        # any subnet; the endpoint SG admits by the tasks' SG, not by a private-subnet CIDR.
        main = (TF / "main.tf").read_text(encoding="utf-8")
        endpoint = block(main, 'resource "aws_vpc_endpoint" "secretsmanager"')
        self.assertRegex(endpoint, r"private_dns_enabled\s*=\s*true\n")
        self.assertRegex(endpoint, r"security_group_ids\s*=\s*\[aws_security_group\.vpc_endpoints\.id\]\n")
        sg = block(main, 'resource "aws_security_group" "vpc_endpoints"')
        ingress = re.findall(r"^  ingress \{(.*?)^  \}", sg, re.S | re.M)
        self.assertEqual(len(ingress), 1)
        self.assertRegex(ingress[0], r"security_groups\s*=\s*\[aws_security_group\.ecs_tasks\.id\]\n")
        self.assertNotIn("cidr_blocks", ingress[0])

    def test_tasks_take_no_inbound_from_the_internet(self):
        main = (TF / "main.tf").read_text(encoding="utf-8")
        sg = block(main, 'resource "aws_security_group" "ecs_tasks"')
        ingress = re.findall(r"^  ingress \{(.*?)^  \}", sg, re.S | re.M)
        self.assertEqual(len(ingress), 2)
        for rule in ingress:
            self.assertNotIn("cidr_blocks", rule)
            self.assertNotIn("ipv6_cidr_blocks", rule)
            self.assertRegex(rule, r"from_port\s*=\s*8080\n")

    def test_no_running_tasks_alarm_reads_the_alb(self):
        if defaults()["enable_container_insights"] != "false":
            self.skipTest("Container Insights is on")
        alarm = block((TF / "alerts-imported.tf").read_text(encoding="utf-8"),
                      'resource "aws_cloudwatch_metric_alarm" "orphan_no_running_tasks"')
        for pattern in (r'metric_name\s*=\s*"HealthyHostCount"', r'namespace\s*=\s*"AWS/ApplicationELB"',
                        r"LoadBalancer\s*=\s*aws_lb\.main\.arn_suffix", r"TargetGroup\s*=\s*aws_lb_target_group\.main\.arn_suffix",
                        r'comparison_operator\s*=\s*"LessThanThreshold"', r"threshold\s*=\s*1\n",
                        r'treat_missing_data\s*=\s*"breaching"', r'statistic\s*=\s*"Minimum"'):
            self.assertRegex(alarm, pattern)


def block(text: str, header: str) -> str:
    start = text.index(header + " {")
    return text[start:text.index("\n}\n", start)]


if __name__ == "__main__":
    unittest.main()
