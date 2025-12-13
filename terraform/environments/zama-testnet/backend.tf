# Terraform Backend Configuration
# Stores state in S3 with DynamoDB locking
# Uses separate state file from production x402-rs facilitator

terraform {
  backend "s3" {
    bucket         = "facilitator-terraform-state"
    key            = "zama-testnet/terraform.tfstate"
    region         = "us-east-2"
    encrypt        = true
    dynamodb_table = "facilitator-terraform-locks"
  }
}
