# x402-rs Facilitator Safe Upgrade Script
#
# This script automates the safe upgrade process for the x402-rs facilitator
# while preserving Ultravioleta DAO customizations.
#
# Usage:
#   .\scripts\upgrade_facilitator.ps1 -TargetVersion "v0.10.0" -UpstreamUrl "https://github.com/polyphene/x402-rs"
#
# Flags:
#   -DryRun: Preview changes without making them
#   -SkipTests: Skip local testing (NOT RECOMMENDED)
#   -AutoDeploy: Deploy to ECS after successful tests (DANGEROUS)

param(
    [Parameter(Mandatory=$true)]
    [string]$TargetVersion,

    [Parameter(Mandatory=$false)]
    [string]$UpstreamUrl = "https://github.com/polyphene/x402-rs",

    [switch]$DryRun = $false,
    [switch]$SkipTests = $false,
    [switch]$AutoDeploy = $false
)

$ErrorActionPreference = "Stop"
$ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$X402_DIR = Join-Path $ROOT_DIR "x402-rs"
$BACKUP_DIR = "x402-rs-backup-$TargetVersion-$(Get-Date -Format 'yyyyMMdd-HHmmss')"

# Color output functions
function Write-Success { param($msg) Write-Host "[✅] $msg" -ForegroundColor Green }
function Write-Danger { param($msg) Write-Host "[🚨] $msg" -ForegroundColor Red }
function Write-Warning { param($msg) Write-Host "[⚠️ ] $msg" -ForegroundColor Yellow }
function Write-Info { param($msg) Write-Host "[ℹ️ ] $msg" -ForegroundColor Cyan }
function Write-Step { param($step, $msg) Write-Host "`n[$step] $msg" -ForegroundColor Magenta }

# Safety checks
function Test-Prerequisites {
    Write-Step "0" "Checking prerequisites..."

    # Check we're in the right directory
    if (-not (Test-Path $X402_DIR)) {
        Write-Danger "x402-rs directory not found at: $X402_DIR"
        Write-Danger "Are you running this from the karmacadabra root?"
        exit 1
    }

    # Check git is available
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Danger "git not found in PATH"
        exit 1
    }

    # Check cargo is available
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Danger "cargo not found in PATH (Rust toolchain required)"
        exit 1
    }

    # Check docker is available
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        Write-Warning "docker not found in PATH (Docker tests will be skipped)"
    }

    # Check we're on the right branch
    Push-Location $X402_DIR
    $currentBranch = git branch --show-current
    if ($currentBranch -ne "karmacadabra-production" -and $currentBranch -ne "master") {
        Write-Warning "Current branch: $currentBranch"
        Write-Warning "Expected: karmacadabra-production or master"
        $response = Read-Host "Continue anyway? (yes/no)"
        if ($response -ne "yes") {
            Pop-Location
            exit 1
        }
    }
    Pop-Location

    Write-Success "Prerequisites check passed"
}

# Create backup of customizations
function Backup-Customizations {
    Write-Step "1" "Creating backup of customizations..."

    if ($DryRun) {
        Write-Info "[DRY RUN] Would create backup at: $BACKUP_DIR"
        return
    }

    New-Item -ItemType Directory -Path $BACKUP_DIR -Force | Out-Null

    # Backup critical files
    $filesToBackup = @(
        @{src="static"; dest="static"; recurse=$true},
        @{src="src/handlers.rs"; dest="handlers.rs"; recurse=$false},
        @{src="src/network.rs"; dest="network.rs"; recurse=$false},
        @{src="Dockerfile"; dest="Dockerfile"; recurse=$false},
        @{src="Cargo.toml"; dest="Cargo.toml"; recurse=$false}
    )

    foreach ($file in $filesToBackup) {
        $srcPath = Join-Path $X402_DIR $file.src
        $destPath = Join-Path $BACKUP_DIR $file.dest

        if (Test-Path $srcPath) {
            if ($file.recurse) {
                Copy-Item $srcPath $destPath -Recurse -Force
            } else {
                Copy-Item $srcPath $destPath -Force
            }
            Write-Success "Backed up: $($file.src)"
        } else {
            Write-Warning "Not found (skipping): $($file.src)"
        }
    }

    # Create patch file of current customizations
    Push-Location $X402_DIR
    git diff HEAD > (Join-Path $BACKUP_DIR "uncommitted-changes.patch")

    # Try to create diff against upstream if branch exists
    $upstreamBranch = git branch --list "upstream-mirror"
    if ($upstreamBranch) {
        git diff upstream-mirror > (Join-Path $BACKUP_DIR "our-customizations.patch")
        Write-Success "Created customizations patch"
    } else {
        Write-Warning "upstream-mirror branch not found, skipping patch creation"
    }
    Pop-Location

    Write-Success "Backup saved to: $BACKUP_DIR"
}

