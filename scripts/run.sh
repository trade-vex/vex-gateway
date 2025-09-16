#!/bin/bash
set -e

echo "🚀 VEX Gateway - Quick Start"
echo "============================"

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

# Function to check if Docker is running
check_docker() {
    if ! docker info > /dev/null 2>&1; then
        echo "❌ Docker is not running. Please start Docker and try again."
        exit 1
    fi
}

# Function to check if SSH agent has keys
check_ssh() {
    if ! ssh-add -l > /dev/null 2>&1; then
        echo "⚠️  SSH agent has no keys loaded. Attempting to add GitHub key..."
        if [ -f ~/.ssh/docker_github_key ]; then
            ssh-add ~/.ssh/docker_github_key
            echo "✅ GitHub SSH key added to agent"
        else
            echo "❌ GitHub SSH key not found at ~/.ssh/docker_github_key"
            echo "Please ensure your SSH key is available for Docker builds"
            exit 1
        fi
    fi
}

# Function to check and generate JWT keys
check_jwt_keys() {
    if [ ! -f "./keys/private.pem" ] || [ ! -f "./keys/public.pem" ]; then
        echo "⚠️  JWT keys not found. Generating new keys..."
        ./scripts/generate-keys.sh
        echo "✅ JWT keys generated successfully"
    else
        echo "✅ JWT keys already exist"
    fi
}

# Main execution
main() {
    case "${1:-start}" in
        "start")
            print_status "Starting VEX Gateway..."
            check_docker
            check_ssh
            check_jwt_keys
            
            # Start all services
            docker-compose up -d
            
            # Wait for services to be ready
            print_status "Waiting for services to be ready..."
            sleep 10
            
            # Show status
            echo ""
            echo "📊 Service Status:"
            docker-compose ps
            
            echo ""
            echo "🌐 Access Points:"
            echo "  • VEX Gateway API: http://localhost:8080"
            echo "  • Health Check: http://localhost:8080/health"
            echo "  • Redis Commander: http://localhost:8081"
            echo "  • pgAdmin: http://localhost:8082 (admin@vex-gateway.com/admin123)"
            echo "  • Aeron Media Driver: localhost:40123"
            echo ""
            
            # Test the setup
            print_status "Testing the setup..."
            if curl -f http://localhost:8080/health > /dev/null 2>&1; then
                print_success "VEX Gateway is running and accessible! 🎉"
            else
                echo "⚠️  VEX Gateway is running but may not be accessible externally."
                echo "   This is likely due to the application binding to 127.0.0.1 instead of 0.0.0.0"
                echo "   The application is running internally and can be accessed via Docker exec:"
                echo "   docker-compose exec app curl http://127.0.0.1:8081/health"
            fi
            ;;
        "stop")
            print_status "Stopping VEX Gateway..."
            docker-compose down
            print_success "VEX Gateway stopped"
            ;;
        "restart")
            print_status "Restarting VEX Gateway..."
            docker-compose restart
            print_success "VEX Gateway restarted"
            ;;
        "status")
            docker-compose ps
            ;;
        "logs")
            docker-compose logs -f
            ;;
        "build")
            print_status "Building VEX Gateway..."
            check_docker
            check_ssh
            docker-compose build
            print_success "Build completed"
            ;;
        "clean")
            print_status "Cleaning up VEX Gateway..."
            docker-compose down -v --remove-orphans
            print_success "Containers and volumes removed"
            ;;
        "clean-all")
            print_status "Performing complete cleanup..."
            docker-compose down -v --remove-orphans
            docker system prune -f
            docker volume prune -f
            if [ -d "keys" ]; then
                rm -rf keys
                print_success "JWT keys removed"
            fi
            print_success "Complete cleanup finished (containers, volumes, images, keys, cache)"
            ;;
        "reset")
            print_status "Resetting VEX Gateway (clean + rebuild)..."
            docker-compose down -v --remove-orphans
            docker-compose build --no-cache
            print_success "Reset completed"
            ;;
        "compose-keys")
            print_status "Removing Docker Compose containers and JWT keys..."
            docker-compose down -v --remove-orphans
            if [ -d "keys" ]; then
                rm -rf keys
                print_success "JWT keys removed"
            fi
            print_success "Docker Compose containers and keys removed"
            ;;
        *)
            echo "Usage: $0 {start|stop|restart|status|logs|build|clean|clean-all|reset|compose-keys}"
            echo ""
            echo "Commands:"
            echo "  start        - Start all services (default)"
            echo "  stop         - Stop all services"
            echo "  restart      - Restart all services"
            echo "  status       - Show service status"
            echo "  logs         - Show logs from all services"
            echo "  build        - Build all Docker images"
            echo "  clean        - Stop and remove containers and volumes"
            echo "  clean-all    - Complete cleanup (containers, volumes, images, keys, cache)"
            echo "  reset        - Clean and rebuild everything from scratch"
            echo "  compose-keys - Remove Docker Compose containers and JWT keys"
            exit 1
            ;;
    esac
}

# Run main function with all arguments
main "$@"
