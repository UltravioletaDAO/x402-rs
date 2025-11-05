# x402-rs Upgrade Checklist

**Quick reference for upgrading x402-rs facilitator**

Print this checklist and check off each step during upgrades.

---

## Pre-Upgrade (Before Touching Code)

### Preparation
- [ ] Read `CUSTOMIZATIONS.md` to refresh on what's customized
- [ ] Check upstream releases: https://github.com/polyphene/x402-rs/releases
- [ ] Note target version: `_________________`
- [ ] Review upstream changelog for breaking changes
- [ ] Inform team of planned upgrade (schedule downtime if needed)

### Backup
- [ ] Run: `git status` (ensure working directory clean)
- [ ] Create backup: `$VERSION = "vX.X.X"; $BACKUP = "x402-rs-backup-$VERSION-$(Get-Date -Format 'yyyyMMdd-HHmmss')"; mkdir $BACKUP`
- [ ] Backup static: `cp x402-rs/static/ $BACKUP/static/ -Recurse`
- [ ] Backup handlers: `cp x402-rs/src/handlers.rs $BACKUP/`
- [ ] Backup network: `cp x402-rs/src/network.rs $BACKUP/`
- [ ] Backup Dockerfile: `cp x402-rs/Dockerfile $BACKUP/`
- [ ] Backup Cargo.toml: `cp x402-rs/Cargo.toml $BACKUP/`
- [ ] Create patch: `cd x402-rs && git diff upstream-mirror > ../$BACKUP/our-customizations.patch`
- [ ] Write backup path: `_________________`

---

## Upgrade Process

### Fetch Upstream
- [ ] Add remote (first time): `cd x402-rs && git remote add upstream https://github.com/polyphene/x402-rs`
- [ ] Fetch: `git fetch upstream`
- [ ] Update mirror: `git checkout upstream-mirror && git pull upstream main && git push origin upstream-mirror`
- [ ] Switch back: `git checkout karmacadabra-production` (or `master`)

### Review Changes
- [ ] Check commits: `git log --oneline HEAD..upstream-mirror -10`
- [ ] Check handlers: `git diff HEAD..upstream-mirror -- src/handlers.rs`
- [ ] Check network: `git diff HEAD..upstream-mirror -- src/network.rs`
- [ ] Check Dockerfile: `git diff HEAD..upstream-mirror -- Dockerfile`
- [ ] Check Cargo.toml: `git diff HEAD..upstream-mirror -- Cargo.toml`
- [ ] Note breaking changes: `_________________`

### Merge
- [ ] Merge: `git merge upstream-mirror`
- [ ] Check conflicts: `git status`
- [ ] If conflicts, resolve CAREFULLY:
  - [ ] `handlers.rs`: KEEP `include_str!("../static/index.html")`
  - [ ] `network.rs`: KEEP `HyperEvm`, `Optimism`, `Polygon`, `Solana`
  - [ ] `Dockerfile`: KEEP `RUN rustup default nightly`
  - [ ] `Cargo.toml`: MERGE (keep AWS deps if present, add upstream deps)
- [ ] Mark resolved: `git add <file>` for each

### Restore Branding (ALWAYS)
- [ ] Force restore static: `cp $BACKUP/static/ x402-rs/static/ -Recurse -Force`
- [ ] Verify branding: `Select-String -Path x402-rs/static/index.html -Pattern "Ultravioleta DAO"`
  - [ ] Output contains "Ultravioleta DAO"

---

## Verification

### Code Verification
- [ ] Check include_str: `Select-String -Path x402-rs/src/handlers.rs -Pattern "include_str"`
- [ ] Check HyperEVM: `Select-String -Path x402-rs/src/network.rs -Pattern "HyperEvm"`
- [ ] Check Optimism: `Select-String -Path x402-rs/src/network.rs -Pattern "Optimism"`
- [ ] Check Polygon: `Select-String -Path x402-rs/src/network.rs -Pattern "Polygon"`
- [ ] Check nightly: `Select-String -Path x402-rs/Dockerfile -Pattern "rustup default nightly"`

### Build Tests
- [ ] Clean: `cd x402-rs && cargo clean`
- [ ] Build: `cargo build --release`
  - [ ] Exit code 0 (success)
  - [ ] No errors related to edition/syntax

### Runtime Tests (Local)
- [ ] Run: `cargo run` (in separate terminal)
- [ ] Wait 10 seconds for startup
- [ ] Health check: `curl http://localhost:8080/health`
  - [ ] Returns 200 OK
- [ ] Branding check: `curl http://localhost:8080/ | Select-String "Ultravioleta DAO"`
  - [ ] Output contains "Ultravioleta DAO"
