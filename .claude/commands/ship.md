Full automated deployment pipeline - from uncommitted changes to production verification:

**Phase 0: Secrets Validation (CRITICAL - DO NOT SKIP)**
1. Validate all secrets exist before ANY deployment:
   ```bash
   cd terraform/environments/production && bash validate_secrets.sh us-east-2
   ```
   **STOP IMMEDIATELY if validation fails** - this prevents broken production deployments

**Phase 1: Pre-flight Checks**
2. Check current deployed version: `curl -s https://facilitator.ultravioletadao.xyz/version`
3. Run `git status` to check for uncommitted changes
4. Run `git log -1` to see last commit
5. Check if there are any uncommitted changes to deploy
6. Verify `.dockerignore` exists and Cargo.toml is optimized

**Phase 2: Commit Changes (if uncommitted changes exist)**
5. Run `git diff` to analyze what changed
6. Auto-generate a descriptive commit message based on the changes (be specific about what was modified - files, features, fixes)
7. Run: `git add .`
8. Run: `git commit -m "[auto-generated message]"`
9. Confirm commit was successful

**Phase 3: Version Tagging**
10. Ask user for version tag (e.g., v1.2.1, v1.4.0) OR auto-suggest incrementing from DEPLOYED version (not local)
11. **CRITICAL**: Update Cargo.toml version to match (without 'v' prefix, e.g., "1.7.9")
12. Commit the version bump: `git add Cargo.toml && git commit -m "chore: bump version to [version]"`
13. Run: `git tag [version]`
14. Optional: `git push && git push --tags` (ask user if they want to push to remote)

**Phase 4: Build & Push**
13. Format code: `just format-all` (optional, report if fails)
14. Lint code: `just clippy-all` (optional, report warnings)
15. Build Docker image: `docker build --platform linux/amd64 -t facilitator:[version] .`
16. Report build time and image size
17. Tag for ECR: `docker tag facilitator:[version] 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:[version]`
18. Login to ECR: `aws ecr get-login-password --region us-east-2 | docker login --username AWS --password-stdin 518898403364.dkr.ecr.us-east-2.amazonaws.com`
19. Push to ECR: `docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:[version]`

**Phase 5: Deploy to Production**
20. Get and clean current task definition:
    ```bash
    aws ecs describe-task-definition --task-definition facilitator-production --region us-east-2 --query 'taskDefinition' > task-def-base.json
    cat task-def-base.json | jq 'del(.taskDefinitionArn, .revision, .status, .requiresAttributes, .placementConstraints, .compatibilities, .registeredAt, .registeredBy)' > task-def-clean.json
    ```
21. Update image: `cat task-def-clean.json | jq '.containerDefinitions[0].image = "518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:[version]"' > task-def-updated.json`
22. Register task definition: `aws ecs register-task-definition --cli-input-json file://task-def-updated.json --region us-east-2`
23. Note the revision number from the response
24. Deploy: `aws ecs update-service --cluster facilitator-production --service facilitator-production --task-definition facilitator-production:[revision] --force-new-deployment --region us-east-2`
25. Wait 60 seconds for deployment to start

**Phase 6: Production Verification**
26. Check deployment status:
    ```bash
    aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query 'services[0].deployments[*].{status:status,running:runningCount}'
    ```
27. Get running task ID: `aws ecs list-tasks --cluster facilitator-production --service-name facilitator-production --desired-status RUNNING --region us-east-2`
28. Check logs (use MSYS_NO_PATHCONV=1 on Windows):
    ```bash
    MSYS_NO_PATHCONV=1 aws logs get-log-events --log-group-name /ecs/facilitator-production --log-stream-name "ecs/facilitator/[task-id]" --region us-east-2 --start-from-head --limit 30 --query 'events[*].message' --output text | grep -E "(Successfully loaded blacklist|Starting server|Initialized provider)"
    ```
29. Run health check: `curl https://facilitator.ultravioletadao.xyz/health`
30. Verify branding: `curl https://facilitator.ultravioletadao.xyz/ | grep -i "Ultravioleta"`
31. **CRITICAL**: Verify ALL network families are present (not just count):
    ```bash
    curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network' | sort
    ```
    Must include: EVM networks (18), Solana/Fogo (4), NEAR (2), Stellar (2) = 26 total
    **If ANY family is missing (especially NEAR or Stellar), deployment FAILED - check logs**
32. Verify blacklist loaded: `curl https://facilitator.ultravioletadao.xyz/blacklist | jq '{totalBlocked,loadedAtStartup}'`

**Phase 7: Final Report**
33. Display summary:
    ```
    ✓ DEPLOYMENT SUCCESSFUL

    Version: [tag]
    Commit: [hash] - [message]
    Deployed: [timestamp]
    Task Definition: facilitator-production:[revision]

    Production Status:
    ✓ Health check: PASS
    ✓ Branding: PASS
    ✓ Networks: [count] networks available
    ✓ Blacklist: [count] addresses blocked, loaded at startup
    ✓ Wallet separation: Mainnet/testnet wallets configured

    Site is live at: https://facilitator.ultravioletadao.xyz
    Blacklist endpoint: https://facilitator.ultravioletadao.xyz/blacklist
    ```

**Build Time Expectations:**
- First build (cold cache): 3-5 minutes
- Code changes only: 30-90 seconds
- Full pipeline: 5-8 minutes total

If ANY step fails, STOP immediately and report the error with specific troubleshooting steps. Do NOT continue to next phases if earlier phases fail.

**User Interaction**: Only ask for version tag. Everything else should be automated. Show progress updates at each phase so user knows what's happening.

**Critical Checks:**
- Blacklist must show `loadedAtStartup: true`
- Testnets must use testnet wallet (0x34033041...)
- Mainnets must use mainnet wallet (0x103040...)
- Total blocked addresses should be > 0
- **ALL 26 networks must be present** (EVM + Solana + NEAR + Stellar)
- NEAR must show feePayer: `uvd-facilitator.near` / `uvd-facilitator.testnet`
- Stellar must show public key starting with `G...`

**If networks are missing after deployment:**
1. Check logs: `aws logs tail /ecs/facilitator-production --since 5m --region us-east-2 | grep -i "skipping\|warn"`
2. Look for "no RPC URL configured" messages
3. Verify task definition has all secrets: `aws ecs describe-task-definition --task-definition facilitator-production --region us-east-2 | jq '.taskDefinition.containerDefinitions[0].secrets[].name' | sort`
4. Re-run secrets validation: `cd terraform/environments/production && bash validate_secrets.sh us-east-2`
5. See `terraform/environments/production/SECRETS_MANAGEMENT.md` for fixing
