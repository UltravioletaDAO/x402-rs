Compare versions across local development, ECR registry, and deployed ECS service:

1. **Local Source Code Status**:
   - Show current git branch: `git branch --show-current`
   - Show latest commit: `git log -1 --oneline`
   - Check for uncommitted changes: `git status --short`
   - Show last commit date: `git log -1 --format=%cd`

2. **Local Docker Images**:
   - List local facilitator images: `docker images --filter "reference=facilitator*" --format "table {{.Repository}}\t{{.Tag}}\t{{.ID}}\t{{.CreatedAt}}\t{{.Size}}"`
   - Note: Check for both `facilitator-test` (local) and `facilitator` images

3. **ECR Registry**:
   - List images in ECR: `aws ecr describe-images --repository-name facilitator --region  --query 'sort_by(imageDetails,& imagePushedAt)[-5:]' --output table`
   - Show image tags and push dates for the 5 most recent images

4. **Currently Deployed on ECS**:
   - Get running task definition: `aws ecs describe-services --cluster facilitator-production --services facilitator-production --region  --query 'services[0].taskDefinition' --output text`
   - Get image from task definition: `aws ecs describe-task-definition --task-definition [task-def-from-above] --region  --query 'taskDefinition.containerDefinitions[0].image' --output text`
   - Get deployment status: `aws ecs describe-services --cluster facilitator-production --services facilitator-production --region  --query 'services[0].deployments[*].[status,taskDefinition,desiredCount,runningCount,createdAt]' --output table`

5. **Analysis & Summary**:
   - Compare git commit hash with any version labels in ECR images
   - Check if local source has uncommitted changes (warn if dirty)
   - Compare ECR latest image timestamp with ECS deployment timestamp
   - Determine if ECS is running the latest ECR image
   - Report clear summary:
     * "Local code is [clean/dirty]"
     * "Local Docker image built: [timestamp or 'not found']"
     * "Latest ECR image: [tag] pushed [timestamp]"
     * "ECS deployment: Running [image-tag] since [timestamp]"
     * "Status: [IN SYNC / OUT OF SYNC / NEEDS REBUILD / NEEDS DEPLOY]"

If AWS CLI commands fail, check that credentials are configured and the user has appropriate permissions.
