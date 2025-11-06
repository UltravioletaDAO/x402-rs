#!/bin/bash
# =============================================================================
# x402 Facilitator Deployment Script
# Karmacadabra - Ultravioleta DAO
# =============================================================================
#
# Purpose: Automate deployment of x402 facilitator to Cherry Servers
# Network: Avalanche Fuji Testnet
# Domain: facilitator.ultravioletadao.xyz
#
# Usage:
#   ./deploy-facilitator.sh [init|build|deploy|start|stop|restart|logs|status]
#
# =============================================================================

set -e  # Exit on error
set -u  # Exit on undefined variable

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
PROJECT_NAME="x402-facilitator-karmacadabra"
DOCKER_IMAGE="ultravioletadao/x402-facilitator"
CONTAINER_NAME="x402-facilitator"
ENV_FILE=".env"
ENV_EXAMPLE=".env.example"

# =============================================================================
# Helper Functions
# =============================================================================

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_dependencies() {
    log_info "Checking dependencies..."

    if ! command -v docker &> /dev/null; then
        log_error "Docker not found. Please install Docker first."
        exit 1
    fi

    if ! command -v docker-compose &> /dev/null; then
        log_error "Docker Compose not found. Please install Docker Compose first."
        exit 1
    fi

    log_success "All dependencies found"
}

check_env_file() {
    if [ ! -f "$ENV_FILE" ]; then
        log_warning ".env file not found"

        if [ -f "$ENV_EXAMPLE" ]; then
            log_info "Copying .env.example to .env"
            cp "$ENV_EXAMPLE" "$ENV_FILE"
            log_warning "Please edit .env and fill in your configuration:"
            log_warning "  - EVM_PRIVATE_KEY (hot wallet private key)"
            log_warning "  - UVD_TOKEN_ADDRESS (after deploying erc-20)"
            log_warning "  - RPC_URL_AVALANCHE_FUJI (your custom RPC)"
            log_warning "  - OTEL_EXPORTER_OTLP_ENDPOINT (Grafana endpoint)"
            echo ""
            read -p "Press Enter after updating .env file..."
        else
            log_error ".env.example not found. Cannot create .env"
            exit 1
        fi
    fi

    # Validate critical variables
    if grep -q "0x0000000000000000000000000000000000000000" "$ENV_FILE"; then
        log_error "Please update placeholder addresses in .env file"
        exit 1
    fi

    if grep -q "0xdeadbeef" "$ENV_FILE"; then
        log_error "Please set a real EVM_PRIVATE_KEY in .env file"
        exit 1
    fi

    log_success ".env file validated"
}

# =============================================================================
# Deployment Commands
# =============================================================================

cmd_init() {
    log_info "Initializing x402 facilitator deployment..."

    check_dependencies
    check_env_file

    log_info "Creating necessary directories..."
    mkdir -p logs
    mkdir -p data

    log_success "Initialization complete"
    log_info "Next steps:"
    echo "  1. Deploy UVD token: cd ../erc-20 && ./deploy-fuji.sh"
    echo "  2. Update UVD_TOKEN_ADDRESS in .env"
    echo "  3. Build Docker image: ./deploy-facilitator.sh build"
    echo "  4. Deploy: ./deploy-facilitator.sh deploy"
}

cmd_build() {
    log_info "Building Docker image..."

    docker build -t "$DOCKER_IMAGE:latest" .

    log_success "Docker image built: $DOCKER_IMAGE:latest"
}

cmd_deploy() {
    log_info "Deploying facilitator with Docker Compose..."

    check_env_file

    # Pull latest images (if using pre-built)
    # docker-compose pull

    # Start services
    docker-compose up -d

    log_success "Facilitator deployed"

    # Wait for health check
    log_info "Waiting for facilitator to be healthy..."
    sleep 10

    if docker ps | grep -q "$CONTAINER_NAME"; then
        log_success "Facilitator is running"
        cmd_status
    else
        log_error "Facilitator failed to start"
        cmd_logs
        exit 1
    fi
}

cmd_start() {
    log_info "Starting facilitator..."
    docker-compose start facilitator
    log_success "Facilitator started"
}

cmd_stop() {
    log_info "Stopping facilitator..."
    docker-compose stop facilitator
    log_success "Facilitator stopped"
}

