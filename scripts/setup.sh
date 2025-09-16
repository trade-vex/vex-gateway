#!/bin/bash
set -e

echo "🚀 VEX Gateway Docker Setup"
echo "============================"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Function to check if Docker is running
check_docker() {
    if ! docker info > /dev/null 2>&1; then
        print_error "Docker is not running. Please start Docker and try again."
        exit 1
    fi
    print_success "Docker is running"
}

# Function to check if SSH agent is running and has keys
check_ssh() {
    if ! ssh-add -l > /dev/null 2>&1; then
        print_warning "SSH agent has no keys loaded. Attempting to add GitHub key..."
        if [ -f ~/.ssh/docker_github_key ]; then
            ssh-add ~/.ssh/docker_github_key
            print_success "GitHub SSH key added to agent"
        else
            print_error "GitHub SSH key not found at ~/.ssh/docker_github_key"
            print_error "Please ensure your SSH key is available for Docker builds"
            exit 1
        fi
    else
        print_success "SSH agent has keys loaded"
    fi
}

# Function to check and generate JWT keys
check_jwt_keys() {
    if [ ! -f "./keys/private.pem" ] || [ ! -f "./keys/public.pem" ]; then
        print_warning "JWT keys not found. Generating new keys..."
        ./scripts/generate-keys.sh
        print_success "JWT keys generated successfully"
    else
        print_success "JWT keys already exist"
    fi
}

# Function to build Aeron Media Driver
build_aeron() {
    print_status "Building Aeron Media Driver..."
    
    # Check if aeron-binaries already exists
    if [ -d "aeron-binaries" ] && [ "$(ls -A aeron-binaries 2>/dev/null)" ]; then
        print_success "Aeron binaries already exist, skipping extraction"
    else
        print_status "Extracting Aeron binaries..."
        docker build -f Dockerfile.aeron -t vex-gateway-aeron-extractor:latest .
        docker run --rm -v "$(pwd)/aeron-binaries:/output" vex-gateway-aeron-extractor:latest
        print_success "Aeron binaries extracted to ./aeron-binaries/"
    fi
    
    # Build Aeron Media Driver runtime
    print_status "Building Aeron Media Driver runtime..."
    docker build -f Dockerfile.aeron-media-driver -t vex-gateway-aeron-media-driver:latest .
    print_success "Aeron Media Driver built successfully"
}

# Function to build application
build_app() {
    print_status "Building VEX Gateway application..."
    DOCKER_BUILDKIT=1 docker-compose build app
    print_success "Application built successfully"
}

# Function to start all services
start_services() {
    print_status "Starting all services..."
    docker-compose up -d
    print_success "All services started"
}

# Function to wait for services to be healthy
wait_for_services() {
    print_status "Waiting for services to be healthy..."
    
    # Wait for postgres
    print_status "Waiting for PostgreSQL..."
    local count=0
    while [ $count -lt 30 ]; do
        if docker-compose exec postgres pg_isready -U vex_user -d vex_gateway > /dev/null 2>&1; then
            break
        fi
        sleep 2
        count=$((count + 1))
    done
    if [ $count -eq 30 ]; then
        print_error "PostgreSQL failed to start within 60 seconds"
        return 1
    fi
    print_success "PostgreSQL is ready"
    
    # Wait for redis
    print_status "Waiting for Redis..."
    count=0
    while [ $count -lt 30 ]; do
        if docker-compose exec redis redis-cli ping 2>/dev/null | grep -q PONG; then
            break
        fi
        sleep 2
        count=$((count + 1))
    done
    if [ $count -eq 30 ]; then
        print_error "Redis failed to start within 60 seconds"
        return 1
    fi
    print_success "Redis is ready"
    
    # Wait for aeron
    print_status "Waiting for Aeron Media Driver..."
    count=0
    while [ $count -lt 30 ]; do
        if docker-compose exec aeron-media-driver pgrep aeronmd > /dev/null 2>&1; then
            break
        fi
        sleep 2
        count=$((count + 1))
    done
    if [ $count -eq 30 ]; then
        print_error "Aeron Media Driver failed to start within 60 seconds"
        return 1
    fi
    print_success "Aeron Media Driver is ready"
    
    # Wait for app (with longer timeout)
    print_status "Waiting for VEX Gateway application..."
    count=0
    while [ $count -lt 24 ]; do
        if curl -f http://localhost:8080/health > /dev/null 2>&1; then
            break
        fi
        sleep 5
        count=$((count + 1))
    done
    if [ $count -eq 24 ]; then
        print_error "VEX Gateway application failed to start within 120 seconds"
        return 1
    fi
    print_success "VEX Gateway application is ready"
}

