//! OpenAPI documentation for the VEX Gateway API.
//!
//! This module provides auto-generated OpenAPI 3.0 documentation using the `utoipa` crate.
//! The Swagger UI is available at `/swagger-ui/` when enabled.
//!
//! # Endpoint Categories
//!
//! - **Health**: Liveness and readiness probes
//! - **Auth**: Authentication flows (signup, login, OAuth)
//! - **User**: User data and portfolio information
//! - **Order**: Order placement and management
//! - **API Key**: API key management

use utoipa::OpenApi;

/// Health check response.
#[derive(utoipa::ToSchema)]
pub struct HealthResponse {
    /// Service status: "healthy" or "unhealthy"
    pub status: String,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Service version
    pub version: String,
}

/// Readiness check response with dependency status.
#[derive(utoipa::ToSchema)]
pub struct ReadinessResponse {
    /// Overall readiness status
    pub status: String,
    /// Individual dependency checks
    pub checks: ReadinessChecks,
}

/// Individual dependency health checks.
#[derive(utoipa::ToSchema)]
pub struct ReadinessChecks {
    /// Database connection status
    pub database: String,
    /// Redis connection status
    pub redis: String,
}

/// Standard API error response.
#[derive(utoipa::ToSchema)]
pub struct ApiErrorResponse {
    /// Error code (e.g., "VALIDATION_ERROR", "UNAUTHORIZED")
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Optional field-level error details
    #[schema(nullable)]
    pub details: Option<serde_json::Value>,
}

/// User signup request.
#[derive(utoipa::ToSchema)]
pub struct SignupRequest {
    /// User's email address
    #[schema(example = "user@example.com")]
    pub email: String,
    /// Username (3-50 characters)
    #[schema(example = "johndoe", min_length = 3, max_length = 50)]
    pub username: String,
    /// Password (minimum 8 characters)
    #[schema(example = "securepassword123", min_length = 8)]
    pub password: String,
}

/// User login request.
#[derive(utoipa::ToSchema)]
pub struct LoginRequest {
    /// User's email address
    #[schema(example = "user@example.com")]
    pub email: String,
    /// User's password
    #[schema(example = "securepassword123")]
    pub password: String,
}

/// Authentication response with tokens.
#[derive(utoipa::ToSchema)]
pub struct AuthResponse {
    /// JWT access token
    pub access_token: String,
    /// Refresh token for obtaining new access tokens
    pub refresh_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Access token expiry in seconds
    pub expires_in: i64,
    /// User information
    pub user: UserInfo,
}

/// Basic user information.
#[derive(utoipa::ToSchema)]
pub struct UserInfo {
    /// Unique user identifier
    pub user_id: String,
    /// User's email address
    pub email: String,
    /// Username
    pub username: String,
    /// Whether email is verified
    pub email_verified: bool,
}

/// Order placement request.
#[derive(utoipa::ToSchema)]
pub struct OrderRequest {
    /// Market identifier
    #[schema(example = 1)]
    pub market_id: i64,
    /// Order side: "buy" or "sell"
    #[schema(example = "buy")]
    pub side: String,
    /// Order type: "limit" or "market"
    #[schema(example = "limit")]
    pub order_type: String,
    /// Order quantity
    #[schema(example = "1.5")]
    pub quantity: String,
    /// Limit price (required for limit orders)
    #[schema(example = "50000.00", nullable)]
    pub price: Option<String>,
    /// Time in force: "gtc", "ioc", "fok"
    #[schema(example = "gtc")]
    pub time_in_force: String,
}

/// Order response.
#[derive(utoipa::ToSchema)]
pub struct OrderResponse {
    /// Unique order identifier
    pub order_id: String,
    /// Client-provided order ID
    pub client_order_id: String,
    /// Order status
    pub status: String,
    /// Filled quantity
    pub filled_qty: String,
    /// Average fill price
    pub avg_price: String,
    /// Order creation timestamp
    pub created_at: String,
}

/// Order cancellation response.
#[derive(utoipa::ToSchema)]
pub struct CancelOrderResponse {
    /// Order ID that was cancelled
    pub order_id: u64,
    /// Client order ID for correlation
    pub client_order_id: u64,
    /// Cancellation status
    pub status: String,
    /// Status message
    pub message: String,
}

/// User balance information.
#[derive(utoipa::ToSchema)]
pub struct BalanceResponse {
    /// Asset symbol
    pub asset: String,
    /// Available balance
    pub available: String,
    /// Locked balance (in open orders)
    pub locked: String,
    /// Total balance
    pub total: String,
}

/// Open order information.
#[derive(utoipa::ToSchema)]
pub struct OpenOrderResponse {
    /// Order ID
    pub order_id: String,
    /// Market ID
    pub market_id: i64,
    /// Order side
    pub side: String,
    /// Order price
    pub price: String,
    /// Order quantity
    pub quantity: String,
    /// Filled quantity
    pub filled: String,
    /// Order status
    pub status: String,
    /// Creation timestamp
    pub created_at: String,
}