- [ ] Networks check: `curl http://localhost:8080/networks | Select-String "HyperEVM"`
  - [ ] Output contains "HyperEVM" or "Optimism"
- [ ] Payment test: `cd scripts && python test_glue_payment_simple.py --facilitator http://localhost:8080`
  - [ ] Payment succeeds
- [ ] Stop local: `Ctrl+C` in terminal running cargo

### Docker Tests
- [ ] Build: `cd x402-rs && docker build -t x402-test:latest .`
  - [ ] Exit code 0
- [ ] Run: `docker run -d -p 8080:8080 --name x402-test x402-test:latest`
- [ ] Wait 10 seconds
- [ ] Test: `curl http://localhost:8080/ | Select-String "Ultravioleta"`
  - [ ] Output contains "Ultravioleta"
- [ ] Cleanup: `docker stop x402-test && docker rm x402-test`

---

## Commit & Deploy

### Commit
- [ ] Stage: `git add .`
- [ ] Commit message (template):
```
Merge upstream x402-rs vX.X.X

- Preserved Ultravioleta DAO branding (static/)
- Preserved custom handlers (include_str! in get_root)
- Preserved custom networks (HyperEVM, Optimism, Polygon, Solana)
- Integrated upstream improvements:
  - [LIST WHAT YOU TOOK FROM UPSTREAM]

Tested:
- [x] Local cargo build/run
- [x] Branding verification
- [x] Custom networks verification
- [x] Docker build/run
- [x] All endpoints responding

Backup: [BACKUP_PATH]

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
```
- [ ] Commit: `git commit -m "..."`
- [ ] Review: `git log -1 -p`

### Deploy (DANGER ZONE)
- [ ] Push: `git push origin main`
- [ ] Deploy ECS: `aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2`
- [ ] Monitor: `aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query 'services[0].deployments'`
- [ ] Wait 60 seconds for deployment

---

## Post-Deployment Verification

### Production Checks
- [ ] Health: `curl https://facilitator.ultravioletadao.xyz/health`
  - [ ] Returns 200 OK
- [ ] Branding: `curl https://facilitator.ultravioletadao.xyz/ | Select-String "Ultravioleta"`
  - [ ] Output contains "Ultravioleta"
- [ ] Payment test: `cd tests/integration && python test_usdc_payment.py --network base-mainnet`
  - [ ] Payment succeeds

### ECS Logs
- [ ] Check for errors: `aws logs tail /ecs/facilitator-production/facilitator --follow --region us-east-2`
  - [ ] No errors in last 5 minutes

### End-to-End Test
- [ ] Full payment test: `cd tests/integration && python test_facilitator.py`
  - [ ] All networks operational
  - [ ] Payment verification succeeds

---

## Post-Upgrade Tasks

### Documentation
- [ ] Update `CUSTOMIZATIONS.md` version table
- [ ] Document any new customizations made during merge
- [ ] Update this checklist if process changed

### Cleanup
- [ ] Keep backup for 30 days: `$BACKUP_PATH`
- [ ] After 30 days stable: `rm -r $BACKUP_PATH`
- [ ] Delete local test artifacts: `rm -r x402-rs/target/` (if desired)

### Team Communication
- [ ] Notify team upgrade complete
- [ ] Share any issues encountered
- [ ] Update upgrade documentation if needed

---

## Emergency Rollback (If Production Breaks)

### Immediate Rollback
- [ ] Get current revision: `aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query 'services[0].taskDefinition'`
- [ ] Note revision number: `_________________`
- [ ] Rollback: `aws ecs update-service --cluster facilitator-production --service facilitator-production --task-definition facilitator-production:[PREVIOUS_REVISION] --force-new-deployment --region us-east-2`
- [ ] Verify: `curl https://facilitator.ultravioletadao.xyz/health`

### Git Rollback
- [ ] Find last good commit: `git log --oneline -5`
- [ ] Revert: `git revert HEAD` or `git reset --hard [COMMIT]`
- [ ] Push: `git push origin main --force` (if reset was used)

### Restore from Backup
- [ ] Copy backup: `cp $BACKUP/* x402-rs/ -Recurse -Force`
- [ ] Rebuild: `cd x402-rs && cargo build --release`
- [ ] Test locally (see "Runtime Tests" above)
- [ ] Redeploy (see "Deploy" above)

---

## Incident Notes

**If anything went wrong, document here:**

Date: `_________________`

What broke: `_________________`

Root cause: `_________________`

How fixed: `_________________`

Prevention for next time: `_________________`

---

**END OF CHECKLIST**

Keep this checklist updated after each upgrade to reflect lessons learned.
