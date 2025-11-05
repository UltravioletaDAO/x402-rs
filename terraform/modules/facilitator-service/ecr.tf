# ============================================================================
# ECR REPOSITORIES - Docker Image Storage
# ============================================================================
# COST: $0.10/GB/month for storage
# OPTIMIZATION: Lifecycle policies to delete old images

# ----------------------------------------------------------------------------
# ECR Repositories (one per agent)
# ----------------------------------------------------------------------------

resource "aws_ecr_repository" "agents" {
  for_each = var.agents

  name                 = "${var.project_name}/${each.key}"
  image_tag_mutability = "MUTABLE"
  force_delete         = true # Allow deletion even with images present

  image_scanning_configuration {
    scan_on_push = true # Security best practice
  }

  encryption_configuration {
    encryption_type = "AES256" # Free (vs KMS which costs $1/month)
  }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-ecr"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# ECR Lifecycle Policies (COST OPTIMIZATION)
# ----------------------------------------------------------------------------
# Automatically delete old images to save storage costs

resource "aws_ecr_lifecycle_policy" "agents" {
  for_each = var.agents

  repository = aws_ecr_repository.agents[each.key].name

  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Keep last 5 images"
        selection = {
          tagStatus     = "tagged"
          tagPrefixList = ["v"]
          countType     = "imageCountMoreThan"
          countNumber   = 5
        }
        action = {
          type = "expire"
        }
      },
      {
        rulePriority = 2
        description  = "Delete untagged images after 7 days"
        selection = {
          tagStatus   = "untagged"
          countType   = "sinceImagePushed"
          countUnit   = "days"
          countNumber = 7
        }
        action = {
          type = "expire"
        }
      }
    ]
  })
}

# ----------------------------------------------------------------------------
# ECR Repository Policies (Optional - for cross-account access)
# ----------------------------------------------------------------------------
# Uncomment if you need to allow other AWS accounts to pull images

# resource "aws_ecr_repository_policy" "agents" {
#   for_each = var.agents
#
#   repository = aws_ecr_repository.agents[each.key].name
#
#   policy = jsonencode({
#     Version = "2012-10-17"
#     Statement = [
#       {
#         Sid    = "AllowPull"
#         Effect = "Allow"
#         Principal = {
#           AWS = [
#             "arn:aws:iam::ACCOUNT_ID:root"
#           ]
#         }
#         Action = [
#           "ecr:GetDownloadUrlForLayer",
#           "ecr:BatchGetImage",
#           "ecr:BatchCheckLayerAvailability"
#         ]
#       }
#     ]
#   })
# }
