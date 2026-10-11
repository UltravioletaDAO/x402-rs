# ============================================================================
# The receipt store (idempotency_records): failures and latency
# ============================================================================
#
# Since 2.51.0 every plain `exact` settle on an EVM network that serves it, and on native
# Hedera, is admitted through the receipt store before anything is sent, and the rail fails
# closed: with the store unreachable those settles answer `503 receipt_store_unavailable`
# and send nothing. That is the safe answer, and it is also a payment outage on every network
# the rail covers. These alarms make it a visible one, on the facilitator's own topic.
#
# Three views, because each misses something the others see:
#   - receipt_store_failures: the facilitator's own count of store requests that did not
#     answer as the store, after the SDK's own retries -- an error other than a lost
#     condition or contention, no answer within the 5 s the rail waits, an unreadable row
#     (src/receipts/store.rs, STORE_FAILURE). Timeouts and denied calls reach no DynamoDB
#     metric; on /settle each of these is a 503, on /verify a fallback to simulation.
#   - receipt_store_dynamodb_errors: DynamoDB's side, SystemErrors and ThrottledRequests on
#     the table for the operations the rail uses. The SDK retries these, so one is not yet
#     a failed payment: two five-minute periods running.
#   - receipt_store_latency: p99 of the requests that succeed, before they turn into timeouts.
#
# The table is shared with the Idempotency-Key cache (GetItem and PutItem): a failure of
# either is a failure of the same store.

locals {
  # GetItem reads every admission, PutItem writes its revisions, TransactWriteItems reserves
  # and readmits. The operator command's Scan is left out: it runs by hand, not on payments.
  receipt_store_operations = ["GetItem", "PutItem", "TransactWriteItems"]
}

resource "aws_cloudwatch_log_metric_filter" "receipt_store_failure" {
  name           = "facilitator-receipt-store-failure"
  log_group_name = aws_cloudwatch_log_group.facilitator.name

  # A bare token matched as a substring, as the other filters here: the line is emitted by
  # `tracing` with structured fields whose order is not a contract. A Rust test reads this
  # pattern back (store::requests, the_terraform_reads_the_attribute_and_the_message_written_here).
  pattern = "receipt_store_failure"

  metric_transformation {
    name          = "ReceiptStoreFailures"
    namespace     = "Facilitator/Receipts"
    value         = "1"
    unit          = "Count"
    default_value = "0"
  }
}

# One failure is a request the SDK could not save: on /settle a payer answered 503 (safe to
# resend). None should happen: `evaluation_periods = 1`, as for the DX402 store.
resource "aws_cloudwatch_metric_alarm" "receipt_store_failures" {
  alarm_name  = "facilitator-${var.environment}-receipt-store-failures"
  namespace   = "Facilitator/Receipts"
  metric_name = "ReceiptStoreFailures"
  statistic   = "Sum"

  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"

  # No line in five minutes means no failures, not an unknown.
  treat_missing_data = "notBreaching"

  alarm_description = join(" ", [
    "The facilitator could not read or write the receipt store (DynamoDB idempotency_records).",
    "While it lasts, exact settles on the networks the receipt rail covers answer",
    "503 receipt_store_unavailable and send nothing (safe to resend). Check the",
    "receipt-store-dynamodb-errors alarm, the task role's DynamoDB access and the",
    "receipt_store_failure lines (op, kind) in /ecs/facilitator-production.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-receipt-store-failures"
    Environment = var.environment
  }
}

resource "aws_cloudwatch_metric_alarm" "receipt_store_dynamodb_errors" {
  alarm_name          = "facilitator-${var.environment}-receipt-store-dynamodb-errors"
  comparison_operator = "GreaterThanOrEqualToThreshold"
  evaluation_periods  = 2
  datapoints_to_alarm = 2
  threshold           = 1
  treat_missing_data  = "notBreaching"

  # FILL: a period where one series has no datapoint still sums the others.
  metric_query {
    id          = "errors"
    expression  = "SUM(FILL(METRICS(), 0))"
    label       = "SystemErrors + ThrottledRequests on the receipt store"
    return_data = true
  }

  dynamic "metric_query" {
    for_each = setproduct(["SystemErrors", "ThrottledRequests"], local.receipt_store_operations)
    content {
      id          = lower("${metric_query.value[0]}_${metric_query.value[1]}")
      return_data = false

      metric {
        namespace   = "AWS/DynamoDB"
        metric_name = metric_query.value[0]
        period      = 300
        stat        = "Sum"
        dimensions = {
          TableName = aws_dynamodb_table.idempotency_store.name
          Operation = metric_query.value[1]
        }
      }
    }
  }

  alarm_description = join(" ", [
    "DynamoDB has answered the receipt store (idempotency_records) with server errors or",
    "throttling for ten minutes. The SDK retries them; those it cannot save show up in the",
    "receipt-store-failures alarm as 503 receipt_store_unavailable on exact settles.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-receipt-store-dynamodb-errors"
    Environment = var.environment
  }
}

# Single-item requests take milliseconds; the rail gives up after 5 s and answers 503. A p99
# over one second for fifteen minutes is the slope before the timeouts, not a blip.
resource "aws_cloudwatch_metric_alarm" "receipt_store_latency" {
  alarm_name          = "facilitator-${var.environment}-receipt-store-latency"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  datapoints_to_alarm = 3
  threshold           = 1000
  treat_missing_data  = "notBreaching"

  metric_query {
    id          = "slowest"
    expression  = "MAX(METRICS())"
    label       = "p99 SuccessfulRequestLatency of the slowest receipt store operation (ms)"
    return_data = true
  }

  dynamic "metric_query" {
    for_each = local.receipt_store_operations
    content {
      id          = "latency_${lower(metric_query.value)}"
      return_data = false

      metric {
        namespace   = "AWS/DynamoDB"
        metric_name = "SuccessfulRequestLatency"
        period      = 300
        stat        = "p99"
        unit        = "Milliseconds"
        dimensions = {
          TableName = aws_dynamodb_table.idempotency_store.name
          Operation = metric_query.value
        }
      }
    }
  }

  alarm_description = join(" ", [
    "p99 latency of the receipt store (idempotency_records) above 1 s for 15 minutes.",
    "The receipt rail waits 5 s per request before answering 503: payers are already",
    "waiting, and timeouts are next.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-receipt-store-latency"
    Environment = var.environment
  }
}