# Function to test the setup
test_setup() {
    print_status "Testing the setup..."
    
    # Test health endpoint
    if curl -f http://localhost:8080/health > /dev/null 2>&1; then
        print_success "Health endpoint is responding"
    else
        print_error "Health endpoint is not responding"
        return 1
    fi
    
    # Test other services
    if curl -f http://localhost:8081 > /dev/null 2>&1; then
        print_success "Redis Commander is accessible"
    else
        print_warning "Redis Commander is not accessible"
    fi
    
    if curl -f http://localhost:8082 > /dev/null 2>&1; then
        print_success "pgAdmin is accessible"
    else
        print_warning "pgAdmin is not accessible"
    fi
}

# Function to show status
show_status() {
    echo ""
    echo "📊 Service Status:"
    echo "=================="
    docker-compose ps
    
    echo ""
    echo "🌐 Access Points:"
    echo "================="
    echo "  • VEX Gateway API: http://localhost:8080"
    echo "  • Health Check: http://localhost:8080/health"
    echo "  • Redis Commander: http://localhost:8081"
    echo "  • pgAdmin: http://localhost:8082 (admin@vex-gateway.com/admin123)"
    echo "  • Aeron Media Driver: localhost:40123"
    echo ""
}

# Function to clean up
cleanup() {
    print_status "Cleaning up..."
    docker-compose down
    print_success "Cleanup completed"
}

# Main execution
main() {
    case "${1:-setup}" in
        "setup")
            check_docker
            check_ssh
            check_jwt_keys
            build_aeron
            build_app
            start_services
            wait_for_services
            test_setup
            show_status
            print_success "VEX Gateway setup completed successfully! 🎉"
            ;;
        "start")
            check_docker
            start_services
            wait_for_services
            show_status
            ;;
        "stop")
            cleanup
            ;;
        "restart")
            cleanup
            main start
            ;;
        "status")
            show_status
            ;;
        "logs")
            docker-compose logs -f
            ;;
        "build")
            check_docker
            check_ssh
            build_aeron
            build_app
            print_success "Build completed"
            ;;
        "test")
            test_setup
            ;;
        "clean")
            print_status "Cleaning up VEX Gateway..."
            docker-compose down -v --remove-orphans
            print_success "Containers and volumes removed"
            ;;
        "clean-all")
            print_status "Performing deep cleanup..."
            docker-compose down -v --remove-orphans
            docker system prune -f
            docker volume prune -f
            print_success "Deep cleanup completed"
            ;;
        "reset")
            print_status "Resetting VEX Gateway (clean + rebuild)..."
            docker-compose down -v --remove-orphans
            docker-compose build --no-cache
            print_success "Reset completed"
            ;;
        *)
            echo "Usage: $0 {setup|start|stop|restart|status|logs|build|test|clean|clean-all|reset}"
            echo ""
            echo "Commands:"
            echo "  setup     - Complete setup (build and start all services)"
            echo "  start     - Start all services (assumes already built)"
            echo "  stop      - Stop all services"
            echo "  restart   - Stop and start all services"
            echo "  status    - Show service status and access points"
            echo "  logs      - Show logs from all services"
            echo "  build     - Build all Docker images"
            echo "  test      - Test the setup"
            echo "  clean     - Stop and remove containers and volumes"
            echo "  clean-all - Deep cleanup (containers, volumes, images, cache)"
            echo "  reset     - Clean and rebuild everything from scratch"
            exit 1
            ;;
    esac
}

# Run main function with all arguments
main "$@"
