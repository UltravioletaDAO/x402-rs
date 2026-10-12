# The receipt store (idempotency_records): its TTL, and the alarms that make a store outage
# visible (alerts-receipts-store.tf). Against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# Nothing here reaches AWS. The names the Rust code shares with this configuration (the TTL
# attribute and the filter's pattern) are held by a Rust test in src/receipts/store.rs.

mock_provider "aws" {
  # The HTTPS listener validates certificate_arn as an ARN, and a mock makes up a random
  # string. No account id in it: this repo is public (.github/workflows/no-account-id.yml).
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-2::certificate/mock"
    }
  }
  # Alarm actions are validated as ARNs.
  mock_resource "aws_sns_topic" {
    defaults = {
      arn = "arn:aws:sns:us-east-2::facilitator-alerts-mock"
    }
  }
  # The imported alarms (alerts-imported.tf) build Execution Market's topic ARN from the
  # account id, and the teardown validates it once the facilitator's topic is in state.
  # A placeholder of the right shape, not an account.
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "000000000000"
    }
  }
}
mock_provider "archive" {}

# Setup, first, as in nat_guard.tftest.hcl: the teardown walks the whole configuration, and
# the certificate validation records take their for_each keys from the certificates.
run "the_certificates_exist" {
  command = apply

  plan_options {
    target = [aws_acm_certificate.main, aws_acm_certificate.metrics]
  }
}

run "the_receipt_store_expires_rows_and_is_watched" {
  command = apply

  plan_options {
    target = [
      aws_dynamodb_table.idempotency_store,
      aws_cloudwatch_log_metric_filter.receipt_store_failure,
      aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors,
      aws_cloudwatch_metric_alarm.receipt_store_latency,
    ]
  }

  # Receipt rows carry expires_at (src/receipts/store.rs, TTL_ATTRIBUTE): the table must
  # sweep on it, or the rows never leave.
  assert {
    condition     = aws_dynamodb_table.idempotency_store.ttl[0].attribute_name == "expires_at" && aws_dynamodb_table.idempotency_store.ttl[0].enabled == true
    error_message = "idempotency_records must expire rows on expires_at, the attribute receipt rows carry."
  }

  # Every receipt store alarm speaks on the facilitator's own topic, both ways.
  assert {
    condition = alltrue([
      for alarm in [
        aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors,
        aws_cloudwatch_metric_alarm.receipt_store_latency,
      ] :
      toset(alarm.alarm_actions) == toset([aws_sns_topic.alerts.arn]) && toset(alarm.ok_actions) == toset([aws_sns_topic.alerts.arn])
    ])
    error_message = "Every receipt store alarm must notify aws_sns_topic.alerts on alarm and on recovery."
  }

  # The facilitator's own count, a metric to read during an incident: the filter reads the
  # facilitator's log group for the bare token. It pages nobody (alerts-receipts-store.tf says
  # why): the paging alarms read AWS/DynamoDB only.
  assert {
    condition = (
      aws_cloudwatch_log_metric_filter.receipt_store_failure.pattern == "receipt_store_failure" &&
      aws_cloudwatch_log_metric_filter.receipt_store_failure.log_group_name == aws_cloudwatch_log_group.facilitator.name &&
      aws_cloudwatch_log_metric_filter.receipt_store_failure.metric_transformation[0].namespace == "Facilitator/Receipts" &&
      aws_cloudwatch_log_metric_filter.receipt_store_failure.metric_transformation[0].name == "ReceiptStoreFailures"
    )
    error_message = "The filter must count receipt_store_failure lines of the facilitator's log group into Facilitator/Receipts ReceiptStoreFailures."
  }

  assert {
    condition = alltrue(flatten([
      for alarm in [
        aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors,
        aws_cloudwatch_metric_alarm.receipt_store_latency,
        ] : [
        for q in alarm.metric_query : [for m in q.metric : m.namespace == "AWS/DynamoDB"]
      ]
    ]))
    error_message = "The receipt store alarms must page on AWS/DynamoDB metrics only, which no request can write."
  }

  # DynamoDB's side: SystemErrors and ThrottledRequests, for each operation the rail uses,
  # on this table, summed. (HCL's && does not short-circuit: the expression query, which
  # has no metric, is skipped by iterating its empty `metric` instead of indexing it.)
  assert {
    condition = toset(flatten([
      for q in aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors.metric_query : [
        for m in q.metric :
        "${m.metric_name}/${m.dimensions["Operation"]}" if m.dimensions["TableName"] == aws_dynamodb_table.idempotency_store.name
      ]
      ])) == toset([
      "SystemErrors/GetItem", "SystemErrors/PutItem", "SystemErrors/TransactWriteItems",
      "ThrottledRequests/GetItem", "ThrottledRequests/PutItem", "ThrottledRequests/TransactWriteItems",
    ])
    error_message = "The DynamoDB errors alarm must read SystemErrors and ThrottledRequests of GetItem, PutItem and TransactWriteItems on idempotency_records."
  }

  # The SDK retries these: two five-minute periods running, each with at least one.
  assert {
    condition = (
      length([
        for q in aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors.metric_query : q
        if q.expression == "SUM(FILL(METRICS(), 0))" && q.return_data == true
      ]) == 1 &&
      aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors.threshold == 1 &&
      aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors.evaluation_periods == 2 &&
      aws_cloudwatch_metric_alarm.receipt_store_dynamodb_errors.datapoints_to_alarm == 2
    )
    error_message = "The DynamoDB errors alarm must fire when the summed metrics reach one in two periods running."
  }

  # Latency: p99 of each of the three operations, the slowest one, over a second for three
  # periods running.
  assert {
    condition = toset(flatten([
      for q in aws_cloudwatch_metric_alarm.receipt_store_latency.metric_query : [
        for m in q.metric :
        "${m.metric_name}/${m.stat}/${m.dimensions["Operation"]}" if m.dimensions["TableName"] == aws_dynamodb_table.idempotency_store.name
      ]
      ])) == toset([
      "SuccessfulRequestLatency/p99/GetItem",
      "SuccessfulRequestLatency/p99/PutItem",
      "SuccessfulRequestLatency/p99/TransactWriteItems",
    ])
    error_message = "The latency alarm must read the p99 SuccessfulRequestLatency of GetItem, PutItem and TransactWriteItems on idempotency_records."
  }

  assert {
    condition = (
      aws_cloudwatch_metric_alarm.receipt_store_latency.threshold == 1000 &&
      aws_cloudwatch_metric_alarm.receipt_store_latency.evaluation_periods == 3 &&
      aws_cloudwatch_metric_alarm.receipt_store_latency.datapoints_to_alarm == 3 &&
      length([
        for q in aws_cloudwatch_metric_alarm.receipt_store_latency.metric_query : q
        if q.expression == "MAX(METRICS())" && q.return_data == true
      ]) == 1
    )
    error_message = "The latency alarm must fire when the slowest operation's p99 stays above 1000 ms for three periods."
  }
}
