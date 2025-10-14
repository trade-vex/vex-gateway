#!/bin/bash

# Start the VEX Gateway server for local development
# This script starts the server with proper configuration

set -e

echo "🚀 Starting VEX Gateway Server..."
echo "================================="

# Check if .env file exists
if [ ! -f ".env" ]; then
    echo "❌ .env file not found. Please run setup first:"
    echo "   ./scripts/setup_dev.sh"
    exit 1
fi

# Check if JWT keys exist
if [ ! -f "crates/libs/lib-auth/src/keys/private_key.pem" ]; then
    echo "❌ JWT keys not found. Please run setup first:"
    echo "   ./scripts/setup_dev.sh"
    exit 1
fi

# Check if PostgreSQL is running
if ! pg_isready -h localhost -p 5432 > /dev/null 2>&1; then
    echo "❌ PostgreSQL is not running. Please start it first:"
    echo "   brew services start postgresql  # macOS"
    echo "   sudo systemctl start postgresql  # Ubuntu"
    exit 1
fi

# Check if Redis is running
if ! redis-cli -h 127.0.0.1 -p 6379 ping > /dev/null 2>&1; then
    echo "❌ Redis is not running. Please start it first:"
    echo "   brew services start redis  # macOS"
    echo "   sudo systemctl start redis  # Ubuntu"
    exit 1
fi

echo "✅ Prerequisites checked"
echo "✅ Starting server on http://localhost:8081"
echo ""
echo "📝 Available endpoints:"
echo "   GET  /health           - Health check"
echo "   POST /signup           - User registration"
echo "   POST /login            - User login"
echo "   GET  /profile          - User profile (JWT required)"
echo "   POST /api-keys         - Create API key (JWT required)"
echo "   GET  /balances         - User balances (API key required)"
echo ""
echo "🧪 Run tests:"
echo "   ./test_scripts/test_auth.sh     - JWT authentication"
echo "   ./test_scripts/test_api_key.sh  - API key authentication"
echo ""
echo "Press Ctrl+C to stop the server"
echo "================================="

# Start the server
cargo run --bin gateway
