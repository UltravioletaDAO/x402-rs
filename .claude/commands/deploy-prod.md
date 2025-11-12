Build and deploy the facilitator to production AWS ECS:

**Phase 1: Version & Image Preparation**
1. Ask the user for a version tag (e.g., v1.2.1) to use for the Docker image
2. **CRITICAL**: Build Docker image with version information:
   ```bash
   docker build --platform linux/amd64 --build-arg FACILITATOR_VERSION=[version-tag] -t facilitator:[version-tag] .
   ```

3. Push to ECR:
   ```bash
   docker tag facilitator:[version-tag] 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:[version-tag]
   aws ecr get-login-password --region us-east-2 | docker login --username AWS --password-stdin 518898403364.dkr.ecr.us-east-2.amazonaws.com
   docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:[version-tag]
   ```

**Phase 2: Task Definition Update**
4. Get current task definition and clean it:
   ```bash
   aws ecs describe-task-definition --task-definition facilitator-production --region us-east-2 --query 'taskDefinition' > task-def-base.json
   cat task-def-base.json | jq 'del(.taskDefinitionArn, .revision, .status, .requiresAttributes, .placementConstraints, .compatibilities, .registeredAt, .registeredBy)' > task-def-clean.json
   ```

5. Update image tag in task definition:
   ```bash
   cat task-def-clean.json | jq '.containerDefinitions[0].image = "518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:[version-tag]"' > task-def-updated.json
   ```

6. Register new task definition:
   ```bash
   aws ecs register-task-definition --cli-input-json file://task-def-updated.json --region us-east-2 --query 'taskDefinition.{family:family,revision:revision}'
   ```
   Note the revision number (e.g., 13)

**Phase 3: Deploy to ECS**
7. Update ECS service with new task definition:
   ```bash
   aws ecs update-service --cluster facilitator-production --service facilitator-production --task-definition facilitator-production:[revision] --force-new-deployment --region us-east-2
   ```

8. Wait 60 seconds, then check deployment status:
   ```bash
   aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query 'services[0].deployments[*].{status:status,taskDef:taskDefinition,running:runningCount,rolloutState:rolloutState}'
   ```

**Phase 4: Verify Deployment**
9. Get running task ID and check logs (wait 30 more seconds if needed):
   ```bash
   aws ecs list-tasks --cluster facilitator-production --service-name facilitator-production --desired-status RUNNING --region us-east-2
   ```

10. Check logs for successful startup (use MSYS_NO_PATHCONV=1 on Windows):
   ```bash
   MSYS_NO_PATHCONV=1 aws logs get-log-events --log-group-name /ecs/facilitator-production --log-stream-name "ecs/facilitator/[task-id]" --region us-east-2 --start-from-head --limit 30 --query 'events[*].message' --output text | grep -E "(Successfully loaded blacklist|Starting server)"
   ```

11. **CRITICAL**: Verify version endpoint returns correct version:
   ```bash
   curl https://facilitator.ultravioletadao.xyz/version
   ```
   Should return: `{"version":"[version-tag]"}` (e.g., `{"version":"v1.3.3"}`)

12. Run `/test-prod` to verify all endpoints

**Important Notes:**
- **MUST** pass `--build-arg FACILITATOR_VERSION=[version-tag]` to docker build
- Always clean task definition before registering (remove AWS metadata)
- Task definition revisions increment automatically
- Deployment takes 1-2 minutes for health checks to pass
- Old tasks drain connections before terminating (2-5 minutes total)
- Version endpoint MUST reflect the actual deployed version

**Environment Variables to Verify:**
- `BLACKLIST_REQUIRED=true` (fail-fast if blacklist missing)
- `EVM_PRIVATE_KEY_MAINNET` / `EVM_PRIVATE_KEY_TESTNET` (wallet separation)
- `SOLANA_PRIVATE_KEY_MAINNET` / `SOLANA_PRIVATE_KEY_TESTNET`

If deployment fails, check CloudWatch logs for error messages and verify secrets are accessible.