/// VEX Gateway API Documentation
#[derive(OpenApi)]
#[openapi(
    info(
        title = "VEX Gateway API",
        version = "1.0.0",
        description = "Trading gateway API for the VEX exchange platform.\n\n## Authentication\n\nMost endpoints require authentication via JWT tokens or API keys.\n\n### JWT Authentication\nInclude the access token in the Authorization header:\n```\nAuthorization: Bearer <access_token>\n```\n\n### API Key Authentication\nInclude API key and signature in headers:\n```\nX-VEX-APIKEY: <api_key>\nX-VEX-SIGNATURE: <signature>\nX-VEX-TIMESTAMP: <timestamp>\n```",
        contact(
            name = "VEX Support",
            email = "support@vex.exchange"
        ),
        license(
            name = "Proprietary"
        )
    ),
    servers(
        (url = "/api/v1", description = "API v1")
    ),
    tags(
        (name = "health", description = "Health check endpoints"),
        (name = "auth", description = "Authentication endpoints"),
        (name = "user", description = "User data endpoints"),
        (name = "order", description = "Order management endpoints"),
        (name = "api-key", description = "API key management endpoints")
    ),
    paths(
        health_liveness,
        health_readiness,
        place_order,
        cancel_order,
        get_balances,
        get_open_orders,
        get_order_history,
        get_portfolio,
    ),
    components(
        schemas(
            HealthResponse,
            ReadinessResponse,
            ReadinessChecks,
            ApiErrorResponse,
            SignupRequest,
            LoginRequest,
            AuthResponse,
            UserInfo,
            OrderRequest,
            OrderResponse,
            CancelOrderResponse,
            BalanceResponse,
            OpenOrderResponse,
        )
    )
)]
pub struct ApiDoc;

/// Liveness probe - checks if the service is running.
#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    responses(
        (status = 200, description = "Service is alive", body = HealthResponse),
    )
)]
pub async fn health_liveness() {}

/// Readiness probe - checks if the service can handle requests.
#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    responses(
        (status = 200, description = "Service is ready", body = ReadinessResponse),
        (status = 503, description = "Service is not ready", body = ReadinessResponse),
    )
)]
pub async fn health_readiness() {}

/// Place a new order.
#[utoipa::path(
    post,
    path = "/orders",
    tag = "order",
    request_body = OrderRequest,
    responses(
        (status = 200, description = "Order placed successfully", body = OrderResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Unauthorized", body = ApiErrorResponse),
        (status = 408, description = "Order timeout", body = ApiErrorResponse),
    ),
    security(
        ("bearer_auth" = []),
        ("api_key_auth" = [])
    )
)]
pub async fn place_order() {}

/// Cancel an existing order.
#[utoipa::path(
    delete,
    path = "/orders/{order_id}",
    tag = "order",
    params(
        ("order_id" = String, Path, description = "Order ID to cancel"),
        ("market_id" = u32, Query, description = "Market ID for validation")
    ),
    responses(
        (status = 200, description = "Order cancelled successfully", body = CancelOrderResponse),
        (status = 400, description = "Invalid request", body = ApiErrorResponse),
        (status = 401, description = "Unauthorized", body = ApiErrorResponse),
        (status = 404, description = "Order not found", body = ApiErrorResponse),
        (status = 408, description = "Cancel timeout", body = ApiErrorResponse),
    ),
    security(
        ("bearer_auth" = []),
        ("api_key_auth" = [])
    )
)]
pub async fn cancel_order() {}

/// Get user balances.
#[utoipa::path(
    get,
    path = "/user/balances",
    tag = "user",
    responses(
        (status = 200, description = "User balances", body = Vec<BalanceResponse>),
        (status = 401, description = "Unauthorized", body = ApiErrorResponse),
    ),
    security(
        ("bearer_auth" = []),
        ("api_key_auth" = [])
    )
)]
pub async fn get_balances() {}

/// Get user's open orders.
#[utoipa::path(
    get,
    path = "/user/orders/open",
    tag = "user",
    responses(
        (status = 200, description = "Open orders", body = Vec<OpenOrderResponse>),
        (status = 401, description = "Unauthorized", body = ApiErrorResponse),
    ),
    security(
        ("bearer_auth" = []),
        ("api_key_auth" = [])
    )
)]
pub async fn get_open_orders() {}

/// Get user's order history.
#[utoipa::path(
    get,
    path = "/user/orders/history",
    tag = "user",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of orders to return"),
        ("offset" = Option<u32>, Query, description = "Number of orders to skip")
    ),
    responses(
        (status = 200, description = "Order history", body = Vec<OpenOrderResponse>),
        (status = 401, description = "Unauthorized", body = ApiErrorResponse),
    ),
    security(
        ("bearer_auth" = []),
        ("api_key_auth" = [])
    )
)]
pub async fn get_order_history() {}

/// Get user's portfolio overview.
#[utoipa::path(
    get,
    path = "/user/portfolio",
    tag = "user",
    responses(
        (status = 200, description = "Portfolio overview"),
        (status = 401, description = "Unauthorized", body = ApiErrorResponse),
    ),
    security(
        ("bearer_auth" = []),
        ("api_key_auth" = [])
    )
)]
pub async fn get_portfolio() {}

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::OpenApi;

    #[test]
    fn test_openapi_spec_generation() {
        let spec = ApiDoc::openapi();
        let json = spec.to_json().expect("Failed to serialize OpenAPI spec");

        // Verify basic structure
        assert!(json.contains("\"title\":\"VEX Gateway API\""));
        assert!(json.contains("\"version\":\"1.0.0\""));
        assert!(json.contains("/health/live"));
        assert!(json.contains("/health/ready"));
    }

    #[test]
    fn test_openapi_has_components() {
        let spec = ApiDoc::openapi();
        let json = spec.to_json().expect("Failed to serialize OpenAPI spec");

        // Verify schemas are included
        assert!(json.contains("HealthResponse"));
        assert!(json.contains("ApiErrorResponse"));
        assert!(json.contains("SignupRequest"));
    }
}
