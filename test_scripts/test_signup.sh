#!/bin/bash

# Test script for signup and email verification flow
# Usage: ./test_scripts/test_signup.sh

set -e

BASE_URL="${BASE_URL:-http://localhost:8081}"
API_BASE="$BASE_URL/api/v1"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
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

# Generate random test user
TIMESTAMP=$(date +%s)
TEST_EMAIL="test+$TIMESTAMP@example.com"
TEST_USERNAME="testuser_$TIMESTAMP"
TEST_PASSWORD="TestPass123!"

print_header "VEX Gateway - Signup Flow Test"

# Step 1: Test signup
print_header "Step 1: Testing Signup"
print_info "Creating user: $TEST_USERNAME ($TEST_EMAIL)"

SIGNUP_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_BASE/auth/signup" \
  -H "Content-Type: application/json" \
  -d "{
    \"email\": \"$TEST_EMAIL\",
    \"username\": \"$TEST_USERNAME\",
    \"password\": \"$TEST_PASSWORD\"
  }")

HTTP_CODE=$(echo "$SIGNUP_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$SIGNUP_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "201" ]; then
    print_success "User created successfully!"
    echo "$RESPONSE_BODY" | jq '.'
    USER_ID=$(echo "$RESPONSE_BODY" | jq -r '.user.user_id')
    print_info "User ID: $USER_ID"
else
    print_error "Signup failed with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
    exit 1
fi

# Step 2: Test duplicate signup
print_header "Step 2: Testing Duplicate Signup Prevention"

DUPLICATE_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_BASE/auth/signup" \
  -H "Content-Type: application/json" \
  -d "{
    \"email\": \"$TEST_EMAIL\",
    \"username\": \"$TEST_USERNAME\",
    \"password\": \"$TEST_PASSWORD\"
  }")

HTTP_CODE=$(echo "$DUPLICATE_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$DUPLICATE_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "409" ]; then
    print_success "Duplicate email correctly rejected!"
    echo "$RESPONSE_BODY" | jq '.'
else
    print_error "Expected 409 Conflict, got HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
fi

# Step 3: Test login before verification
print_header "Step 3: Testing Login Before Email Verification"

LOGIN_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_BASE/auth/login" \
  -H "Content-Type: application/json" \
  -d "{
    \"email\": \"$TEST_EMAIL\",
    \"password\": \"$TEST_PASSWORD\"
  }")

HTTP_CODE=$(echo "$LOGIN_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$LOGIN_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "403" ]; then
    print_success "Unverified user login correctly blocked!"
    echo "$RESPONSE_BODY" | jq '.'
else
    print_error "Expected 403 Forbidden, got HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
fi

# Step 4: Prompt for OTP
print_header "Step 4: Email Verification with OTP"
print_info "Check your email ($TEST_EMAIL) for the 6-digit OTP code"
print_info "Note: If using development mode, check server logs for the OTP"

read -p "Enter OTP code: " OTP_CODE

if [ -z "$OTP_CODE" ]; then
    print_error "OTP code is required"
    exit 1
fi

# Step 5: Verify email with OTP
print_header "Step 5: Verifying Email with OTP"

VERIFY_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_BASE/auth/verify-email" \
  -H "Content-Type: application/json" \
  -d "{
    \"email\": \"$TEST_EMAIL\",
    \"otp\": \"$OTP_CODE\"
  }")

HTTP_CODE=$(echo "$VERIFY_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$VERIFY_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Email verified successfully!"
    echo "$RESPONSE_BODY" | jq '.'
    ACCESS_TOKEN=$(echo "$RESPONSE_BODY" | jq -r '.access_token')
    REFRESH_TOKEN=$(echo "$RESPONSE_BODY" | jq -r '.refresh_token')
    print_info "Access Token: ${ACCESS_TOKEN:0:50}..."
else
    print_error "Email verification failed with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
    exit 1
fi

# Step 6: Test authenticated endpoint
print_header "Step 6: Testing Authenticated Endpoint"

PROFILE_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_BASE/auth/profile" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

HTTP_CODE=$(echo "$PROFILE_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$PROFILE_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Profile retrieved successfully!"
    echo "$RESPONSE_BODY" | jq '.'
else
    print_error "Profile retrieval failed with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
fi

# Step 7: Test token refresh
print_header "Step 7: Testing Token Refresh"

REFRESH_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_BASE/auth/refresh" \
  -H "Content-Type: application/json" \
  -d "{
    \"refresh_token\": \"$REFRESH_TOKEN\"
  }")

HTTP_CODE=$(echo "$REFRESH_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$REFRESH_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Token refreshed successfully!"
    echo "$RESPONSE_BODY" | jq '.'
    NEW_ACCESS_TOKEN=$(echo "$RESPONSE_BODY" | jq -r '.access_token')
else
    print_error "Token refresh failed with HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
fi

# Step 8: Test logout
print_header "Step 8: Testing Logout"

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

# Step 9: Test using blacklisted token
print_header "Step 9: Testing Blacklisted Token"

BLACKLISTED_RESPONSE=$(curl -s -w "\n%{http_code}" -X GET "$API_BASE/auth/profile" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

HTTP_CODE=$(echo "$BLACKLISTED_RESPONSE" | tail -n1)
RESPONSE_BODY=$(echo "$BLACKLISTED_RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "401" ]; then
    print_success "Blacklisted token correctly rejected!"
    echo "$RESPONSE_BODY" | jq '.'
else
    print_error "Expected 401 Unauthorized, got HTTP $HTTP_CODE"
    echo "$RESPONSE_BODY" | jq '.'
fi

print_header "✓ All Signup Flow Tests Complete!"
print_info "Test user: $TEST_EMAIL"
print_info "Test passed successfully!"

