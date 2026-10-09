# X402-NAT-OFF: the guards around the NAT gateway, against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards"). Nothing here reaches AWS.
#
# The guards' data sources are built from variables only (main.tf says why), so they are
# read at plan even on an empty state, as in production: the runs that expect a guard to
# fire are plans. The runs share one mocked state, in order.

mock_provider "aws" {
  # The HTTPS listener validates certificate_arn as an ARN, and a mock makes up a random
  # string. No account id in it: this repo is public (.github/workflows/no-account-id.yml).
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-2::certificate/mock"
    }
  }
}
mock_provider "archive" {}

# Fixed ids for the two private subnets, so the route tables below can name them.
override_resource {
  target = aws_subnet.private[0]
  values = {
    id = "subnet-0a0a0a0a0a0a0a0a1"
  }
}

override_resource {
  target = aws_subnet.private[1]
  values = {
    id = "subnet-0b0b0b0b0b0b0b0b2"
  }
}

# G2's view of AWS: the private table and the two subnets associated with it.
override_data {
  target = data.aws_route_table.private_without_nat
  values = {
    associations = [
      { subnet_id = "subnet-0a0a0a0a0a0a0a0a1" },
      { subnet_id = "subnet-0b0b0b0b0b0b0b0b2" },
    ]
  }
}

# The Secrets Manager endpoint's ENI: a VPC endpoint, the one kind allowed to stay.
override_data {
  target = data.aws_network_interfaces.private_subnet_endpoints
  values = {
    ids = ["eni-0e0d0e0d0e0d0e0d1"]
  }
}

# Setup, first, so a red run further down still tears down cleanly and its own error is
# the one that shows:
# - the certificate validation records take their for_each keys from the certificates:
#   aws_acm_certificate.main has to be in state before the service (one of its
#   dependencies) can be planned, and both before Terraform 1.9.8 can plan the teardown,
#   which walks the whole configuration;
# - the private subnets, as in production: G3 compares their ids with the route tables',
#   and a subnet that does not exist yet has no id at plan.
run "the_certificates_and_the_private_subnets_exist" {
  command = apply

  plan_options {
    target = [aws_acm_certificate.main, aws_acm_certificate.metrics, aws_subnet.private]
  }
}

run "the_shipped_values_drop_the_nat_and_its_route" {
  # production.auto.tfvars as committed: enable_nat_gateway = false. Only the endpoint is
  # in the private subnets.
  command = apply

  plan_options {
    target = [aws_route_table.private]
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0e0d0e0d0e0d0e0d1"]
    }
  }

  assert {
    condition     = var.enable_nat_gateway == false
    error_message = "This run is meant to exercise the committed tfvars (enable_nat_gateway = false)."
  }

  assert {
    condition     = length(aws_nat_gateway.main) == 0 && length(aws_eip.nat) == 0
    error_message = "With the NAT off there is no NAT gateway and no EIP."
  }

  assert {
    condition     = length(aws_route_table.private) == 1
    error_message = "The private route table stays: the gateway endpoints and the Secrets Manager endpoint's subnet use it."
  }

  assert {
    condition     = length(aws_route_table.private[0].route) == 0
    error_message = "With the NAT off the private table must carry no route: zero route blocks would leave the old 0.0.0.0/0 entry behind as a blackhole."
  }

  assert {
    condition     = length(data.aws_network_interfaces.private_subnets) == 1
    error_message = "With the NAT off, G2 reads the ENIs of the subnets associated with the private table."
  }
}

run "a_task_left_in_a_private_subnet_stops_the_plan" {
  command = plan

  plan_options {
    target = [aws_route_table.private]
  }

  variables {
    enable_nat_gateway = false
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0e0d0e0d0e0d0e0d1", "eni-0a5c0a5c0a5c0a5c2"]
    }
  }

  expect_failures = [data.aws_network_interfaces.private_subnets]
}

run "a_lambda_left_in_a_private_subnet_stops_the_plan" {
  # G2 does not ask what an ENI is, only whether it is a VPC endpoint's: a Lambda's stops
  # the plan like a task's.
  command = plan

  plan_options {
    target = [aws_route_table.private]
  }

  variables {
    enable_nat_gateway = false
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0b1a0b1a0b1a0b1a3"]
    }
  }

  expect_failures = [data.aws_network_interfaces.private_subnets]
}

