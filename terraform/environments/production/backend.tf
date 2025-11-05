# Terraform Backend Configuration
# Stores state in S3 with DynamoDB locking

terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket         = "facilitator-terraform-state"
    key            = "production/terraform.tfstate"
    region         = "us-east-2"
    encrypt        = true
    dynamodb_table = "facilitator-terraform-locks"
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project     = "facilitator"
      Environment = "production"
      ManagedBy   = "terraform"
      Owner       = "ultravioleta-dao"
    }
  }
}