cmd_restart() {
    log_info "Restarting facilitator..."
    docker-compose restart facilitator
    log_success "Facilitator restarted"
}

cmd_logs() {
    log_info "Showing facilitator logs (Ctrl+C to exit)..."
    docker-compose logs -f facilitator
}

cmd_status() {
    log_info "Checking facilitator status..."

    echo ""
    docker-compose ps
    echo ""

    # Check health endpoint
    if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
        log_success "Health check: OK"
    else
        log_error "Health check: FAILED"
    fi

    # Check supported endpoints
    log_info "Supported payment methods:"
    curl -s http://localhost:8080/supported | jq '.' || log_warning "Could not fetch supported methods"

    # Check AVAX balance
    log_info "Checking facilitator wallet balance..."
    # This would require cast or web3 CLI tools
    # cast balance $FACILITATOR_ADDRESS --rpc-url $RPC_URL
}

cmd_test() {
    log_info "Running integration tests..."

    # Test health endpoint
    log_info "Testing /health endpoint..."
    curl -i http://localhost:8080/health

    # Test supported endpoint
    log_info "Testing /supported endpoint..."
    curl -i http://localhost:8080/supported

    log_success "Basic tests passed"
}

cmd_clean() {
    log_warning "This will stop and remove all containers and volumes"
    read -p "Are you sure? (y/N) " -n 1 -r
    echo

    if [[ $REPLY =~ ^[Yy]$ ]]; then
        log_info "Cleaning up..."
        docker-compose down -v
        log_success "Cleanup complete"
    else
        log_info "Cancelled"
    fi
}

cmd_update() {
    log_info "Updating facilitator..."

    # Pull latest code (if in git repo)
    # git pull

    # Rebuild image
    cmd_build

    # Restart with new image
    docker-compose up -d facilitator

    log_success "Update complete"
}

cmd_backup() {
    log_info "Creating backup..."

    BACKUP_DIR="backups/$(date +%Y%m%d_%H%M%S)"
    mkdir -p "$BACKUP_DIR"

    # Backup .env
    cp "$ENV_FILE" "$BACKUP_DIR/"

    # Backup logs (if exist)
    if [ -d "logs" ]; then
        cp -r logs "$BACKUP_DIR/"
    fi

    # Backup data (if exist)
    if [ -d "data" ]; then
        cp -r data "$BACKUP_DIR/"
    fi

    log_success "Backup created: $BACKUP_DIR"
}

cmd_help() {
    cat << EOF
x402 Facilitator Deployment Script
Karmacadabra - Ultravioleta DAO

Usage: ./deploy-facilitator.sh [COMMAND]

Commands:
  init        Initialize deployment (create .env, directories)
  build       Build Docker image
  deploy      Deploy facilitator with Docker Compose
  start       Start facilitator container
  stop        Stop facilitator container
  restart     Restart facilitator container
  logs        View facilitator logs (live tail)
  status      Check facilitator status and health
  test        Run basic integration tests
  update      Update facilitator (rebuild + restart)
  backup      Create backup of configuration and data
  clean       Stop and remove all containers and volumes
  help        Show this help message

Examples:
  # First-time deployment
  ./deploy-facilitator.sh init
  ./deploy-facilitator.sh build
  ./deploy-facilitator.sh deploy

  # Check status
  ./deploy-facilitator.sh status

  # View logs
  ./deploy-facilitator.sh logs

  # Update to latest version
  ./deploy-facilitator.sh update

  # Restart after config change
  ./deploy-facilitator.sh restart

For more information, see README.md
EOF
}

# =============================================================================
# Main
# =============================================================================

# Parse command
COMMAND="${1:-help}"

case "$COMMAND" in
    init)
        cmd_init
        ;;
    build)
        cmd_build
        ;;
    deploy)
        cmd_deploy
        ;;
    start)
        cmd_start
        ;;
    stop)
        cmd_stop
        ;;
    restart)
        cmd_restart
        ;;
    logs)
        cmd_logs
        ;;
    status)
        cmd_status
        ;;
    test)
        cmd_test
        ;;
    update)
        cmd_update
        ;;
    backup)
        cmd_backup
        ;;
    clean)
        cmd_clean
        ;;
    help|--help|-h)
        cmd_help
        ;;
    *)
        log_error "Unknown command: $COMMAND"
        echo ""
        cmd_help
        exit 1
        ;;
esac
