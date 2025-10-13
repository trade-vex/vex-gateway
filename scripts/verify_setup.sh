#!/bin/bash

# Verification script for VEX Gateway setup
# This script checks if all components are properly configured

set -e

echo "🔍 VEX Gateway Setup Verification"
echo "=================================="
echo ""

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Not in project root directory"
    exit 1
fi
echo "✅ In project root directory"

# Check .env file
if [ -f ".env" ]; then
    echo "✅ .env file exists"
else
    echo "❌ .env file missing - run: cp env.example .env"
fi

# Check JWT keys
if [ -f "crates/libs/lib-auth/src/keys/private_key.pem" ] && [ -f "crates/libs/lib-auth/src/keys/public_key.pem" ]; then
    echo "✅ JWT keys exist"
else
    echo "❌ JWT keys missing - run: ./scripts/generate_keys.sh"
fi

# Check PostgreSQL
if pg_isready -h localhost -p 5432 > /dev/null 2>&1; then
    echo "✅ PostgreSQL is running"
    
    # Check if database exists
    if psql -h localhost -p 5432 -U vex -d vex_gateway -c "SELECT 1;" > /dev/null 2>&1; then
        echo "✅ Database connection successful"
        
        # Check if tables exist
        if psql -h localhost -p 5432 -U vex -d vex_gateway -c "SELECT COUNT(*) FROM users;" > /dev/null 2>&1; then
            echo "✅ Database tables exist"
        else
            echo "❌ Database tables missing - run: ./scripts/setup_database.sh"
        fi
    else
        echo "❌ Database connection failed - check credentials"
    fi
else
    echo "❌ PostgreSQL not running - start with: brew services start postgresql"
fi

# Check Redis
if redis-cli -h 127.0.0.1 -p 6379 ping > /dev/null 2>&1; then
    echo "✅ Redis is running"
else
    echo "❌ Redis not running - start with: brew services start redis"
fi

# Check if project builds
if cargo check > /dev/null 2>&1; then
    echo "✅ Project compiles successfully"
else
    echo "❌ Project compilation failed - run: cargo build"
fi

# Check test data
if psql -h localhost -p 5432 -U vex -d vex_gateway -c "SELECT COUNT(*) FROM users WHERE email = 'test@example.com';" 2>/dev/null | grep -q "1"; then
    echo "✅ Test data exists"
else
    echo "❌ Test data missing - run: ./scripts/setup_test_data.sh"
fi

echo ""
echo "🎯 Setup Status Summary"
echo "======================"

# Count successful checks
success_count=0
total_checks=6

if [ -f ".env" ]; then ((success_count++)); fi
if [ -f "crates/libs/lib-auth/src/keys/private_key.pem" ] && [ -f "crates/libs/lib-auth/src/keys/public_key.pem" ]; then ((success_count++)); fi
if pg_isready -h localhost -p 5432 > /dev/null 2>&1; then ((success_count++)); fi
if redis-cli -h 127.0.0.1 -p 6379 ping > /dev/null 2>&1; then ((success_count++)); fi
if cargo check > /dev/null 2>&1; then ((success_count++)); fi
if psql -h localhost -p 5432 -U vex -d vex_gateway -c "SELECT COUNT(*) FROM users WHERE email = 'test@example.com';" 2>/dev/null | grep -q "1"; then ((success_count++)); fi

echo "✅ Successful checks: $success_count/$total_checks"

if [ $success_count -eq $total_checks ]; then
    echo ""
    echo "🎉 Setup is complete! Ready to start testing."
    echo ""
    echo "Next steps:"
    echo "  1. Start server: make start"
    echo "  2. Run tests: make test-all"
    echo "  3. Manual testing: ./test_scripts/test_auth.sh"
else
    echo ""
    echo "⚠️  Setup incomplete. Please fix the issues above."
    echo ""
    echo "Quick fix:"
    echo "  make setup"
fi
