# Vex Gateway Trading API Documentation

## Overview

The Vex Gateway Trading API provides comprehensive trading functionality including order management, user balances, and authentication. The API supports both JWT and API key authentication methods.

## Base URL

- **Production**: `https://api.vex-gateway.com`
- **Staging**: `https://staging-api.vex-gateway.com`
- **Local Development**: `http://localhost:8080`

## Authentication

The API supports two authentication methods:

### JWT Authentication
Include the JWT token in the Authorization header:
```
Authorization: Bearer <your-jwt-token>
```

### API Key Authentication
Include your API key in the X-API-Key header:
```
X-API-Key: <your-api-key>
```

## API Endpoints

### System

#### Health Check
```http
GET /health
```

**Description**: Check if the API service is healthy and running

**Authentication**: None required

**Response**:
```json
{
  "status": "healthy",
  "timestamp": "2023-01-01T00:00:00Z"
}
```

### Authentication

#### Initiate OAuth Login
```http
GET /auth/oauth/{provider}
```

**Description**: Start OAuth authentication flow with the specified provider

**Parameters**:
- `provider` (path): OAuth provider (`google`, `github`, `microsoft`)

**Authentication**: None required

**Response**: Redirect to OAuth provider (302)

#### OAuth Callback
```http
GET /auth/oauth/{provider}/callback
```

**Description**: Handle OAuth callback from provider

**Parameters**:
- `provider` (path): OAuth provider
- `code` (query): Authorization code from OAuth provider
- `state` (query, optional): State parameter for CSRF protection

**Authentication**: None required

**Response**:
```json
{
  "access_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "token_type": "Bearer",
  "expires_in": 3600,
  "user": {
    "user_id": "550e8400-e29b-41d4-a716-446655440000",
    "username": "john_doe",
    "email": "john@example.com",
    "created_at": "2023-01-01T00:00:00Z",
    "last_login": "2023-01-01T00:00:00Z"
  }
}
```

#### Get Available OAuth Providers
```http
GET /auth/oauth/providers
```

**Description**: List all available OAuth authentication providers

**Authentication**: None required

**Response**:
```json
{
  "providers": [
    {
      "name": "google",
      "display_name": "Google",
      "auth_url": "/auth/oauth/google"
    }
  ]
}
```

### User Management

#### Get Current User Info
```http
GET /api/me
```

**Description**: Get information about the currently authenticated user

**Authentication**: Required (JWT or API Key)

**Response**:
```json
{
  "message": "Hello authenticated user!",
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "timestamp": "2023-01-01T00:00:00Z"
}
```

#### Get User Profile
```http
GET /api/profile
```

**Description**: Get detailed profile information for the authenticated user

**Authentication**: Required (JWT or API Key)

**Response**:
```json
{
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "username": "john_doe",
  "email": "john@example.com",
  "created_at": "2023-01-01T00:00:00Z",
  "last_login": "2023-01-01T00:00:00Z"
}
```

### Trading

#### Get User Balances
```http
GET /api/users/balances
```

**Description**: Retrieve available and locked balances for the authenticated user

**Authentication**: Required (JWT or API Key)

**Response**:
```json
{
  "balances": [
    {
      "user_id": "550e8400-e29b-41d4-a716-446655440000",
      "available": 10000,
      "locked": 5000,
      "total": 15000,
      "last_updated": "2023-01-01T00:00:00Z"
    }
  ],
  "total_assets": 1
}
```

**Error Responses**:
- `401 Unauthorized`: Invalid or missing authentication
- `500 Internal Server Error`: Server error

#### Get User Open Orders
```http
GET /api/users/orders/open
```

**Description**: Retrieve all open orders for the authenticated user

**Authentication**: Required (JWT or API Key)

**Response**:
```json
{
  "orders": [
    {
      "order_id": 12345,
      "user_id": "550e8400-e29b-41d4-a716-446655440000",
      "price": 50000,
      "size": 1000,
      "side": "Bid",
      "timestamp": 1640995200000
    }
  ],
  "total": 1
}
```

**Error Responses**:
- `401 Unauthorized`: Invalid or missing authentication
- `500 Internal Server Error`: Server error

#### Place Order
```http
POST /order
```

**Description**: Place a new trading order

**Authentication**: Required (JWT or API Key)

**Request Body**:
```json
{
  "price": 50000,
  "volume": 1000,
  "symbol_id": 1,
  "side": "Bid",
  "order_type": "Gtc"
}
```

**Request Parameters**:
- `price` (integer, required): Order price in cents (minimum: 1)
- `volume` (integer, required): Order volume/quantity (minimum: 1)
- `symbol_id` (integer, required): Trading symbol identifier (minimum: 1)
- `side` (string, required): Order side - `Bid` (buy) or `Ask` (sell)
- `order_type` (string, required): Order type - `Gtc` (Good Till Cancel), `Ioc` (Immediate or Cancel), `Fok` (Fill or Kill)