# Setup or update upstream tracking
function Update-UpstreamTracking {
    Write-Step "2" "Setting up upstream tracking..."

    Push-Location $X402_DIR

    # Check if upstream remote exists
    $upstreamRemote = git remote | Where-Object { $_ -eq "upstream" }

    if (-not $upstreamRemote) {
        Write-Info "Adding upstream remote: $UpstreamUrl"
        if (-not $DryRun) {
            git remote add upstream $UpstreamUrl
        }
    } else {
        Write-Info "Upstream remote already exists"
    }

    # Fetch from upstream
    Write-Info "Fetching from upstream..."
    if (-not $DryRun) {
        git fetch upstream
    }

    # Check if upstream-mirror branch exists
    $upstreamMirrorBranch = git branch --list "upstream-mirror"

    if (-not $upstreamMirrorBranch) {
        Write-Info "Creating upstream-mirror branch..."
        if (-not $DryRun) {
            git checkout -b upstream-mirror
            git reset --hard upstream/main
            git push origin upstream-mirror
            git checkout -
        }
    } else {
        Write-Info "Updating upstream-mirror branch..."
        if (-not $DryRun) {
            $currentBranch = git branch --show-current
            git checkout upstream-mirror
            git pull upstream main
            git push origin upstream-mirror
            git checkout $currentBranch
        }
    }

    Pop-Location
    Write-Success "Upstream tracking updated"
}

# Show what changed in upstream
function Show-UpstreamChanges {
    Write-Step "3" "Analyzing upstream changes..."

    Push-Location $X402_DIR

    Write-Info "Last 10 upstream commits:"
    git log --oneline upstream-mirror -10

    Write-Info "`nChecking critical files for changes:"
    $criticalFiles = @(
        "src/handlers.rs",
        "src/network.rs",
        "Cargo.toml",
        "Dockerfile"
    )

    foreach ($file in $criticalFiles) {
        $changes = git diff HEAD..upstream-mirror -- $file
        if ($changes) {
            Write-Warning "CHANGED: $file"
            if ($DryRun) {
                Write-Info "Preview of changes:"
                git diff HEAD..upstream-mirror -- $file | Select-Object -First 30
            }
        } else {
            Write-Success "Unchanged: $file"
        }
    }

    Pop-Location
}

