Recompile the Rust code and rebuild the Docker image locally:

**Pre-build Optimization Check:**
1. Verify `.dockerignore` exists (excludes docs/, tests/, *.md, scripts/)
2. Check Cargo.toml has optimized dependencies:
   - `solana-sdk` has `default-features = false` (NOT `features = ["full"]`)
   - `tokio` has minimal features (NOT `features = ["full"]`)
   - This saves 120+ crates and 3-4 min compile time

**Build Steps:**
3. Format all code: `just format-all`
4. Run clippy linter: `just clippy-all` - report any warnings or errors
5. Build the Rust binary in release mode: `cargo build --release`
6. Build the Docker image with version tag: `docker build --platform linux/amd64 --build-arg FACILITATOR_VERSION=dev-local -t facilitator-test .`
7. Report build status and image size (should be ~105MB)

**Expected Build Times:**
- First build (cold cache): 3-5 minutes
- Subsequent builds (code changes only): 30-90 seconds
- Dependency change (Cargo.toml): 3-5 minutes
- If build takes >10 minutes, dependencies may not be optimized

**Verification:**
8. Check image was created: `docker images facilitator-test`
9. Test container starts: `docker run --rm facilitator-test --version` (if version flag exists)

After successful build, ask the user if they want to:
- Start the container with `docker-compose up -d`
- Run local tests against the container
- Push to ECR with `/deploy-prod`

If any step fails, stop immediately and report the error with suggestions for fixing it.

**Troubleshooting:**
- If build is slow (>10 min): Check Cargo.toml for `features = ["full"]` bloat
- If "edition2024" error: Use stable Rust, not nightly
- If context transfer is slow: Verify `.dockerignore` exists