run "g2_stops_the_plan_even_with_a_pending_subnet_change" {
  # VER of this PR, P2-1: a data source that references a managed resource with a pending
  # change is read during apply, and in that apply the NAT is destroyed before the table is
  # updated. A tag change on the private subnets is such a change; G2 must still stop the
  # PLAN.
  command = plan

  plan_options {
    target = [aws_route_table.private, aws_subnet.private]
  }

  variables {
    enable_nat_gateway = false
    environment        = "production-drift"
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0a5c0a5c0a5c0a5c2"]
    }
  }

  expect_failures = [data.aws_network_interfaces.private_subnets]
}

run "a_plan_that_targets_only_the_nat_reads_g2_too" {
  # `plan -target=aws_nat_gateway.main` leaves the route table out; the NAT's depends_on
  # brings G2 in anyway.
  command = plan

  plan_options {
    target = [aws_nat_gateway.main]
  }

  variables {
    enable_nat_gateway = false
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0a5c0a5c0a5c0a5c2"]
    }
  }

  expect_failures = [data.aws_network_interfaces.private_subnets]
}

run "a_private_table_with_no_subnets_reads_no_enis" {
  # Nothing routes through the table, so nothing can lose its route out.
  command = plan

  plan_options {
    target = [aws_route_table.private]
  }

  variables {
    enable_nat_gateway = false
  }

  override_data {
    target = data.aws_route_table.private_without_nat
    values = {
      associations = []
    }
  }

  assert {
    condition     = length(data.aws_network_interfaces.private_subnets) == 0
    error_message = "With no subnet associated with the private table, G2 has nothing to read."
  }
}

run "nat_on_keeps_one_default_route_to_the_nat" {
  # The rollback value: the same single route the old `dynamic "route"` block declared.
  command = apply

  plan_options {
    target = [aws_route_table.private]
  }

  variables {
    enable_nat_gateway = true
  }

  assert {
    condition     = length(aws_nat_gateway.main) == 1 && length(aws_eip.nat) == 1
    error_message = "With the NAT on (single_nat_gateway) there is one NAT gateway and one EIP."
  }

  assert {
    condition     = length(aws_route_table.private[0].route) == 1
    error_message = "With the NAT on the private table carries exactly one route."
  }

  assert {
    condition     = one(aws_route_table.private[0].route).cidr_block == "0.0.0.0/0" && one(aws_route_table.private[0].route).nat_gateway_id == aws_nat_gateway.main[0].id
    error_message = "The private default route must point at the NAT gateway."
  }

  assert {
    condition     = length(data.aws_network_interfaces.private_subnets) == 0
    error_message = "With the NAT on, G2 reads nothing."
  }
}

run "turning_the_nat_off_deletes_its_route" {
  # Step e from a state that has the NAT: the plan drops the NAT, its EIP and the route.
  command = plan

  plan_options {
    target = [aws_route_table.private]
  }

  variables {
    enable_nat_gateway = false
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0e0d0e0d0e0d0e0d1"]
    }
  }

  assert {
    condition     = length(aws_nat_gateway.main) == 0 && length(aws_eip.nat) == 0
    error_message = "Turning the NAT off plans the NAT gateway and its EIP away."
  }

  assert {
    condition     = length(aws_route_table.private) == 1 && length(aws_route_table.private[0].route) == 0
    error_message = "Turning the NAT off keeps the private table and deletes its default route."
  }
}

run "nat_off_with_the_tasks_private_is_refused_by_the_variables" {
  # AWS still shows a NAT route here, so G3 passes and only the precondition on the
  # variables can fail this run.
  command = plan

  plan_options {
    target = [aws_ecs_service.facilitator]
  }

  variables {
    enable_nat_gateway          = false
    ecs_tasks_in_public_subnets = false
  }

  override_data {
    target = data.aws_network_interfaces.private_subnets
    values = {
      ids = ["eni-0e0d0e0d0e0d0e0d1"]
    }
  }

  override_data {
    target = data.aws_nat_gateways.available
    values = {
      ids = ["nat-0c4a0c4a0c4a0c4a4"]
    }
  }

  override_data {
    target = data.aws_route_table.private_for_tasks
    values = {
      associations = [
        { subnet_id = "subnet-0a0a0a0a0a0a0a0a1" },
        { subnet_id = "subnet-0b0b0b0b0b0b0b0b2" },
      ]
      routes = [{ cidr_block = "0.0.0.0/0", nat_gateway_id = "nat-0c4a0c4a0c4a0c4a4" }]
    }
  }

  expect_failures = [aws_ecs_service.facilitator]
}

