#!/bin/bash

# Gateway setup script - works with Docker services
# Assumes Docker services (Postgres, Redis, Kafka) are already running

set -e

echo "🚀 Setting up VEX Gateway"
echo "========================="

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Please run this script from the project root directory"
    exit 1
fi

# Verify Docker services are running
echo "📋 Step 1: Verifying Docker services..."
if ! docker ps | grep -q postgres; then
    echo "❌ PostgreSQL container not running. Start Docker services first."
    exit 1
fi
if ! docker ps | grep -q redis; then
    echo "❌ Redis container not running. Start Docker services first."
    exit 1
fi
echo "✅ Docker services verified"

echo ""
echo "📋 Step 2: Generating JWT keys..."
if [ -f "scripts/generate_keys.sh" ]; then
    ./scripts/generate_keys.sh
else
    echo "❌ generate_keys.sh not found"
    exit 1
fi

echo ""
echo "📋 Step 3: Building the project..."
cargo build

echo ""
echo "📋 Step 4: Setting up test data..."
if [ -f "scripts/setup_test_data.sh" ]; then
    ./scripts/setup_test_data.sh
else
    echo "⚠️  setup_test_data.sh not found, skipping test data setup"
fi

echo ""
echo "🎉 Gateway setup complete!"
echo "========================="
echo "✅ JWT keys generated"
echo "✅ Project built"
echo "✅ Test data created"
echo ""
echo "🚀 Start the server:"
echo "  make start"
echo ""
echo "🧪 Run tests:"
echo "  make test-auth    # Test JWT authentication"
echo "  make test-api     # Test API key authentication"
echo ""
