NOTE: The Terraform files in terraform/environments/production/ are templates that need customization.

CRITICAL SETUP STEPS BEFORE TERRAFORM APPLY:

1. Create S3 backend:
   aws s3 mb s3://facilitator-terraform-state --region us-east-2
   aws s3api put-bucket-versioning --bucket facilitator-terraform-state --versioning-configuration Status=Enabled

2. Create DynamoDB table:
   aws dynamodb create-table --table-name facilitator-terraform-locks --attribute-definitions AttributeName=LockID,AttributeType=S --key-schema AttributeName=LockID,KeyType=HASH --billing-mode PAY_PER_REQUEST --region us-east-2

3. Create AWS Secrets:
   aws secretsmanager create-secret --name facilitator-evm-private-key --secret-string '{"private_key":"0x..."}' --region us-east-2
   aws secretsmanager create-secret --name facilitator-solana-keypair --secret-string '{"private_key":"[...]"}' --region us-east-2

4. Create ECR repository:
   aws ecr create-repository --repository-name facilitator --region us-east-2
   
5. Build and push Docker image:
   cd facilitator
   ./scripts/build-and-push.sh v1.0.0
   
6. Initialize and apply Terraform:
   cd terraform/environments/production
   terraform init
   terraform plan
   terraform apply

See docs/DEPLOYMENT.md for full instructions.
