# VEX Gateway Docker Setup Makefile

.PHONY: help run start stop restart status logs build clean

# Default target
help:
	@echo "VEX Gateway Docker Setup"
	@echo "========================"
	@echo ""
	@echo "Available commands:"
	@echo "  make run          - Quick start (start all services)"
	@echo "  make start        - Start all services (assumes already built)"
	@echo "  make stop         - Stop all services"
	@echo "  make restart      - Restart all services"
	@echo "  make status       - Show service status and access points"
	@echo "  make logs         - Show logs from all services"
	@echo "  make build        - Build all Docker images"
	@echo "  make clean        - Complete cleanup (containers, volumes, images, keys, cache)"
	@echo ""

# Quick start (default)
run:
	@./scripts/run.sh start

# Start services
start:
	@./scripts/run.sh start

# Stop services
stop:
	@./scripts/run.sh stop

# Restart services
restart:
	@./scripts/run.sh restart

# Show status
status:
	@./scripts/run.sh status

# Show logs
logs:
	@./scripts/run.sh logs

# Build images
build:
	@./scripts/run.sh build

# Complete cleanup (containers, volumes, images, keys, cache)
clean:
	@./scripts/run.sh clean-all