run "tasks_back_to_private_without_an_available_nat_stop_the_plan" {
  # VER-X402-115 point 2: the variables say the NAT is on, AWS has none (the deploy never
  # creates it), so moving the tasks back stops before the move.
  command = plan

  plan_options {
    target = [aws_ecs_service.facilitator]
  }

  variables {
    enable_nat_gateway          = true
    ecs_tasks_in_public_subnets = false
  }

  override_data {
    target = data.aws_nat_gateways.available
    values = {
      ids = []
    }
  }

  override_data {
    target = data.aws_route_table.private_for_tasks
    values = {
      associations = [
        { subnet_id = "subnet-0a0a0a0a0a0a0a0a1" },
        { subnet_id = "subnet-0b0b0b0b0b0b0b0b2" },
      ]
      routes = [{ cidr_block = "0.0.0.0/0", nat_gateway_id = "nat-0c4a0c4a0c4a0c4a4" }]
    }
  }

  expect_failures = [aws_ecs_service.facilitator]
}

run "an_available_nat_without_the_private_route_stops_the_plan" {
  # VER of this PR, P2-2: a NAT applied without its route (-target=aws_nat_gateway.main, or
  # made by hand) is available, and the private table still has no way out.
  command = plan

  plan_options {
    target = [aws_ecs_service.facilitator]
  }

  variables {
    enable_nat_gateway          = true
    ecs_tasks_in_public_subnets = false
  }

  override_data {
    target = data.aws_nat_gateways.available
    values = {
      ids = ["nat-0c4a0c4a0c4a0c4a4"]
    }
  }

  override_data {
    target = data.aws_route_table.private_for_tasks
    values = {
      associations = [
        { subnet_id = "subnet-0a0a0a0a0a0a0a0a1" },
        { subnet_id = "subnet-0b0b0b0b0b0b0b0b2" },
      ]
      routes = []
    }
  }

  expect_failures = [aws_ecs_service.facilitator]
}

run "a_private_subnet_outside_the_routed_table_stops_the_plan" {
  # One task subnet is associated somewhere else (the VPC's main table, say): its tasks
  # would have no route out even though the private table has one.
  command = plan

  plan_options {
    target = [aws_ecs_service.facilitator]
  }

  variables {
    enable_nat_gateway          = true
    ecs_tasks_in_public_subnets = false
  }

  override_data {
    target = data.aws_nat_gateways.available
    values = {
      ids = ["nat-0c4a0c4a0c4a0c4a4"]
    }
  }

  override_data {
    target = data.aws_route_table.private_for_tasks
    values = {
      associations = [{ subnet_id = "subnet-0a0a0a0a0a0a0a0a1" }]
      routes       = [{ cidr_block = "0.0.0.0/0", nat_gateway_id = "nat-0c4a0c4a0c4a0c4a4" }]
    }
  }

  expect_failures = [aws_ecs_service.facilitator]
}

run "tasks_back_to_private_with_the_nat_routed" {
  command = plan

  plan_options {
    target = [aws_ecs_service.facilitator]
  }

  variables {
    enable_nat_gateway          = true
    ecs_tasks_in_public_subnets = false
  }

  override_data {
    target = data.aws_nat_gateways.available
    values = {
      ids = ["nat-0c4a0c4a0c4a0c4a4"]
    }
  }

  override_data {
    target = data.aws_route_table.private_for_tasks
    values = {
      associations = [
        { subnet_id = "subnet-0a0a0a0a0a0a0a0a1" },
        { subnet_id = "subnet-0b0b0b0b0b0b0b0b2" },
      ]
      routes = [{ cidr_block = "0.0.0.0/0", nat_gateway_id = "nat-0c4a0c4a0c4a0c4a4" }]
    }
  }

  assert {
    condition     = one(aws_ecs_service.facilitator.network_configuration).assign_public_ip == false
    error_message = "Back in the private subnets the tasks take no public IP."
  }
}

run "the_shipped_tasks_run_public_and_never_read_the_nats" {
  # production.auto.tfvars as committed: the deploy's plan reads none of G3's data sources
  # (count = 0), so it needs no new permission and makes no extra call. Last: Terraform
  # 1.9.8 panics on these assertions if a change leaves the data sources out of the graph,
  # and the runs above report that change by name first.
  command = plan

  plan_options {
    target = [aws_ecs_service.facilitator]
  }

  assert {
    condition     = length(data.aws_nat_gateways.available) == 0 && length(data.aws_route_table.private_for_tasks) == 0
    error_message = "With the tasks public G3 reads nothing."
  }

  assert {
    condition     = one(aws_ecs_service.facilitator.network_configuration).assign_public_ip == true
    error_message = "The shipped tasks take a public IP."
  }
}
