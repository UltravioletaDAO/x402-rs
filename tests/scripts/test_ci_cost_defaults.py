"""What the CI deploy applies on its own defaults, for the COSTO-X402 cost cuts.

terraform.tfvars is gitignored, so the deploy job applies on the defaults in
variables.tf. A default that disagrees with production.auto.tfvars undoes the
operator's apply on the next release (ci.yaml, "Terraform apply"). Each check skips
a variable that is not declared, so reverting one cut's commit does not turn this red.
"""
import re
import sys
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

    def test_the_nat_is_off_in_its_own_change(self):
        # REF-X402-115 P2-1 kept the NAT on in the change that moved the tasks: the service
        # precondition reads variables, not where the tasks run, and no graph edge orders the
        # NAT destroy after the rolling deployment. X402-NAT-OFF turned it off on its own,
        # after both tasks ran a day in the public subnets. Turning it back on is the hand
        # rollback in docs/handoffs/COSTO-X402-recorte-aws.md, whose branch flips this.
        d, t = defaults(), tfvars()
        if "enable_nat_gateway" not in d:
            self.skipTest("B5 not in this tree")
        self.assertEqual(d["enable_nat_gateway"], "false", "variables.tf default")
        self.assertEqual(t.get("enable_nat_gateway"), "false", "production.auto.tfvars")

    def test_whatever_runs_in_a_private_subnet_is_guarded_by_the_nat(self):
        # X402-NAT-OFF: without the NAT nothing in a private subnet has a route out. G2 (the
        # postcondition on data.aws_network_interfaces.private_subnets) stops the plan that
        # would strand an ENI that already exists there; this stops a task or a Lambda from
        # being DECLARED there without a precondition on var.enable_nat_gateway. Only what
        # never opens a connection to the internet is exempt. A local that hands the
        # subnets on under another name gets past this; G2 still sees what it creates.
        no_egress = {
            "aws_route_table_association.private",
            "aws_vpc_endpoint.secretsmanager",
            "aws_efs_mount_target.observability",
        }
        text = "\n".join(p.read_text(encoding="utf-8") for p in sorted(TF.glob("*.tf")))
        users = set()
        for m in re.finditer(r'^(resource|data) "([a-z0-9_]+)" "([a-z0-9_]+)" \{(.*?)^\}', text, re.S | re.M):
            if "aws_subnet.private" not in m.group(4):
                continue
            address = ("data." if m.group(1) == "data" else "") + f"{m.group(2)}.{m.group(3)}"
            users.add(address)
            if address not in no_egress:
                self.assertRegex(m.group(4), r"precondition \{\s*condition\s*=\s*var\.enable_nat_gateway\b",
                                 f"{address} runs in a private subnet without a NAT guard")
        self.assertTrue(no_egress <= users, f"stale exemptions: {sorted(no_egress - users)}")
        self.assertIn("aws_ecs_service.facilitator", users)

    def test_the_live_nat_guards_read_the_right_filters(self):
        # The mocks in tests/nat_guard.tftest.hcl override what these data sources return,
        # so they cannot see the lookups. The route tables are found by the Name tag the
        # managed tables carry; the endpoint ENIs are the only exempt kind; without
        # `state = available` a NAT that AWS still lists as `deleted` (it does for a while)
        # would let the tasks move with no egress.
        main = (TF / "main.tf").read_text(encoding="utf-8")
        name = re.search(r"^\s*(Name\s*=\s*\"[^\"]*private-rt-[^\"]*\")\n",
                         block(main, 'resource "aws_route_table" "private"'), re.M).group(1)
        for header, mode in (('data "aws_route_table" "private_without_nat"', "var.enable_nat_gateway"),
                             ('data "aws_route_table" "private_for_tasks"', "var.ecs_tasks_in_public_subnets")):
            rt = block(main, header)
            self.assertRegex(rt, rf"count\s*=\s*{re.escape(mode)} \? 0 : local\.nat_count\n", header)
            self.assertIn(name, rt, header)
        vpc_name = re.search(r"^\s*(Name\s*=\s*\"[^\"]*\")\n", block(main, 'resource "aws_vpc" "main"'), re.M).group(1)
        vpc = block(main, 'data "aws_vpc" "without_nat"')
        self.assertRegex(vpc, r"count\s*=\s*var\.enable_nat_gateway \? 0 : 1\n")
        self.assertIn(vpc_name, vpc)
        self.assertRegex(block(main, 'data "aws_route_table" "private_without_nat"'),
                         r"vpc_id\s*=\s*data\.aws_vpc\.without_nat\[0\]\.id\n")
        # Subnet associations carry gateway_id = "" in AWS: reading that field instead would
        # leave G2 with no subnets and nothing to check.
        for local in ("subnets_without_nat", "private_subnets_with_nat_egress"):
            body = re.search(rf"^  {local}\s*=(.*?)^  \S", main + "\n  x", re.S | re.M).group(1)
            self.assertIn("rt.associations[*].subnet_id", body, local)
        for header in ('data "aws_network_interfaces" "private_subnets"',
                       'data "aws_network_interfaces" "private_subnet_endpoints"'):
            enis = block(main, header)
            self.assertRegex(enis, r"count\s*=\s*length\(local\.subnets_without_nat\) > 0 \? 1 : 0\n", header)
            self.assertRegex(enis, r'name\s*=\s*"subnet-id"\n\s*values\s*=\s*local\.subnets_without_nat\n', header)
        endpoints = block(main, 'data "aws_network_interfaces" "private_subnet_endpoints"')
        self.assertRegex(endpoints, r'name\s*=\s*"interface-type"\n\s*values\s*=\s*\["vpc_endpoint"\]\n')
        nats = block(main, 'data "aws_nat_gateways" "available"')
        self.assertRegex(nats, r"count\s*=\s*var\.ecs_tasks_in_public_subnets \? 0 : 1\n")
        self.assertRegex(nats, r"vpc_id\s*=\s*data\.aws_route_table\.private_for_tasks\[0\]\.vpc_id\n")
        self.assertRegex(nats, r'name\s*=\s*"state"\n\s*values\s*=\s*\["available"\]\n')

    def test_the_live_nat_guards_reference_nothing_managed(self):
        # VER of X402-NAT-OFF, P2-1: a data source that references a managed resource with a
        # pending change is read during apply, and in the apply that drops the NAT Terraform
        # destroys the NAT before it updates the route table. A guard read then fires after
        # the damage. From variables and other data sources only, they are read at plan.
        main = (TF / "main.tf").read_text(encoding="utf-8")
        guards = [block(main, h) for h in ('data "aws_vpc" "without_nat"',
                                           'data "aws_route_table" "private_without_nat"',
                                           'data "aws_network_interfaces" "private_subnet_endpoints"',
                                           'data "aws_network_interfaces" "private_subnets"',
                                           'data "aws_route_table" "private_for_tasks"',
                                           'data "aws_nat_gateways" "available"')]
        guard_locals = ("subnets_without_nat", "private_subnet_endpoint_enis",
                        "available_nat_gateways", "private_subnets_with_nat_egress")
        for body in re.findall(r"^locals \{(.*?)^\}", main, re.S | re.M):
            if any(re.search(rf"^  {n}\s*=", body, re.M) for n in guard_locals):
                guards.append(body)
        self.assertGreaterEqual(len(guards), 8)
        for body in guards:
            self.assertEqual(re.findall(r"(?<![.\w])aws_[a-z0-9_]+\.[a-z0-9_]+", body), [], body[:80])
        for header in ('resource "aws_eip" "nat"', 'resource "aws_nat_gateway" "main"'):
            self.assertRegex(block(main, header),
                             r"depends_on\s*=\s*\[[^\]]*data\.aws_network_interfaces\.private_subnets\]", header)

    def test_the_nat_guards_run_in_ci(self):
        # The guards read AWS, so only terraform test exercises them before a hand apply.
        sys.path.insert(0, str(TF.parents[2] / "scripts"))
        import ci_paths_selftest
        steps = ci_paths_selftest.load_workflows()["ci.yaml"]["jobs"]["test"]["steps"]
        names = [s.get("name") for s in steps]
        guard = steps[names.index("Terraform NAT guards")]
        self.assertEqual(guard.get("working-directory"), "terraform/environments/production")
        self.assertEqual(guard["run"].split(), "terraform init -backend=false -input=false terraform test".split())
        setup = [s for s in steps[:names.index("Terraform NAT guards")]
                 if str(s.get("uses", "")).startswith("hashicorp/setup-terraform@")]
        self.assertEqual(len(setup), 1)
        self.assertEqual(setup[0]["with"]["terraform_version"], "1.9.8")
        tftest = (TF / "tests" / "nat_guard.tftest.hcl").read_text(encoding="utf-8")
        for target in ("data.aws_network_interfaces.private_subnets", "aws_ecs_service.facilitator"):
            self.assertIn(f"expect_failures = [{target}]", tftest)

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
