"""Read-only operator preflight before publishing a receipt-enabled ECS task.

Requires boto3 and credentials allowed to describe resources and simulate IAM.
Never reads a secret value or changes permissions.
"""
import json
import boto3


def main():
    session = boto3.Session(region_name="us-east-2")
    ecs = session.client("ecs")
    service = ecs.describe_services(
        cluster="facilitator-production", services=["facilitator-production"]
    )["services"][0]
    task = ecs.describe_task_definition(taskDefinition=service["taskDefinition"])["taskDefinition"]
    secret = session.client("secretsmanager").describe_secret(
        SecretId="facilitator-receipt-signing-key"
    )["ARN"]
    table = session.client("dynamodb").describe_table(TableName="idempotency_records")["Table"]["TableArn"]
    iam = session.client("iam")
    checks = []
    for role, actions, resource in [
        (task["executionRoleArn"], ["secretsmanager:GetSecretValue"], secret),
        (task["taskRoleArn"], ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:DescribeTable"], table),
    ]:
        result = iam.simulate_principal_policy(
            PolicySourceArn=role, ActionNames=actions, ResourceArns=[resource]
        )
        checks.extend({"action":item["EvalActionName"], "decision":item["EvalDecision"]}
                      for item in result["EvaluationResults"])
    print(json.dumps({"readOnly":True, "checks":checks}))
    if not all(item["decision"] == "allowed" for item in checks):
        raise SystemExit("Apply the reviewed operator IAM grant before CI; the GitHub deploy identity cannot write role policies.")


if __name__ == "__main__":
    main()
