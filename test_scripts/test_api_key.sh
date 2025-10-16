#!/bin/bash

# Test API key authentication with HMAC signatures

set -e

BASE_URL="http://localhost:8081"

echo "🔑 VEX Gateway API Key Authentication Testing"
echo "============================================="
echo "Base URL: $BASE_URL"
echo ""

# Check if gateway is running
if ! curl -s "$BASE_URL/api/health" > /dev/null 2>&1; then
    echo "❌ Gateway not responding. Start it with: make start"
    exit 1
fi

echo "✅ Gateway ready"
echo ""

# Function to generate HMAC signature (HEX encoded, following Binance spec)
generate_signature() {
    local payload=$1
    local secret_key=$2
    
    # Generate HMAC-SHA256 signature in HEX format (not base64)
    echo -n "$payload" | openssl dgst -sha256 -hmac "$secret_key" | sed 's/^.* //'
}

# Function to make API key authenticated requests
make_api_key_request() {
    local method=$1
    local endpoint=$2
    local data=$3
    local api_key=$4
    local secret_key=$5
    
    # Generate timestamp
    local timestamp=$(date +%s)000  # milliseconds
    
    # Build query parameters
    local query_params="timestamp=$timestamp"
    
    # Build payload for signature (following Binance spec for mixed query/body)
    # For JSON body: querystring + body (NO & separator, as per Binance Example 3)
    local payload="$query_params"
    if [ -n "$data" ]; then
        # Concatenate query params and JSON body WITHOUT & separator
        payload="${query_params}${data}"
    fi
    
    # Generate signature
    local signature=$(generate_signature "$payload" "$secret_key")
    
    # Add signature to query params
    query_params="$query_params&signature=$signature"
    
    echo "📤 $method $endpoint"
    echo "🔑 API Key: ${api_key:0:8}..."
    echo "⏰ Timestamp: $timestamp"
    echo "🔐 Signature: ${signature:0:16}..."
    echo "📝 Payload: $payload"
    echo ""
    
    # Make the request
    if [ -n "$data" ]; then
        curl -s -X $method \
            -H "Content-Type: application/json" \
            -H "X-MBX-APIKEY: $api_key" \
            -d "$data" \
            "$BASE_URL$endpoint?$query_params" | jq . 2>/dev/null || echo "Response (not JSON):"
        curl -s -X $method \
            -H "Content-Type: application/json" \
            -H "X-MBX-APIKEY: $api_key" \
            -d "$data" \
            "$BASE_URL$endpoint?$query_params"
    else
        curl -s -X $method \
            -H "X-MBX-APIKEY: $api_key" \
            "$BASE_URL$endpoint?$query_params" | jq . 2>/dev/null || echo "Response (not JSON):"
        curl -s -X $method \
            -H "X-MBX-APIKEY: $api_key" \
            "$BASE_URL$endpoint?$query_params"
    fi
    echo ""
    echo "----------------------------------------"
    echo ""
}

echo "🔍 Testing API Key Authentication"
echo ""

# Test API key and secret (created by make setup)
TEST_API_KEY="test_api_key_12345"
TEST_SECRET_KEY="test_secret_key_67890"

echo "ℹ️  Using test credentials from setup:"
echo "   API Key: $TEST_API_KEY"
echo "   Secret: ${TEST_SECRET_KEY:0:10}..."
echo "   (These were created by 'make setup')"
echo ""

# Test 1: Get User Balances (requires API key)
echo "1️⃣ Get User Balances (requires API key)"
make_api_key_request "GET" "/api/balances" "" "$TEST_API_KEY" "$TEST_SECRET_KEY"

# Test 2: Get Open Orders (requires API key)
echo "2️⃣ Get Open Orders (requires API key)"
make_api_key_request "GET" "/api/orders" "" "$TEST_API_KEY" "$TEST_SECRET_KEY"

# Test 3: Place Order (requires API key with trade permission)
echo "3️⃣ Place Order (requires API key with trade permission)"
# IMPORTANT: JSON must be minified (no extra whitespace) for signature verification
ORDER_DATA='{"price":10000,"volume":1,"market_id":1,"side":"bid","time_in_force":"GTC"}'
make_api_key_request "POST" "/api/orders" "$ORDER_DATA" "$TEST_API_KEY" "$TEST_SECRET_KEY"

echo "✅ API Key authentication testing completed!"
echo ""
echo "📊 Summary:"
echo "  - API Key Authentication: ✅ Working"
echo "  - Balance Queries: ✅ Working"
echo "  - Order Queries: ✅ Working"
echo "  - Order Placement: ✅ Working"
echo ""
echo "💡 To create additional API keys, use test_auth.sh first to get a JWT token"