**Response**:
```json
{
  "order_id": 12345,
  "status": "success",
  "message": "Order processed successfully"
}
```

**Error Responses**:
- `400 Bad Request`: Invalid order request
- `401 Unauthorized`: Invalid or missing authentication
- `408 Request Timeout`: Order processing timeout
- `500 Internal Server Error`: Server error

**Timeout Response**:
```json
{
  "order_id": 12345,
  "status": "timeout",
  "message": "Order processing timed out"
}
```

#### Cancel Order
```http
POST /cancel
```

**Description**: Cancel an existing trading order

**Authentication**: Required (JWT or API Key)

**Request Body**:
```json
{
  "order_id": 12345,
  "symbol_id": 1
}
```

**Request Parameters**:
- `order_id` (integer, required): Order ID to cancel (minimum: 1)
- `symbol_id` (integer, required): Trading symbol identifier (minimum: 1)

**Response**:
```json
{
  "order_id": 12345,
  "status": "success",
  "message": "Order cancelled successfully"
}
```

**Error Responses**:
- `400 Bad Request`: Invalid cancel request
- `401 Unauthorized`: Invalid or missing authentication
- `404 Not Found`: Order not found
- `408 Request Timeout`: Order cancellation timeout
- `500 Internal Server Error`: Server error

**Timeout Response**:
```json
{
  "order_id": 12345,
  "status": "timeout",
  "message": "Order cancellation timed out"
}
```

## Data Models

### Order
```json
{
  "order_id": 12345,
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "price": 50000,
  "size": 1000,
  "side": "Bid",
  "timestamp": 1640995200000
}
```

**Fields**:
- `order_id` (integer): Unique order identifier
- `user_id` (uuid): User who placed the order
- `price` (integer): Order price in cents
- `size` (integer): Order size/quantity
- `side` (string): Order side (`Bid` or `Ask`)
- `timestamp` (integer): Order timestamp in milliseconds since Unix epoch

### UserBalance
```json
{
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "available": 10000,
  "locked": 5000,
  "total": 15000,
  "last_updated": "2023-01-01T00:00:00Z"
}
```

**Fields**:
- `user_id` (uuid): User identifier
- `available` (integer): Available balance in cents
- `locked` (integer): Locked balance in cents
- `total` (integer): Total balance (available + locked) in cents
- `last_updated` (datetime): Last update timestamp

### Error Response
```json
{
  "error": "validation_error",
  "message": "Invalid order parameters",
  "code": 400,
  "details": {}
}
```

**Fields**:
- `error` (string): Error type
- `message` (string): Detailed error message
- `code` (integer): HTTP status code
- `details` (object): Additional error details

## Rate Limiting

API requests are rate limited. Check response headers for current limits:
- `X-RateLimit-Limit`: Maximum requests per window
- `X-RateLimit-Remaining`: Remaining requests in current window
- `X-RateLimit-Reset`: Time when the rate limit resets

## Error Handling

All errors follow a consistent format with the following HTTP status codes:

- `200 OK`: Request successful
- `400 Bad Request`: Invalid request parameters
- `401 Unauthorized`: Authentication required or invalid
- `404 Not Found`: Resource not found
- `408 Request Timeout`: Request processing timeout
- `500 Internal Server Error`: Server error

## Example Usage

### Place an Order
```bash
curl -X POST "http://localhost:8080/order" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "price": 50000,
    "volume": 1000,
    "symbol_id": 1,
    "side": "Bid",
    "order_type": "Gtc"
  }'
```

### Get User Balances
```bash
curl -X GET "http://localhost:8080/api/users/balances" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN"
```

### Cancel an Order
```bash
curl -X POST "http://localhost:8080/cancel" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "order_id": 12345,
    "symbol_id": 1
  }'
```

### Using API Key Authentication
```bash
curl -X GET "http://localhost:8080/api/users/balances" \
  -H "X-API-Key: YOUR_API_KEY"
```

## SDKs and Tools

### Postman Collection
Import the OpenAPI specification directly into Postman for testing:
1. Open Postman
2. Click "Import"
3. Select "Link" tab
4. Enter the OpenAPI spec URL
5. Click "Continue" and "Import"

### Code Generation
Generate client SDKs using OpenAPI Generator:

```bash
# Generate TypeScript client
npx @openapitools/openapi-generator-cli generate \
  -i api-spec.yaml \
  -g typescript-axios \
  -o ./client/typescript

# Generate Python client
npx @openapitools/openapi-generator-cli generate \
  -i api-spec.yaml \
  -g python \
  -o ./client/python
```

## Support

For API support and questions:
- Email: support@vex-gateway.com
- Documentation: This file
- OpenAPI Spec: `api-spec.yaml`