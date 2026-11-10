#!/bin/bash

# Database setup script for local development
# Creates the database, applies the read model's migrations (readmodel/migrations/postgres) with
# golang-migrate (https://github.com/golang-migrate/migrate) and seeds markets and assets from
# vex-core's development configuration (readmodel/seeds/development.sql).

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
if ! command -v migrate > /dev/null; then
    echo "❌ golang-migrate is not installed: https://github.com/golang-migrate/migrate/tree/master/cmd/migrate"
    exit 1
fi
echo "Running database migrations..."
migrate -path readmodel/migrations/postgres \
    -database "postgres://$DB_USER:$DB_PASSWORD@$DB_HOST:$DB_PORT/$DB_NAME?sslmode=disable" up

echo "Seeding markets and assets..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -v ON_ERROR_STOP=1 -q -f readmodel/seeds/development.sql

echo "✅ All migrations completed successfully!"

# Test database connection
echo "Testing database connection..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "SELECT version();" > /dev/null

echo "✅ Database setup complete!"
echo "📊 Database: $DB_NAME"
echo "👤 User: $DB_USER"
echo "🌐 Host: $DB_HOST:$DB_PORT"
