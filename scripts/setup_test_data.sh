#!/bin/bash

# Setup test data for local development
# This script creates test users and API keys for manual testing

set -e

# Database configuration (from development.toml)
DB_NAME="trading_db"
DB_USER="postgres"
DB_PASSWORD="postgres123"
DB_HOST="localhost"
DB_PORT="5432"

echo "📊 Setting up test data for development..."
echo "=========================================="

# Set password for PostgreSQL connection
export PGPASSWORD="$DB_PASSWORD"

# Check if database is accessible
if ! psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "SELECT 1;" > /dev/null 2>&1; then
    echo "❌ Cannot connect to database. Please ensure PostgreSQL is running and configured."
    echo "   Expected database: $DB_NAME"
    echo "   Expected user: $DB_USER"
    echo "   Expected host:port: $DB_HOST:$DB_PORT"
    exit 1
fi

echo "✅ Database connection verified"

# Create test user (with hashed password)
echo "👤 Creating/updating test user..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Create test user with hashed password (password123)
-- If user exists, update to verified and active status
INSERT INTO users (
    user_id,
    email,
    username,
    password_hash,
    auth_provider,
    email_verified,
    status,
    created_at,
    updated_at
) VALUES (
    gen_random_uuid(),
    'test@example.com',
    'testuser',
    '\$2b\$12\$5FfIk3sg7O8gsqUoqxr9sONzDEENq2al6ra.DwjVRpTHutDCb1Ukq', -- hash for password123
    'email',
    true,
    'active',
    NOW(),
    NOW()
) ON CONFLICT (email) DO UPDATE SET
    email_verified = true,
    status = 'active',
    password_hash = EXCLUDED.password_hash,
    updated_at = NOW();
EOF

echo "✅ Test user ready (email: test@example.com, password: password123, verified: true, status: active)"

# Create test API key
echo "🔑 Creating/updating test API key..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Create test API key (update if exists)
INSERT INTO api_keys (
    id,
    api_key,
    key_material_type,
    key_material_value,
    permissions,
    recv_window,
    enabled,
    description,
    ip_blacklist,
    user_id,
    created_at,
    updated_at
) VALUES (
    gen_random_uuid(),
    'test_api_key_12345',
    'hmac_secret',
    'test_secret_key_67890',
    ARRAY['trade', 'user_data']::security_type[],
    5000,
    true,
    'Test API Key for Development',
    ARRAY[]::text[],
    (SELECT user_id FROM users WHERE email = 'test@example.com'),
    NOW(),
    NOW()
) ON CONFLICT (api_key) DO UPDATE SET
    enabled = true,
    key_material_value = EXCLUDED.key_material_value,
    permissions = EXCLUDED.permissions,
    user_id = (SELECT user_id FROM users WHERE email = 'test@example.com'),
    updated_at = NOW();
EOF

echo "✅ Test API key ready (key: test_api_key_12345, secret: test_secret_key_67890, enabled: true)"

# Create test user balance in trading schema
echo "💰 Creating test user balance..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Insert balance event for test user
-- Note: trading.balances_events uses BIGINT user_id (not UUID from users table)
INSERT INTO trading.balances_events (
    user_id,
    asset_id,
    available,
    locked,
    total,
    timestamp
) VALUES (
    1,  -- test user_id (BIGINT, separate from UUID in users table)
    1,  -- asset_id 1 (base currency)
    1000000000000,  -- 1M with 6 decimal precision
    0,              -- 0 locked
    1000000000000,  -- 1M total
    EXTRACT(EPOCH FROM NOW())::bigint * 1000  -- current timestamp in milliseconds
) ON CONFLICT DO NOTHING;
EOF

echo "✅ Test user balance created (1,000,000 available for asset_id 1)"

echo ""
echo "🎉 Test data setup complete!"
echo "=========================================="
echo "📝 Test Credentials:"
echo "   Email: test@example.com"
echo "   Password: password123"
echo "   Username: testuser"
echo ""
echo "🔑 Test API Key:"
echo "   API Key: test_api_key_12345"
echo "   Secret: test_secret_key_67890"
echo "   Permissions: trade, user_data"
echo ""
echo "💰 Test Balance:"
echo "   User ID: 1"
echo "   Asset ID: 1"
echo "   Available: 1,000,000"
echo "   Locked: 0"
echo ""
echo "🧪 Ready for testing!"
echo "   Run: ./test_scripts/test_auth.sh"
echo "   Run: ./test_scripts/test_api_key.sh"
