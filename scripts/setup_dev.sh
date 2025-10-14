#!/bin/bash

# Complete setup script for local development testing
# This script sets up everything needed for manual authentication testing

set -e

echo "🚀 Setting up VEX Gateway for local development testing..."
echo "=================================================="

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Please run this script from the project root directory"
    exit 1
fi

# Create scripts directory if it doesn't exist
mkdir -p scripts

echo "📋 Step 1: Copying environment file..."
if [ ! -f ".env" ]; then
    cp env.example .env
    echo "✅ Created .env file from template"
else
    echo "✅ .env file already exists"
fi

echo ""
echo "📋 Step 2: Generating JWT keys..."
if [ -f "scripts/generate_keys.sh" ]; then
    ./scripts/generate_keys.sh
else
    echo "❌ generate_keys.sh not found. Please ensure it exists."
    exit 1
fi

echo ""
echo "📋 Step 3: Setting up database..."
echo "⚠️  Make sure PostgreSQL is running before continuing..."
read -p "Press Enter to continue with database setup..."

if [ -f "scripts/setup_database.sh" ]; then
    ./scripts/setup_database.sh
else
    echo "❌ setup_database.sh not found. Please ensure it exists."
    exit 1
fi

echo ""
echo "📋 Step 4: Setting up Redis..."
echo "⚠️  Make sure Redis is running before continuing..."
read -p "Press Enter to continue with Redis setup..."

if [ -f "scripts/setup_redis.sh" ]; then
    ./scripts/setup_redis.sh
else
    echo "❌ setup_redis.sh not found. Please ensure it exists."
    exit 1
fi

echo ""
echo "📋 Step 5: Building the project..."
cargo build

echo ""
echo "📋 Step 6: Setting up test data..."
if [ -f "scripts/setup_test_data.sh" ]; then
    ./scripts/setup_test_data.sh
else
    echo "❌ setup_test_data.sh not found. Skipping test data setup."
fi

echo ""
echo "🎉 Setup complete!"
echo "=================================================="
echo "✅ JWT keys generated"
echo "✅ Database configured and migrated"
echo "✅ Redis configured"
echo "✅ Project built"
echo ""
echo "🚀 Ready to start the server!"
echo ""
echo "To start the server, run:"
echo "  cargo run --bin gateway"
echo ""
echo "The server will be available at: http://localhost:8081"
echo ""
echo "📝 Test endpoints:"
echo "  POST /signup - User registration"
echo "  POST /login - User login"
echo "  GET /profile - Get user profile (requires JWT)"
echo "  POST /api-keys - Create API key (requires JWT)"
echo "  GET /api-keys - List API keys (requires JWT)"
echo ""
echo "📚 See test_scripts/ directory for manual testing examples"