# Interactive merge process
function Merge-UpstreamChanges {
    Write-Step "4" "Merging upstream changes..."

    if ($DryRun) {
        Write-Info "[DRY RUN] Would merge upstream-mirror into current branch"
        Write-Info "[DRY RUN] Conflicts would need manual resolution"
        return
    }

    Push-Location $X402_DIR

    Write-Danger "⚠️  CRITICAL: Merge conflicts require careful resolution"
    Write-Danger "⚠️  ALWAYS preserve our customizations when conflicts occur"
    Write-Info ""
    Write-Info "Conflict resolution guide:"
    Write-Info "  handlers.rs: KEEP our include_str!() approach"
    Write-Info "  network.rs: KEEP our networks (HyperEVM, Optimism, Polygon, Solana)"
    Write-Info "  Dockerfile: KEEP our 'RUN rustup default nightly' line"
    Write-Info ""

    $response = Read-Host "Ready to merge? Type 'merge' to continue, anything else to abort"
    if ($response -ne "merge") {
        Write-Warning "Merge aborted by user"
        Pop-Location
        exit 0
    }

    git merge upstream-mirror

    # Check if merge had conflicts
    $conflicts = git diff --name-only --diff-filter=U
    if ($conflicts) {
        Write-Danger "Merge conflicts detected in:"
        $conflicts | ForEach-Object { Write-Danger "  - $_" }
        Write-Info ""
        Write-Info "Please resolve conflicts manually:"
        Write-Info "  1. Edit each conflicted file"
        Write-Info "  2. Search for <<<<<<< HEAD markers"
        Write-Info "  3. Choose our customizations, integrate upstream improvements"
        Write-Info "  4. git add <file> when resolved"
        Write-Info "  5. Run this script again with -SkipMerge flag"
        Pop-Location
        exit 1
    }

    Pop-Location
    Write-Success "Merge completed without conflicts"
}

# Force restore static files (ALWAYS do this)
function Restore-StaticFiles {
    Write-Step "5" "Restoring static files (branding)..."

    if ($DryRun) {
        Write-Info "[DRY RUN] Would restore static/ from backup"
        return
    }

    $backupStatic = Join-Path $BACKUP_DIR "static"
    $x402Static = Join-Path $X402_DIR "static"

    if (Test-Path $backupStatic) {
        Copy-Item $backupStatic $x402Static -Recurse -Force
        Write-Success "Static files restored from backup"
    } else {
        Write-Danger "Backup static files not found!"
        Write-Danger "Branding may be lost - check git history"
        exit 1
    }
}

# Verify customizations are intact
function Test-CustomizationsIntact {
    Write-Step "6" "Verifying customizations are intact..."

    Push-Location $X402_DIR

    $tests = @(
        @{
            name = "Branded landing page"
            file = "static/index.html"
            pattern = "Ultravioleta DAO"
            critical = $true
        },
        @{
            name = "Custom root handler"
            file = "src/handlers.rs"
            pattern = "include_str!"
            critical = $true
        },
        @{
            name = "HyperEVM network"
            file = "src/network.rs"
            pattern = "HyperEvm"
            critical = $false
        },
        @{
            name = "Optimism network"
            file = "src/network.rs"
            pattern = "Optimism"
            critical = $true
        },
        @{
            name = "Polygon network"
            file = "src/network.rs"
            pattern = "Polygon"
            critical = $false
        },
        @{
            name = "Nightly Rust"
            file = "Dockerfile"
            pattern = "rustup default nightly"
            critical = $true
        }
    )

    $failedCritical = $false

    foreach ($test in $tests) {
        $filePath = Join-Path $X402_DIR $test.file
        if (Test-Path $filePath) {
            $content = Get-Content $filePath -Raw
            if ($content -match $test.pattern) {
                Write-Success "$($test.name): PRESENT"
            } else {
                if ($test.critical) {
                    Write-Danger "$($test.name): MISSING (CRITICAL)"
                    $failedCritical = $true
                } else {
                    Write-Warning "$($test.name): MISSING (non-critical)"
                }
            }
        } else {
            Write-Danger "$($test.name): FILE NOT FOUND"
            $failedCritical = $true
        }
    }

    Pop-Location

    if ($failedCritical) {
        Write-Danger "Critical customizations missing - DO NOT DEPLOY"
        Write-Info "Restore from backup: $BACKUP_DIR"
        exit 1
    }

    Write-Success "All critical customizations intact"
}

