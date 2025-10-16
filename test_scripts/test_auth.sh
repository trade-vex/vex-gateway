#!/bin/bash

# Test JWT authentication endpoints

set -e

BASE_URL="http://localhost:8081"

echo "🧪 VEX Gateway Authentication Testing"
echo "====================================="
echo "Base URL: $BASE_URL"
echo ""

# Check if gateway is running
if ! curl -s "$BASE_URL/health" > /dev/null 2>&1; then
    echo "❌ Gateway not responding. Start it with: make start"
    exit 1
fi

echo "✅ Gateway ready"
echo ""

# Function to make HTTP requests with pretty output
make_request() {
    local method=$1
    local endpoint=$2
    local data=$3
    local headers=$4
    
    echo "📤 $method $endpoint"
    if [ -n "$data" ]; then
        echo "📝 Data: $data"
    fi
    if [ -n "$headers" ]; then
        echo "📋 Headers: $headers"
    fi
    echo ""
    
    if [ -n "$data" ]; then
        curl -s -X $method \
            -H "Content-Type: application/json" \
            $headers \
            -d "$data" \
            "$BASE_URL$endpoint" | jq . 2>/dev/null || echo "Response (not JSON):"
        curl -s -X $method \
            -H "Content-Type: application/json" \
            $headers \
            -d "$data" \
            "$BASE_URL$endpoint"
    else
        curl -s -X $method \
            $headers \
            "$BASE_URL$endpoint" | jq . 2>/dev/null || echo "Response (not JSON):"
        curl -s -X $method \
            $headers \
            "$BASE_URL$endpoint"
    fi
    echo ""
    echo "----------------------------------------"
    echo ""
}

echo "🔍 Testing Authentication Endpoints"
echo ""

# Test 1: Health Check
echo "1️⃣ Health Check"
make_request "GET" "/health"

# Test 2: User Signup
echo "2️⃣ User Signup"
SIGNUP_DATA='{
    "email": "test@example.com",
    "username": "testuser",
    "password": "password123"
}'
make_request "POST" "/signup" "$SIGNUP_DATA"

# Test 3: User Login
echo "3️⃣ User Login"
LOGIN_DATA='{
    "email": "test@example.com",
    "password": "password123"
}'
LOGIN_RESPONSE=$(make_request "POST" "/login" "$LOGIN_DATA")

# Extract JWT token from login response (this is a simplified approach)
echo "🔑 Extracting JWT token from login response..."
echo "Note: In a real test, you would parse the JSON response to extract the access_token"
echo ""

# Test 4: Get User Profile (requires JWT)
echo "4️⃣ Get User Profile (requires JWT)"
echo "⚠️  This will fail without a valid JWT token"
echo "Replace 'YOUR_JWT_TOKEN' with the actual token from login response"
make_request "GET" "/profile" "" "-H 'Authorization: Bearer YOUR_JWT_TOKEN'"

# Test 5: Create API Key (requires JWT)
echo "5️⃣ Create API Key (requires JWT)"
echo "⚠️  This will fail without a valid JWT token"
API_KEY_DATA='{
    "description": "Test API Key",
    "permissions": ["trade", "user_data"],
    "recv_window": 5000
}'
make_request "POST" "/api-keys" "$API_KEY_DATA" "-H 'Authorization: Bearer YOUR_JWT_TOKEN'"

# Test 6: List API Keys (requires JWT)
echo "6️⃣ List API Keys (requires JWT)"
echo "⚠️  This will fail without a valid JWT token"
make_request "GET" "/api-keys" "" "-H 'Authorization: Bearer YOUR_JWT_TOKEN'"

echo "✅ Authentication testing completed!"
echo ""
echo "📝 Next Steps:"
echo "1. Copy the JWT token from login response"
echo "2. Replace 'YOUR_JWT_TOKEN' with actual token"
echo "3. Re-run protected endpoint tests"
echo ""
echo "For API Key testing: ./test_scripts/test_api_key.sh"
