#!/bin/bash

# Start the VEX Gateway server
# Assumes Docker services are already running

set -e

echo "🚀 Starting VEX Gateway Server..."
echo "================================="

# Check JWT keys exist
if [ ! -f "crates/libs/lib-auth/src/keys/private_key.pem" ]; then
    echo "❌ JWT keys not found. Run: make setup"
    exit 1
fi

# The gateway has no built-in defaults: development needs its configuration file
if [ "${APP_ENV:-development}" = "development" ] && [ -z "${APP_CONFIG_DIR:-}" ] && [ ! -f "config/development.toml" ]; then
    echo "❌ config/development.toml not found. Run: make setup"
    exit 1
fi

# Verify Docker services
if ! docker ps | grep -q postgres; then
    echo "❌ PostgreSQL not running. Start Docker services first."
    exit 1
fi
if ! docker ps | grep -q redis; then
    echo "❌ Redis not running. Start Docker services first."
    exit 1
fi

echo "✅ Docker services verified"
echo "✅ Starting gateway on http://localhost:8081"
echo ""
echo "Press Ctrl+C to stop"
echo "================================="

# Start the server. The gateway refuses to start without an explicit APP_ENV and never
# reads .env, so this local-dev script selects development unless APP_ENV is already set.
APP_ENV="${APP_ENV:-development}" cargo run --bin gateway
