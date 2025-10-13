#!/bin/bash

# Redis setup script for local development
# This script checks Redis connection and sets up basic configuration

set -e

# Redis configuration
REDIS_HOST="127.0.0.1"
REDIS_PORT="6379"

echo "Setting up Redis for development..."

# Check if Redis is running
if ! redis-cli -h $REDIS_HOST -p $REDIS_PORT ping > /dev/null 2>&1; then
    echo "❌ Redis is not running. Please start Redis first."
    echo "   On macOS: brew services start redis"
    echo "   On Ubuntu: sudo systemctl start redis"
    echo "   Or run: redis-server"
    exit 1
fi

echo "✅ Redis is running"

# Test Redis connection
echo "Testing Redis connection..."
redis-cli -h $REDIS_HOST -p $REDIS_PORT ping

# Clear any existing test data
echo "Clearing existing test data..."
redis-cli -h $REDIS_HOST -p $REDIS_PORT flushdb

echo "✅ Redis setup complete!"
echo "🌐 Redis: $REDIS_HOST:$REDIS_PORT"
echo "🗑️  Cleared existing data for clean testing"
