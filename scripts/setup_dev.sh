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
./scripts/generate_keys.sh

echo ""
echo "📋 Step 3: Creating config/development.toml..."
if [ -f "config/development.toml" ]; then
    echo "✅ config/development.toml already exists"
else
    cp config/development.toml.example config/development.toml
    echo "✅ Copied config/development.toml.example"
fi

echo ""
echo "📋 Step 4: Building the project..."
cargo build

echo ""
echo "📋 Step 5: Creating the schema and test data..."
./scripts/setup_database.sh
./scripts/setup_test_data.sh

echo ""
echo "🎉 Gateway setup complete!"
echo "========================="
echo "✅ JWT keys generated"
echo "✅ Project built"
echo "✅ Schema, seeds and test data created"
echo ""
echo "🚀 Start the server:"
echo "  make start"
echo ""
