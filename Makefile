# VEX Gateway Local Development Makefile
# Setup, build and run commands for local development against the Docker services

.PHONY: help setup keys start build build-release clean

# Default target
help:
	@echo "VEX Gateway Local Development Commands (Docker)"
	@echo "================================================"
	@echo ""
	@echo "Setup Commands:"
	@echo "  make setup          - Setup gateway for Docker-based development"
	@echo "  make keys           - Generate JWT keys only"
	@echo ""
	@echo "Server Commands:"
	@echo "  make start          - Start the gateway server"
	@echo "  make build          - Build the project"
	@echo ""
	@echo "Utility Commands:"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make help           - Show this help"
	@echo ""
	@echo "Quick Start:"
	@echo "  make setup && make start"
	@echo ""

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

# Clean build artifacts
clean:
	@echo "🧹 Cleaning build artifacts..."
	@cargo clean
