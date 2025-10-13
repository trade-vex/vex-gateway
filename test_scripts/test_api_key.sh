#!/bin/bash

# Manual testing script for API key authentication
# This script demonstrates how to test API key authentication with signatures

set -e

BASE_URL="http://localhost:8081"

echo "🔑 VEX Gateway API Key Authentication Testing"
echo "============================================="
echo "Base URL: $BASE_URL"
echo ""

# Function to generate HMAC signature
generate_signature() {
    local payload=$1
    local secret_key=$2
    
    echo -n "$payload" | openssl dgst -sha256 -hmac "$secret_key" -binary | base64
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
    
    # Build payload for signature
    local payload="$query_params"
    if [ -n "$data" ]; then
        payload="$payload&$data"
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

# Example API key and secret (these would come from your API key creation)
EXAMPLE_API_KEY="test_api_key_12345"
EXAMPLE_SECRET_KEY="test_secret_key_67890"

echo "⚠️  Note: Replace EXAMPLE_API_KEY and EXAMPLE_SECRET_KEY with actual values"
echo "   from your API key creation response"
echo ""

# Test 1: Get User Balances (requires API key)
echo "1️⃣ Get User Balances (requires API key)"
make_api_key_request "GET" "/balances" "" "$EXAMPLE_API_KEY" "$EXAMPLE_SECRET_KEY"

# Test 2: Get Open Orders (requires API key)
echo "2️⃣ Get Open Orders (requires API key)"
make_api_key_request "GET" "/orders" "" "$EXAMPLE_API_KEY" "$EXAMPLE_SECRET_KEY"

# Test 3: Place Order (requires API key with trade permission)
echo "3️⃣ Place Order (requires API key with trade permission)"
ORDER_DATA='{
    "price": 10000,
    "volume": 1,
    "market_id": 1,
    "side": "bid",
    "time_in_force": "GTC"
}'
make_api_key_request "POST" "/orders" "$ORDER_DATA" "$EXAMPLE_API_KEY" "$EXAMPLE_SECRET_KEY"

echo "✅ API Key authentication testing examples completed!"
echo ""
echo "📝 Next Steps:"
echo "1. Create an API key using JWT authentication"
echo "2. Copy the API key and secret from the response"
echo "3. Replace EXAMPLE_API_KEY and EXAMPLE_SECRET_KEY in this script"
echo "4. Re-run this script to test API key authentication"
echo ""
echo "🔧 API Key Creation Process:"
echo "1. Login with JWT to get access token"
echo "2. POST /api-keys with JWT token to create API key"
echo "3. Use the returned API key and secret for authenticated requests"
echo ""
echo "📚 Signature Generation:"
echo "1. Create query string with timestamp"
echo "2. Add request body if present"
echo "3. Generate HMAC-SHA256 signature with secret key"
echo "4. Add signature to query parameters"
echo "5. Include X-MBX-APIKEY header"
