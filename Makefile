# VEX Gateway Local Development Makefile
# Provides convenient commands for setup and testing

.PHONY: help setup start test clean cleanup-db cleanup-all

# Default target
help:
	@echo "VEX Gateway Local Development Commands"
	@echo "======================================"
	@echo ""
	@echo "Setup Commands:"
	@echo "  make setup          - Complete development setup"
	@echo "  make keys           - Generate JWT keys only"
	@echo "  make db             - Setup database only"
	@echo "  make redis          - Setup Redis only"
	@echo "  make test-data      - Add test data only"
	@echo ""
	@echo "Server Commands:"
	@echo "  make start          - Start the server"
	@echo "  make build          - Build the project"
	@echo ""
	@echo "Testing Commands:"
	@echo "  make test-auth      - Test JWT authentication"
	@echo "  make test-api       - Test API key authentication"
	@echo "  make test-all       - Run all tests"
	@echo ""
	@echo "Utility Commands:"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make cleanup-db     - Clean database (remove all tables)"
	@echo "  make cleanup-all    - Clean everything (build + database)"
	@echo "  make help           - Show this help"
	@echo ""
	@echo "Quick Start:"
	@echo "  make setup && make start"
	@echo ""

# Complete setup
setup:
	@echo "🚀 Setting up VEX Gateway for local development..."
	@./scripts/setup_dev.sh

# Generate JWT keys
keys:
	@echo "🔑 Generating JWT keys..."
	@./scripts/generate_keys.sh

# Setup database
db:
	@echo "📊 Setting up database..."
	@./scripts/setup_database.sh

# Setup Redis
redis:
	@echo "🔴 Setting up Redis..."
	@./scripts/setup_redis.sh

# Setup test data
test-data:
	@echo "📝 Setting up test data..."
	@./scripts/setup_test_data.sh

# Start server
start:
	@echo "🚀 Starting VEX Gateway server..."
	@./scripts/start_server.sh

# Build project
build:
	@echo "🔨 Building project..."
	@cargo build

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

# Show quick start guide
quick-start:
	@./scripts/quick_start.sh

# Clean database (remove all tables and data)
cleanup-db:
	@echo "🧹 Cleaning database..."
	@./scripts/cleanup_database.sh

# Clean everything (build artifacts + database)
cleanup-all: clean cleanup-db
	@echo "🎉 Complete cleanup finished!"
