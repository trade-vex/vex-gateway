#!/bin/bash

# Cleanup script for VEX Gateway database
# This script removes all tables and data for a fresh start

set -e

# Database configuration
DB_NAME="vex_gateway"
DB_USER="vex"
DB_HOST="localhost"
DB_PORT="5432"

echo "🧹 VEX Gateway Database Cleanup"
echo "================================"
echo "Database: $DB_NAME"
echo "User: $DB_USER"
echo "Host: $DB_HOST:$DB_PORT"
echo ""

# Confirm before proceeding
read -p "⚠️  This will DELETE ALL DATA. Are you sure? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Cleanup cancelled"
    exit 1
fi

echo "🗑️  Starting database cleanup..."

# Check if database is accessible
if ! psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "SELECT 1;" > /dev/null 2>&1; then
    echo "❌ Cannot connect to database. Please ensure PostgreSQL is running and configured."
    exit 1
fi

echo "✅ Database connection verified"

# Drop all tables and types
echo "🗑️  Dropping all tables and types..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Drop tables in reverse dependency order
DROP TABLE IF EXISTS api_key_usage_logs CASCADE;
DROP TABLE IF EXISTS orders CASCADE;
DROP TABLE IF EXISTS user_balances CASCADE;
DROP TABLE IF EXISTS api_keys CASCADE;
DROP TABLE IF EXISTS users CASCADE;

-- Drop custom types
DROP TYPE IF EXISTS user_status CASCADE;
DROP TYPE IF EXISTS security_type CASCADE;
DROP TYPE IF EXISTS key_material_type CASCADE;
DROP TYPE IF EXISTS order_status CASCADE;
DROP TYPE IF EXISTS order_side CASCADE;
DROP TYPE IF EXISTS time_in_force CASCADE;

-- Drop extensions (optional - comment out if you want to keep them)
-- DROP EXTENSION IF EXISTS pgcrypto CASCADE;
-- DROP EXTENSION IF EXISTS citext CASCADE;
EOF

echo "✅ All tables and types dropped"

# Verify cleanup
echo "🔍 Verifying cleanup..."
TABLE_COUNT=$(psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -t -c "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public';" | tr -d ' ')

if [ "$TABLE_COUNT" = "0" ]; then
    echo "✅ Database is completely clean (0 tables remaining)"
else
    echo "⚠️  Warning: $TABLE_COUNT tables still exist"
    echo "Remaining tables:"
    psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "\dt"
fi

echo ""
echo "🎉 Database cleanup complete!"
echo "================================"
echo "📝 Next steps:"
echo "   Run: make setup-db    # Setup database schema"
echo "   Run: make setup-data  # Setup test data"
echo "   Run: make setup       # Full setup"
echo ""
echo "🚀 Ready for fresh setup!"