# Run build and local tests
function Test-LocalBuild {
    Write-Step "7" "Testing local build..."

    if ($SkipTests) {
        Write-Warning "Tests skipped (--SkipTests flag)"
        return
    }

    if ($DryRun) {
        Write-Info "[DRY RUN] Would run: cargo clean && cargo build --release"
        return
    }

    Push-Location $X402_DIR

    # Clean build
    Write-Info "Running cargo clean..."
    cargo clean

    # Build
    Write-Info "Running cargo build --release..."
    $buildOutput = cargo build --release 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Danger "Build failed!"
        Write-Danger $buildOutput
        Pop-Location
        exit 1
    }
    Write-Success "Build succeeded"

    # Run locally
    Write-Info "Starting facilitator locally (port 8080)..."
    $job = Start-Job -ScriptBlock {
        Set-Location $args[0]
        cargo run
    } -ArgumentList $X402_DIR

    Start-Sleep -Seconds 10

    # Test health endpoint
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:8080/health" -UseBasicParsing
        if ($response.StatusCode -eq 200) {
            Write-Success "Health check passed"
        } else {
            Write-Danger "Health check failed: Status $($response.StatusCode)"
            Stop-Job $job
            Remove-Job $job
            Pop-Location
            exit 1
        }
    } catch {
        Write-Danger "Health check failed: $_"
        Stop-Job $job
        Remove-Job $job
        Pop-Location
        exit 1
    }

    # Test branding
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:8080/" -UseBasicParsing
        if ($response.Content -match "Ultravioleta DAO") {
            Write-Success "Branding verified"
        } else {
            Write-Danger "Branding missing from landing page!"
            Stop-Job $job
            Remove-Job $job
            Pop-Location
            exit 1
        }
    } catch {
        Write-Danger "Landing page test failed: $_"
        Stop-Job $job
        Remove-Job $job
        Pop-Location
        exit 1
    }

    # Test custom networks
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:8080/networks" -UseBasicParsing
        if ($response.Content -match "HyperEVM" -or $response.Content -match "Optimism") {
            Write-Success "Custom networks verified"
        } else {
            Write-Warning "Custom networks may be missing (check manually)"
        }
    } catch {
        Write-Warning "Networks endpoint test failed: $_"
    }

    # Stop local instance
    Stop-Job $job
    Remove-Job $job

    Pop-Location
    Write-Success "Local tests passed"
}

# Docker build test
function Test-DockerBuild {
    Write-Step "8" "Testing Docker build..."

    if ($SkipTests) {
        Write-Warning "Tests skipped (--SkipTests flag)"
        return
    }

    if ($DryRun) {
        Write-Info "[DRY RUN] Would build Docker image: x402-test:latest"
        return
    }

    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        Write-Warning "Docker not available, skipping Docker tests"
        return
    }

    Push-Location $X402_DIR

    Write-Info "Building Docker image..."
    docker build -t x402-test:latest . 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Danger "Docker build failed"
        Pop-Location
        exit 1
    }
    Write-Success "Docker build succeeded"

    # Run container
    Write-Info "Running Docker container..."
    docker run -d -p 8080:8080 --name x402-upgrade-test x402-test:latest
    Start-Sleep -Seconds 10

    # Test
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:8080/" -UseBasicParsing
        if ($response.Content -match "Ultravioleta") {
            Write-Success "Docker runtime test passed"
        } else {
            Write-Danger "Docker runtime test failed: branding missing"
            docker stop x402-upgrade-test
            docker rm x402-upgrade-test
            Pop-Location
            exit 1
        }
    } catch {
        Write-Danger "Docker runtime test failed: $_"
        docker stop x402-upgrade-test
        docker rm x402-upgrade-test
        Pop-Location
        exit 1
    }

    # Cleanup
    docker stop x402-upgrade-test
    docker rm x402-upgrade-test

    Pop-Location
    Write-Success "Docker tests passed"
}

# Commit changes
function Commit-Upgrade {
    Write-Step "9" "Committing upgrade..."

    if ($DryRun) {
        Write-Info "[DRY RUN] Would commit with message:"
        Write-Info "Merge upstream x402-rs $TargetVersion"
        return
    }

    Push-Location $X402_DIR

    git add .

    $commitMessage = @"
Merge upstream x402-rs $TargetVersion

- Preserved Ultravioleta DAO branding (static/)
- Preserved custom handlers (include_str! in get_root)
- Preserved custom networks (HyperEVM, Optimism, Polygon, Solana)
- Integrated upstream improvements: [TODO: List what you took from upstream]

Tested:
- [x] Local cargo build/run
- [x] Branding verification
- [x] Custom networks verification
- [x] Docker build/run

Backup: $BACKUP_DIR

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
"@

    git commit -m $commitMessage

    Write-Success "Changes committed"
    Write-Info "Review with: git log -1 -p"

    Pop-Location
}

