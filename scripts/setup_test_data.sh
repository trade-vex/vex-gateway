#!/bin/bash

# Setup test data for local development
# This script creates test users and API keys for manual testing

set -e

# Database configuration
DB_NAME="vex_gateway"
DB_USER="vex"
DB_HOST="localhost"
DB_PORT="5432"

echo "📊 Setting up test data for development..."
echo "=========================================="

# Check if database is accessible
if ! psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME -c "SELECT 1;" > /dev/null 2>&1; then
    echo "❌ Cannot connect to database. Please ensure PostgreSQL is running and configured."
    exit 1
fi

echo "✅ Database connection verified"

# Create test user (with hashed password)
echo "👤 Creating test user..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Create test user with hashed password (password123)
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
) ON CONFLICT (email) DO NOTHING;
EOF

echo "✅ Test user created (email: test@example.com, password: password123)"

# Create test API key
echo "🔑 Creating test API key..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Create test API key
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
) ON CONFLICT (api_key) DO NOTHING;
EOF

echo "✅ Test API key created (key: test_api_key_12345, secret: test_secret_key_67890)"

# Create test user balance
echo "💰 Creating test user balance..."
psql -h $DB_HOST -p $DB_PORT -U $DB_USER -d $DB_NAME << EOF
-- Create test user balance
INSERT INTO user_balances (
    user_id,
    available,
    locked,
    last_updated
) VALUES (
    (SELECT user_id FROM users WHERE email = 'test@example.com'),
    1000000, -- 1M available
    0,       -- 0 locked
    NOW()
) ON CONFLICT (user_id) DO UPDATE SET
    available = 1000000,
    locked = 0,
    last_updated = NOW();
EOF

echo "✅ Test user balance created (1,000,000 available)"

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
echo "   Available: 1,000,000"
echo "   Locked: 0"
echo ""
echo "🧪 Ready for testing!"
echo "   Run: ./test_scripts/test_auth.sh"
echo "   Run: ./test_scripts/test_api_key.sh"
