#!/bin/bash

# Database setup script for local development
# This script sets up PostgreSQL database and runs migrations

set -e

# Database configuration (from development.toml)
DB_NAME="trading_db"
DB_USER="postgres"
DB_PASSWORD="postgres123"
DB_HOST="localhost"
DB_PORT="5432"

echo "Setting up PostgreSQL database for development..."

# Set password for PostgreSQL connection
export PGPASSWORD="$DB_PASSWORD"

# Check if PostgreSQL is running
if ! pg_isready -h $DB_HOST -p $DB_PORT > /dev/null 2>&1; then
    echo "❌ PostgreSQL is not running. Please start PostgreSQL first."
    echo "   On macOS: brew services start postgresql"
    echo "   On Ubuntu: sudo systemctl start postgresql"
    exit 1
fi

echo "✅ PostgreSQL is running"

# Create database if it doesn't exist
echo "Creating database '$DB_NAME'..."
createdb -h $DB_HOST -p $DB_PORT -U $DB_USER $DB_NAME 2>/dev/null || echo "Database already exists"

echo "✅ Database '$DB_NAME' is ready"

# Run migrations
echo "Running database migrations..."

# Migration 1: Create users table
echo "Running migration 001_create_users_table.sql..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -f migrations/001_create_users_table.sql

# Migration 2: Create API keys table
echo "Running migration 002_create_api_keys_table.sql..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -f migrations/002_create_api_keys_table.sql

# Migration 3: Create trading tables
echo "Running migration 003_create_trading_tables.sql..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -f migrations/003_create_trading_tables.sql

echo "✅ All migrations completed successfully!"

# Test database connection
echo "Testing database connection..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "SELECT version();" > /dev/null

echo "✅ Database setup complete!"
echo "📊 Database: $DB_NAME"
echo "👤 User: $DB_USER"
echo "🌐 Host: $DB_HOST:$DB_PORT"
