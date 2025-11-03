Build and deploy the facilitator to production AWS ECS:

1. Ask the user for a version tag (e.g., v1.0.1) to use for the Docker image
2. Run `./scripts/build-and-push.sh [version-tag]` to build and push the Docker image to ECR
3. Run `aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2` to deploy the new image
4. Wait 30 seconds, then check deployment status with `aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query 'services[0].deployments'`
5. Report the deployment status and remind the user to run `/test-prod` to verify the deployment

Note: This assumes AWS CLI is configured with appropriate credentials for the production account.
