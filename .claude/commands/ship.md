Full automated deployment pipeline - from uncommitted changes to production verification:

**Phase 1: Pre-flight Checks**
1. Run git status to check for uncommitted changes
2. Run git log -1 to see last commit
3. Check if there are any uncommitted changes to deploy

**Phase 2: Commit Changes (if uncommitted changes exist)**
4. Run git diff to analyze what changed
5. Auto-generate a descriptive commit message based on the changes (be specific about what was modified - files, features, fixes)
6. Run: `git add .`
7. Run: `git commit -m "[auto-generated message]"`
8. Confirm commit was successful

**Phase 3: Version Tagging**
9. Ask user for version tag (e.g., v1.2.1, v1.4.0) OR auto-suggest incrementing the last tag
10. Run: `git tag [version]`
11. Optional: `git push && git push --tags` (ask user if they want to push to remote)

**Phase 4: Build**
12. Format code: `just format-all`
13. Lint code: `just clippy-all`
14. Build Rust release: `cargo build --release`
15. Build Docker image: `docker build -t facilitator-test .`
16. Report any build failures immediately and STOP if errors occur

**Phase 5: Deploy to Production**
17. Run: `./scripts/build-and-push.sh [version-tag]` to push to ECR
18. Run: `aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2`
19. Wait 45 seconds for deployment to stabilize

**Phase 6: Production Verification**
20. Run health check: `curl https://facilitator.ultravioletadao.xyz/health`
21. Verify branding: `curl https://facilitator.ultravioletadao.xyz/ | grep -i "Ultravioleta"`
22. Check supported networks: `curl https://facilitator.ultravioletadao.xyz/supported`
23. Verify all custom networks are present (HyperEVM, Polygon, Optimism, Celo, Solana)

**Phase 7: Final Report**
24. Display summary:
    ```
    ✓ DEPLOYMENT SUCCESSFUL

    Version: [tag]
    Commit: [hash] - [message]
    Deployed: [timestamp]

    Production Status:
    ✓ Health check: PASS
    ✓ Branding: PASS
    ✓ Networks: [count] networks available
    ✓ Custom networks: All present

    Site is live at: https://facilitator.ultravioletadao.xyz
    ```

If ANY step fails, STOP immediately and report the error with specific troubleshooting steps. Do NOT continue to next phases if earlier phases fail.

**User Interaction**: Only ask for version tag. Everything else should be automated. Show progress updates at each phase so user knows what's happening.
