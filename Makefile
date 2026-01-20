# VEX Gateway Local Development Makefile
# Provides convenient commands for setup and testing with Docker

.PHONY: help setup start test clean cleanup-db cleanup-all

# Default target
help:
	@echo "VEX Gateway Local Development Commands (Docker)"
	@echo "================================================"
	@echo ""
	@echo "Docker Commands:"
	@echo "  make docker-status  - Check Docker services status"
	@echo "  make docker-logs    - View all Docker logs"
	@echo ""
	@echo "Setup Commands:"
	@echo "  make setup          - Setup gateway for Docker-based development"
	@echo "  make keys           - Generate JWT keys only"
	@echo ""
	@echo "Server Commands:"
	@echo "  make start          - Start the gateway server"
	@echo "  make build          - Build the project"
	@echo ""
	@echo "Testing Commands:"
	@echo "  make test-auth      - Test JWT authentication"
	@echo "  make test-api       - Test API key authentication"
	@echo "  make test-all       - Run all tests"
	@echo ""
	@echo "Utility Commands:"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make help           - Show this help"
	@echo ""
	@echo "Quick Start:"
	@echo "  make setup && make start"
	@echo ""

# Docker monitoring commands (Docker services managed separately)
docker-status:
	@echo "📊 Docker Services Status:"
	@echo ""
	@docker-compose ps

docker-logs:
	@echo "📝 Docker Logs (press Ctrl+C to exit):"
	@docker-compose logs -f

# Complete setup
setup:
	@echo "🚀 Setting up VEX Gateway for Docker-based development..."
	@./scripts/setup_dev.sh

# Generate JWT keys
keys:
	@echo "🔑 Generating JWT keys..."
	@./scripts/generate_keys.sh

# Start gateway server
start:
	@echo "🚀 Starting VEX Gateway server..."
	@./scripts/start_server.sh

# Build project
build:
	@echo "🔨 Building project..."
	@cargo build

# Build release
build-release:
	@echo "🔨 Building project (release mode)..."
	@cargo build --release

# Test JWT authentication
test-auth:
	@echo "🧪 Testing JWT authentication..."
	@./test_scripts/test_auth.sh

# Test API key authentication
test-api:
	@echo "🔑 Testing API key authentication..."
	@./test_scripts/test_api_key.sh

# Run all tests
test-all: test-auth test-api
	@echo "✅ All tests completed!"

# Clean build artifacts
clean:
	@echo "🧹 Cleaning build artifacts..."
	@cargo clean
