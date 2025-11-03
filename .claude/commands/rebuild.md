Recompile the Rust code and rebuild the Docker image locally:

1. Format all code: `just format-all`
2. Run clippy linter: `just clippy-all` - report any warnings or errors
3. Build the Rust binary in release mode: `cargo build --release`
4. Build the Docker image: `docker build -t facilitator-test .`
5. Report build status and image size

After successful build, ask the user if they want to:
- Start the container with `docker-compose up -d`
- Run local tests against the container
- Just leave it built and ready

If any step fails, stop immediately and report the error with suggestions for fixing it.
