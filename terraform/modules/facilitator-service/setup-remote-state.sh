#!/bin/bash
# ============================================================================
# Setup Remote State Backend for Terraform
# ============================================================================
# This script creates an S3 bucket and DynamoDB table for Terraform state
# management with locking capabilities.
#
# Benefits:
# - Team collaboration (shared state)
# - State locking (prevents concurrent modifications)
# - State versioning (S3 versioning enabled)
# - Encryption at rest

set -e

# Configuration
BUCKET_NAME="karmacadabra-terraform-state"
DYNAMODB_TABLE="karmacadabra-terraform-locks"
REGION="us-east-1"

echo "========================================="
echo "Setting up Terraform Remote State"
echo "========================================="
echo "Bucket: $BUCKET_NAME"
echo "DynamoDB Table: $DYNAMODB_TABLE"
echo "Region: $REGION"
echo ""

# Check if bucket already exists
if aws s3 ls "s3://$BUCKET_NAME" 2>/dev/null; then
    echo "✓ S3 bucket already exists: $BUCKET_NAME"
else
    echo "Creating S3 bucket: $BUCKET_NAME"
    aws s3api create-bucket \
        --bucket "$BUCKET_NAME" \
        --region "$REGION"

    echo "✓ S3 bucket created"
fi

# Enable versioning
echo "Enabling versioning on S3 bucket..."
aws s3api put-bucket-versioning \
    --bucket "$BUCKET_NAME" \
    --versioning-configuration Status=Enabled

echo "✓ Versioning enabled"

# Enable encryption
echo "Enabling encryption on S3 bucket..."
aws s3api put-bucket-encryption \
    --bucket "$BUCKET_NAME" \
    --server-side-encryption-configuration '{
        "Rules": [{
            "ApplyServerSideEncryptionByDefault": {
                "SSEAlgorithm": "AES256"
            }
        }]
    }'

echo "✓ Encryption enabled"

# Block public access
echo "Blocking public access on S3 bucket..."
aws s3api put-public-access-block \
    --bucket "$BUCKET_NAME" \
    --public-access-block-configuration \
        "BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true"

echo "✓ Public access blocked"

# Check if DynamoDB table exists
if aws dynamodb describe-table --table-name "$DYNAMODB_TABLE" --region "$REGION" 2>/dev/null; then
    echo "✓ DynamoDB table already exists: $DYNAMODB_TABLE"
else
    echo "Creating DynamoDB table for state locking: $DYNAMODB_TABLE"
    aws dynamodb create-table \
        --table-name "$DYNAMODB_TABLE" \
        --attribute-definitions AttributeName=LockID,AttributeType=S \
        --key-schema AttributeName=LockID,KeyType=HASH \
        --billing-mode PAY_PER_REQUEST \
        --region "$REGION" \
        --tags "Key=Project,Value=Karmacadabra" "Key=ManagedBy,Value=Terraform" "Key=Environment,Value=prod"

    echo "Waiting for DynamoDB table to be active..."
    aws dynamodb wait table-exists --table-name "$DYNAMODB_TABLE" --region "$REGION"

    echo "✓ DynamoDB table created"
fi

echo ""
echo "========================================="
echo "Remote State Setup Complete!"
echo "========================================="
echo ""
echo "Next steps:"
echo "1. Uncomment the backend configuration in main.tf"
echo "2. Run: terraform init -migrate-state"
echo "3. Confirm the migration when prompted"
echo ""
echo "Backend configuration:"
echo "  backend \"s3\" {"
echo "    bucket         = \"$BUCKET_NAME\""
echo "    key            = \"ecs-fargate/terraform.tfstate\""
echo "    region         = \"$REGION\""
echo "    encrypt        = true"
echo "    dynamodb_table = \"$DYNAMODB_TABLE\""
echo "  }"
echo ""
