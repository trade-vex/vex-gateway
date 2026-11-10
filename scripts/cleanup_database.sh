#!/bin/bash

# Cleanup script for VEX Gateway database
# This script drops the database for a fresh start; scripts/setup_database.sh recreates it

set -e

# Database configuration (from development.toml)
DB_NAME="trading_db"
DB_USER="postgres"
DB_PASSWORD="postgres123"
DB_HOST="localhost"
DB_PORT="5432"

echo "🧹 VEX Gateway Database Cleanup"
echo "================================"
echo "Database: $DB_NAME"
echo "User: $DB_USER"
echo "Host: $DB_HOST:$DB_PORT"
echo ""

# Set password for PostgreSQL connection
export PGPASSWORD="$DB_PASSWORD"

# Confirm before proceeding
read -p "⚠️  This will DELETE ALL DATA. Are you sure? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Cleanup cancelled"
    exit 1
fi

echo "🗑️  Starting database cleanup..."

# Drop the database: its tables, the read model's trading schema and golang-migrate's version
echo "🗑️  Dropping database '$DB_NAME'..."
dropdb -h $DB_HOST -p $DB_PORT -U $DB_USER --if-exists --force $DB_NAME

echo "✅ Database dropped"

echo ""
echo "🎉 Database cleanup complete!"
echo "================================"
echo "📝 Next steps:"
echo "   Run: ./scripts/setup_database.sh   # Database, migrations and seeds"
echo "   Run: ./scripts/setup_test_data.sh  # Test data"
echo ""
echo "🚀 Ready for fresh setup!"
