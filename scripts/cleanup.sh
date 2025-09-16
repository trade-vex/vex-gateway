#!/bin/bash
set -e

echo "🧹 VEX Gateway Cleanup"
echo "======================"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

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

# Function to show disk usage
show_disk_usage() {
    print_status "Current Docker disk usage:"
    docker system df
    echo ""
}

# Function to clean containers and volumes
clean_containers() {
    print_status "Stopping and removing containers..."
    docker-compose down -v --remove-orphans
    print_success "Containers and volumes removed"
}

# Function to clean images
clean_images() {
    print_status "Removing unused images..."
    docker image prune -f
    print_success "Unused images removed"
}

# Function to clean volumes
clean_volumes() {
    print_status "Removing unused volumes..."
    docker volume prune -f
    print_success "Unused volumes removed"
}

# Function to clean networks
clean_networks() {
    print_status "Removing unused networks..."
    docker network prune -f
    print_success "Unused networks removed"
}

# Function to clean build cache
clean_build_cache() {
    print_status "Clearing build cache..."
    docker builder prune -f
    print_success "Build cache cleared"
}

# Function to clean everything
clean_all() {
    print_status "Performing complete cleanup..."
    docker-compose down -v --remove-orphans
    docker system prune -a -f
    docker volume prune -f
    docker network prune -f
    docker builder prune -f
    print_success "Complete cleanup finished"
}

# Function to clean specific images
clean_vex_images() {
    print_status "Removing VEX Gateway images..."
    docker images | grep vex-gateway | awk '{print $3}' | xargs -r docker rmi -f
    print_success "VEX Gateway images removed"
}

# Function to clean aeron binaries
clean_aeron_binaries() {
    if [ -d "aeron-binaries" ]; then
        print_status "Removing Aeron binaries..."
        rm -rf aeron-binaries
        print_success "Aeron binaries removed"
    else
        print_status "No Aeron binaries to clean"
    fi
}

# Function to clean JWT keys
clean_jwt_keys() {
    if [ -d "keys" ]; then
        print_status "Removing JWT keys..."
        rm -rf keys
        print_success "JWT keys removed"
    else
        print_status "No JWT keys to clean"
    fi
}

# Function to clean Docker Compose containers and keys
clean_compose_and_keys() {
    print_status "Stopping and removing Docker Compose containers..."
    docker-compose down -v --remove-orphans
    print_success "Docker Compose containers and volumes removed"
    
    if [ -d "keys" ]; then
        print_status "Removing JWT keys..."
        rm -rf keys
        print_success "JWT keys removed"
    else
        print_status "No JWT keys to clean"
    fi
}

# Function to reset everything
reset_all() {
    print_warning "This will remove ALL VEX Gateway data and images!"
    read -p "Are you sure? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        print_status "Resetting VEX Gateway completely..."
        clean_all
        clean_vex_images
        clean_aeron_binaries
        clean_jwt_keys
        print_success "Complete reset finished"
    else
        print_status "Reset cancelled"
    fi
}

# Function to show help
show_help() {
    echo "Usage: $0 {containers|images|volumes|networks|cache|all|vex-images|aeron|keys|compose-keys|reset|status|help}"
    echo ""
    echo "Commands:"
    echo "  containers   - Stop and remove containers and volumes"
    echo "  images       - Remove unused images"
    echo "  volumes      - Remove unused volumes"
    echo "  networks     - Remove unused networks"
    echo "  cache        - Clear build cache"
    echo "  all          - Clean everything (containers, images, volumes, networks, cache)"
    echo "  vex-images   - Remove only VEX Gateway images"
    echo "  aeron        - Remove Aeron binaries"
    echo "  keys         - Remove JWT keys"
    echo "  compose-keys - Remove Docker Compose containers and JWT keys"
    echo "  reset        - Complete reset (everything + VEX data)"
    echo "  status       - Show current disk usage"
    echo "  help         - Show this help message"
    echo ""
}

# Main execution
main() {
    case "${1:-help}" in
        "containers")
            clean_containers
            ;;
        "images")
            clean_images
            ;;
        "volumes")
            clean_volumes
            ;;
        "networks")
            clean_networks
            ;;
        "cache")
            clean_build_cache
            ;;
        "all")
            clean_all
            ;;
        "vex-images")
            clean_vex_images
            ;;
        "aeron")
            clean_aeron_binaries
            ;;
        "keys")
            clean_jwt_keys
            ;;
        "compose-keys")
            clean_compose_and_keys
            ;;
        "reset")
            reset_all
            ;;
        "status")
            show_disk_usage
            ;;
        "help"|*)
            show_help
            ;;
    esac
}

# Run main function with all arguments
main "$@"
