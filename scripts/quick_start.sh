#!/bin/bash

# Quick start guide for VEX Gateway local testing
# This script provides a summary of all available commands

echo "🚀 VEX Gateway Local Testing - Quick Start"
echo "=========================================="
echo ""

echo "📋 Setup Commands:"
echo "  ./scripts/setup_dev.sh        - Complete setup (run this first)"
echo "  ./scripts/generate_keys.sh    - Generate JWT keys only"
echo "  ./scripts/setup_database.sh   - Setup database only"
echo "  ./scripts/setup_redis.sh      - Setup Redis only"
echo "  ./scripts/setup_test_data.sh  - Add test data only"
echo ""

echo "🚀 Server Commands:"
echo "  ./scripts/start_server.sh     - Start the server"
echo "  APP_ENV=development cargo run --bin gateway - Start server directly"
echo ""

echo "🧪 Testing Commands:"
echo "  ./test_scripts/test_auth.sh      - Test JWT authentication"
echo "  ./test_scripts/test_api_key.sh    - Test API key authentication"
echo ""

echo "📝 Manual Testing Examples:"
echo ""
echo "1. User Signup:"
echo "   curl -X POST http://localhost:8081/signup \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{\"email\":\"test@example.com\",\"username\":\"testuser\",\"password\":\"password123\"}'"
echo ""

echo "2. User Login:"
echo "   curl -X POST http://localhost:8081/login \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{\"email\":\"test@example.com\",\"password\":\"password123\"}'"
echo ""

echo "3. Get Profile (JWT required):"
echo "   curl -X GET http://localhost:8081/profile \\"
echo "     -H 'Authorization: Bearer YOUR_JWT_TOKEN'"
echo ""

echo "4. API Key Request (API key required):"
echo "   # For every request, sign: ENC(method) LF ENC(path) LF SORTED_QUERY LF HEX(body)"
echo "   # Empty query/body still contribute empty components; duplicate query/form names are rejected"
echo "   curl -X GET 'http://localhost:8081/api/balances?timestamp=<timestamp>&signature=<hex-signature>' \\"
echo "     -H 'X-MBX-APIKEY: <your-api-key>'"
echo ""

echo "🔑 Test Credentials (after setup):"
echo "   Email: test@example.com"
echo "   Password: password123"
echo "   API Key: test_api_key_12345"
echo "   Secret: test_secret_key_67890"
echo ""

echo "📚 Documentation:"
echo "   TESTING_SETUP.md - Complete setup guide"
echo "   README.md - Project documentation"
echo ""

echo "🆘 Troubleshooting:"
echo "   - Ensure PostgreSQL is running: pg_isready"
echo "   - Ensure Redis is running: redis-cli ping"
echo "   - Check server logs for errors"
echo "   - Verify .env file exists"
echo "   - Verify JWT keys exist in crates/libs/lib-auth/src/keys/"
echo ""

echo "🎯 Quick Start:"
echo "   1. Run: ./scripts/setup_dev.sh"
echo "   2. Run: ./scripts/start_server.sh"
echo "   3. Run: ./test_scripts/test_auth.sh"
echo "   4. Test manually with curl commands above"
echo ""

echo "✅ Ready to test authentication!"
