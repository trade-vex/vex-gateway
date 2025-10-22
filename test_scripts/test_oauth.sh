#!/bin/bash

# Test script for OAuth flow
# Usage: ./test_scripts/test_oauth.sh [provider]
# Example: ./test_scripts/test_oauth.sh google

set -e

BASE_URL="${BASE_URL:-http://localhost:8081}"
API_BASE="$BASE_URL/api/v1"
PROVIDER="${1:-google}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_header() {
    echo -e "\n${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}\n"
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

print_info() {
    echo -e "${YELLOW}ℹ $1${NC}"
}

print_header "VEX Gateway - OAuth Flow Test"
print_info "Provider: $PROVIDER"

# Step 1: Get available providers
print_header "Step 1: Get Available OAuth Providers"

PROVIDERS_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_BASE/oauth/providers")

HTTP_CODE=$(echo "$PROVIDERS_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$PROVIDERS_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Providers fetched successfully!"
    echo "$RESPONSE_BODY" | jq '.'
    
    # Check if provider is available
    AVAILABLE=$(echo "$RESPONSE_BODY" | jq -r ".providers[] | select(. == \"$PROVIDER\")")
    if [ -z "$AVAILABLE" ]; then
        print_error "Provider '$PROVIDER' is not configured!"
        print_info "Available providers:"
        echo "$RESPONSE_BODY" | jq -r '.providers[]'
        exit 1
    fi
    print_success "Provider '$PROVIDER' is available!"
else
    print_error "Failed to fetch providers with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
    exit 1
fi

# Step 2: Get authorization URL
print_header "Step 2: Get Authorization URL"

AUTH_URL_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_BASE/oauth/$PROVIDER" \
  -H "Accept: text/plain")

HTTP_CODE=$(echo "$AUTH_URL_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$AUTH_URL_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "302" ] || [ "$HTTP_CODE" = "301" ]; then
    print_success "Authorization URL retrieved!"
    print_info "You will be redirected to the OAuth provider"
else
    print_error "Failed to get authorization URL with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY"
    exit 1
fi

# Open browser for OAuth flow
print_header "Step 3: OAuth Flow - Browser Interaction Required"
print_info "Opening OAuth flow in browser..."
print_info "After authorizing, you'll be redirected to: $BASE_URL/auth/$PROVIDER/callback"

# Try to open in browser (macOS/Linux)
if command -v open &> /dev/null; then
    open "$API_BASE/oauth/$PROVIDER"
elif command -v xdg-open &> /dev/null; then
    xdg-open "$API_BASE/oauth/$PROVIDER"
else
    print_info "Please open this URL in your browser:"
    echo "$API_BASE/oauth/$PROVIDER"
fi

print_header "OAuth Flow Instructions"
print_info "1. Complete the OAuth authorization in your browser"
print_info "2. After redirect, you'll see a JSON response with tokens"
print_info "3. Copy the access_token from the response"
print_info ""

read -p "Paste the access_token here: " ACCESS_TOKEN

if [ -z "$ACCESS_TOKEN" ]; then
    print_error "Access token is required to continue"
    exit 1
fi

# Step 4: Test authenticated endpoint with OAuth token
print_header "Step 4: Testing Authenticated Endpoint"

PROFILE_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_BASE/auth/profile" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

HTTP_CODE=$(echo "$PROFILE_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$PROFILE_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Profile retrieved successfully!"
    echo "$RESPONSE_BODY" | jq '.'
    
    AUTH_PROVIDER=$(echo "$RESPONSE_BODY" | jq -r '.auth_provider')
    print_success "Auth provider: $AUTH_PROVIDER"
    
    if [ "$AUTH_PROVIDER" = "$PROVIDER" ]; then
        print_success "Correct OAuth provider in user profile!"
    else
        print_error "Expected provider '$PROVIDER', got '$AUTH_PROVIDER'"
    fi
else
    print_error "Profile retrieval failed with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
    exit 1
fi

# Step 5: Test OAuth logout
print_header "Step 5: Testing OAuth Logout"

LOGOUT_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_BASE/auth/logout" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

HTTP_CODE=$(echo "$LOGOUT_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$LOGOUT_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Logout successful!"
    echo "$RESPONSE_BODY" | jq '.'
else
    print_error "Logout failed with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
fi

print_header "✓ OAuth Flow Test Complete!"
print_info "Provider: $PROVIDER"
print_info "Test passed successfully!"