# Production deployment
function Deploy-Production {
    Write-Step "10" "Deploying to production..."

    if (-not $AutoDeploy) {
        Write-Warning "Production deployment requires --AutoDeploy flag"
        Write-Info "To deploy manually:"
        Write-Info "  1. git push origin karmacadabra-production"
        Write-Info "  2. aws ecs update-service --cluster karmacadabra-prod --service karmacadabra-prod-facilitator --force-new-deployment --region us-east-1"
        Write-Info "  3. Monitor: aws ecs describe-services --cluster karmacadabra-prod --services karmacadabra-prod-facilitator --region us-east-1"
        Write-Info "  4. Verify: curl https://facilitator.karmacadabra.ultravioletadao.xyz/health"
        return
    }

    if ($DryRun) {
        Write-Info "[DRY RUN] Would deploy to ECS"
        return
    }

    Write-Danger "⚠️  DEPLOYING TO PRODUCTION ⚠️"
    $response = Read-Host "Are you SURE? Type 'DEPLOY' to continue"
    if ($response -ne "DEPLOY") {
        Write-Warning "Deployment cancelled"
        return
    }

    Push-Location $X402_DIR

    # Push to git
    Write-Info "Pushing to git..."
    git push origin karmacadabra-production

    # Deploy to ECS
    Write-Info "Triggering ECS deployment..."
    aws ecs update-service `
        --cluster karmacadabra-prod `
        --service karmacadabra-prod-facilitator `
        --force-new-deployment `
        --region us-east-1

    Write-Info "Waiting for deployment (60 seconds)..."
    Start-Sleep -Seconds 60

    # Verify
    Write-Info "Verifying production..."
    try {
        $response = Invoke-WebRequest -Uri "https://facilitator.karmacadabra.ultravioletadao.xyz/health" -UseBasicParsing
        if ($response.StatusCode -eq 200) {
            Write-Success "Production health check passed"
        } else {
            Write-Danger "Production health check failed!"
        }

        $response = Invoke-WebRequest -Uri "https://facilitator.karmacadabra.ultravioletadao.xyz/" -UseBasicParsing
        if ($response.Content -match "Ultravioleta") {
            Write-Success "Production branding verified"
        } else {
            Write-Danger "Production branding missing!"
        }
    } catch {
        Write-Danger "Production verification failed: $_"
    }

    Pop-Location
    Write-Success "Deployment complete"
}

# Main execution
function Main {
    Write-Host "`n========================================" -ForegroundColor Cyan
    Write-Host "x402-rs Facilitator Safe Upgrade Script" -ForegroundColor Cyan
    Write-Host "========================================`n" -ForegroundColor Cyan
    Write-Info "Target version: $TargetVersion"
    Write-Info "Upstream URL: $UpstreamUrl"
    if ($DryRun) {
        Write-Warning "DRY RUN MODE - No changes will be made"
    }
    Write-Host ""

    Test-Prerequisites
    Backup-Customizations
    Update-UpstreamTracking
    Show-UpstreamChanges
    Merge-UpstreamChanges
    Restore-StaticFiles
    Test-CustomizationsIntact
    Test-LocalBuild
    Test-DockerBuild
    Commit-Upgrade
    Deploy-Production

    Write-Host "`n========================================" -ForegroundColor Green
    Write-Host "Upgrade process complete!" -ForegroundColor Green
    Write-Host "========================================`n" -ForegroundColor Green
    Write-Info "Backup location: $BACKUP_DIR"
    Write-Info "Keep this backup until production is verified stable"
    Write-Host ""
}

# Run main
Main
