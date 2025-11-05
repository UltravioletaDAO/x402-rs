# ============================================================================
# Makefile for Karmacadabra ECS Fargate Deployment
# ============================================================================
# Usage:
#   make init           - Initialize Terraform
#   make plan           - Plan infrastructure changes
#   make apply          - Apply infrastructure changes
#   make destroy        - Destroy all infrastructure
#   make cost           - Estimate monthly costs
#   make push-images    - Build and push all Docker images
#   make deploy         - Full deployment (plan + apply + push-images)
#   make logs-validator - Tail logs for validator
#   make scale-up       - Scale all services to 2 tasks
#   make scale-down     - Scale all services to 1 task
#   make help           - Show this help message

.PHONY: help init plan apply destroy cost push-images deploy logs-* scale-up scale-down update-*

# Variables
PROJECT := karmacadabra
ENV := prod
CLUSTER := $(PROJECT)-$(ENV)
REGION := us-east-1
DOCKER_DIR := ../..

# Colors
COLOR_RESET := \033[0m
COLOR_BOLD := \033[1m
COLOR_GREEN := \033[32m
COLOR_YELLOW := \033[33m
COLOR_BLUE := \033[34m

help:
	@echo "$(COLOR_BOLD)Karmacadabra ECS Fargate - Available Commands$(COLOR_RESET)"
	@echo ""
	@echo "$(COLOR_GREEN)Terraform Commands:$(COLOR_RESET)"
	@echo "  make init            - Initialize Terraform"
	@echo "  make plan            - Plan infrastructure changes"
	@echo "  make apply           - Apply infrastructure changes"
	@echo "  make destroy         - Destroy all infrastructure (WARNING: Destructive)"
	@echo "  make output          - Show Terraform outputs"
	@echo ""
	@echo "$(COLOR_GREEN)Docker Commands:$(COLOR_RESET)"
	@echo "  make ecr-login       - Login to ECR"
	@echo "  make build-validator - Build validator Docker image"
	@echo "  make push-validator  - Push validator image to ECR"
	@echo "  make push-images     - Build and push all images"
	@echo ""
	@echo "$(COLOR_GREEN)Deployment Commands:$(COLOR_RESET)"
	@echo "  make deploy          - Full deployment (plan + apply + push-images)"
	@echo "  make update-services - Force new deployment of all services"
	@echo ""
	@echo "$(COLOR_GREEN)Monitoring Commands:$(COLOR_RESET)"
	@echo "  make logs-validator       - Tail logs for validator"
	@echo "  make logs-karma-hello     - Tail logs for karma-hello"
	@echo "  make logs-abracadabra     - Tail logs for abracadabra"
	@echo "  make logs-skill-extractor - Tail logs for skill-extractor"
	@echo "  make logs-voice-extractor - Tail logs for voice-extractor"
	@echo "  make dashboard            - Open CloudWatch dashboard"
	@echo ""
	@echo "$(COLOR_GREEN)Operations Commands:$(COLOR_RESET)"
	@echo "  make scale-up        - Scale all services to 2 tasks"
	@echo "  make scale-down      - Scale all services to 1 task"
	@echo "  make scale-zero      - Scale all services to 0 tasks (stop all)"
	@echo "  make health-check    - Check health of all services"
	@echo "  make cost            - Show cost estimate"
	@echo ""
	@echo "$(COLOR_YELLOW)Quick Start:$(COLOR_RESET)"
	@echo "  1. cp terraform.tfvars.example terraform.tfvars"
	@echo "  2. Edit terraform.tfvars"
	@echo "  3. make deploy"
	@echo ""

# ============================================================================
# Terraform Commands
# ============================================================================

init:
	@echo "$(COLOR_BLUE)Initializing Terraform...$(COLOR_RESET)"
	terraform init

plan:
	@echo "$(COLOR_BLUE)Planning infrastructure changes...$(COLOR_RESET)"
	terraform plan -out=tfplan

apply:
	@echo "$(COLOR_BLUE)Applying infrastructure changes...$(COLOR_RESET)"
	terraform apply tfplan

destroy:
	@echo "$(COLOR_YELLOW)WARNING: This will destroy all infrastructure!$(COLOR_RESET)"
	@read -p "Are you sure? [yes/NO]: " confirm && [ "$$confirm" = "yes" ]
	terraform destroy

output:
	@terraform output

cost:
	@echo "$(COLOR_BLUE)Estimated Monthly Cost:$(COLOR_RESET)"
	@terraform output -json estimated_monthly_cost_usd | jq -r 'to_entries | .[] | "\(.key): \(.value)"'

# ============================================================================
# Docker Commands
# ============================================================================

ecr-login:
	@echo "$(COLOR_BLUE)Logging in to ECR...$(COLOR_RESET)"
	@aws ecr get-login-password --region $(REGION) | docker login --username AWS --password-stdin $$(aws sts get-caller-identity --query Account --output text).dkr.ecr.$(REGION).amazonaws.com

build-validator:
	@echo "$(COLOR_BLUE)Building validator Docker image...$(COLOR_RESET)"
	cd $(DOCKER_DIR) && docker build -f Dockerfile.agent -t $(PROJECT)/validator .

build-karma-hello:
	@echo "$(COLOR_BLUE)Building karma-hello Docker image...$(COLOR_RESET)"
	cd $(DOCKER_DIR) && docker build -f Dockerfile.agent -t $(PROJECT)/karma-hello .

build-abracadabra:
	@echo "$(COLOR_BLUE)Building abracadabra Docker image...$(COLOR_RESET)"
	cd $(DOCKER_DIR) && docker build -f Dockerfile.agent -t $(PROJECT)/abracadabra .

build-skill-extractor:
	@echo "$(COLOR_BLUE)Building skill-extractor Docker image...$(COLOR_RESET)"
	cd $(DOCKER_DIR) && docker build -f Dockerfile.agent -t $(PROJECT)/skill-extractor .

build-voice-extractor:
	@echo "$(COLOR_BLUE)Building voice-extractor Docker image...$(COLOR_RESET)"
	cd $(DOCKER_DIR) && docker build -f Dockerfile.agent -t $(PROJECT)/voice-extractor .

push-validator: ecr-login build-validator
	@echo "$(COLOR_BLUE)Pushing validator to ECR...$(COLOR_RESET)"
	@ECR_URL=$$(terraform output -json ecr_repository_urls | jq -r '.validator'); \
	docker tag $(PROJECT)/validator:latest $$ECR_URL:latest && \
	docker push $$ECR_URL:latest

push-karma-hello: ecr-login build-karma-hello
	@echo "$(COLOR_BLUE)Pushing karma-hello to ECR...$(COLOR_RESET)"
	@ECR_URL=$$(terraform output -json ecr_repository_urls | jq -r '."karma-hello"'); \
	docker tag $(PROJECT)/karma-hello:latest $$ECR_URL:latest && \
	docker push $$ECR_URL:latest

push-abracadabra: ecr-login build-abracadabra
	@echo "$(COLOR_BLUE)Pushing abracadabra to ECR...$(COLOR_RESET)"
	@ECR_URL=$$(terraform output -json ecr_repository_urls | jq -r '.abracadabra'); \
	docker tag $(PROJECT)/abracadabra:latest $$ECR_URL:latest && \
	docker push $$ECR_URL:latest

push-skill-extractor: ecr-login build-skill-extractor
	@echo "$(COLOR_BLUE)Pushing skill-extractor to ECR...$(COLOR_RESET)"
	@ECR_URL=$$(terraform output -json ecr_repository_urls | jq -r '."skill-extractor"'); \
	docker tag $(PROJECT)/skill-extractor:latest $$ECR_URL:latest && \
	docker push $$ECR_URL:latest

push-voice-extractor: ecr-login build-voice-extractor
	@echo "$(COLOR_BLUE)Pushing voice-extractor to ECR...$(COLOR_RESET)"
	@ECR_URL=$$(terraform output -json ecr_repository_urls | jq -r '."voice-extractor"'); \
	docker tag $(PROJECT)/voice-extractor:latest $$ECR_URL:latest && \
	docker push $$ECR_URL:latest

push-images: push-validator push-karma-hello push-abracadabra push-skill-extractor push-voice-extractor
	@echo "$(COLOR_GREEN)All images pushed successfully!$(COLOR_RESET)"

# ============================================================================
# Deployment Commands
# ============================================================================

deploy: plan apply push-images update-services
	@echo "$(COLOR_GREEN)Deployment complete!$(COLOR_RESET)"
	@echo ""
	@echo "$(COLOR_BOLD)Access your agents:$(COLOR_RESET)"
	@terraform output -json agent_health_check_urls | jq -r 'to_entries | .[] | "\(.key): \(.value)"'

update-services:
	@echo "$(COLOR_BLUE)Forcing new deployment of all services...$(COLOR_RESET)"
	@for service in validator karma-hello abracadabra skill-extractor voice-extractor; do \
		echo "Updating $(CLUSTER)-$$service..."; \
		aws ecs update-service --cluster $(CLUSTER) --service $(CLUSTER)-$$service --force-new-deployment --region $(REGION) > /dev/null; \
	done
	@echo "$(COLOR_GREEN)All services updated!$(COLOR_RESET)"

# ============================================================================
# Monitoring Commands
# ============================================================================

logs-validator:
	@echo "$(COLOR_BLUE)Tailing logs for validator...$(COLOR_RESET)"
	aws logs tail /ecs/$(CLUSTER)/validator --follow --region $(REGION)

logs-karma-hello:
	@echo "$(COLOR_BLUE)Tailing logs for karma-hello...$(COLOR_RESET)"
	aws logs tail /ecs/$(CLUSTER)/karma-hello --follow --region $(REGION)

logs-abracadabra:
	@echo "$(COLOR_BLUE)Tailing logs for abracadabra...$(COLOR_RESET)"
	aws logs tail /ecs/$(CLUSTER)/abracadabra --follow --region $(REGION)

logs-skill-extractor:
	@echo "$(COLOR_BLUE)Tailing logs for skill-extractor...$(COLOR_RESET)"
	aws logs tail /ecs/$(CLUSTER)/skill-extractor --follow --region $(REGION)

logs-voice-extractor:
	@echo "$(COLOR_BLUE)Tailing logs for voice-extractor...$(COLOR_RESET)"
	aws logs tail /ecs/$(CLUSTER)/voice-extractor --follow --region $(REGION)

dashboard:
	@echo "$(COLOR_BLUE)Opening CloudWatch dashboard...$(COLOR_RESET)"
	@terraform output cloudwatch_dashboard_url | xargs open || terraform output cloudwatch_dashboard_url

# ============================================================================
# Operations Commands
# ============================================================================

scale-up:
	@echo "$(COLOR_BLUE)Scaling all services to 2 tasks...$(COLOR_RESET)"
	@for service in validator karma-hello abracadabra skill-extractor voice-extractor; do \
		echo "Scaling $(CLUSTER)-$$service to 2..."; \
		aws ecs update-service --cluster $(CLUSTER) --service $(CLUSTER)-$$service --desired-count 2 --region $(REGION) > /dev/null; \
	done
	@echo "$(COLOR_GREEN)All services scaled up!$(COLOR_RESET)"

scale-down:
	@echo "$(COLOR_BLUE)Scaling all services to 1 task...$(COLOR_RESET)"
	@for service in validator karma-hello abracadabra skill-extractor voice-extractor; do \
		echo "Scaling $(CLUSTER)-$$service to 1..."; \
		aws ecs update-service --cluster $(CLUSTER) --service $(CLUSTER)-$$service --desired-count 1 --region $(REGION) > /dev/null; \
	done
	@echo "$(COLOR_GREEN)All services scaled down!$(COLOR_RESET)"

scale-zero:
	@echo "$(COLOR_YELLOW)Scaling all services to 0 tasks (stopping all agents)...$(COLOR_RESET)"
	@for service in validator karma-hello abracadabra skill-extractor voice-extractor; do \
		echo "Scaling $(CLUSTER)-$$service to 0..."; \
		aws ecs update-service --cluster $(CLUSTER) --service $(CLUSTER)-$$service --desired-count 0 --region $(REGION) > /dev/null; \
	done
	@echo "$(COLOR_GREEN)All services stopped!$(COLOR_RESET)"

health-check:
	@echo "$(COLOR_BLUE)Checking health of all services...$(COLOR_RESET)"
	@ALB_DNS=$$(terraform output -raw alb_dns_name); \
	for agent in validator karma-hello abracadabra skill-extractor voice-extractor; do \
		echo -n "$$agent: "; \
		curl -sf http://$$ALB_DNS/$$agent/health > /dev/null && echo "$(COLOR_GREEN)HEALTHY$(COLOR_RESET)" || echo "$(COLOR_YELLOW)UNHEALTHY$(COLOR_RESET)"; \
	done

# ============================================================================
# Utility Commands
# ============================================================================

validate:
	@echo "$(COLOR_BLUE)Validating Terraform configuration...$(COLOR_RESET)"
	terraform validate

fmt:
	@echo "$(COLOR_BLUE)Formatting Terraform files...$(COLOR_RESET)"
	terraform fmt -recursive

clean:
	@echo "$(COLOR_BLUE)Cleaning up temporary files...$(COLOR_RESET)"
	rm -f tfplan
	rm -rf .terraform/

.DEFAULT_GOAL := help
