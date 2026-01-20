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

# Start the server
cargo run --bin gateway
