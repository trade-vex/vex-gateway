//! Standardized API error responses
//!
//! This module provides consistent error handling across all API handlers.
//! All error responses follow a standard JSON format with error code and message.

use actix_web::{HttpResponse, http::StatusCode};
use serde::Serialize;
use std::fmt;

/// Standard API error response format
#[derive(Debug, Serialize)]
pub struct ApiError {
    /// Machine-readable error code (e.g., "VALIDATION_ERROR", "NOT_FOUND")
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Optional additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    /// Create a new API error
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    /// Add details to the error
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Convert to HttpResponse with the given status code
    pub fn into_response(self, status: StatusCode) -> HttpResponse {
        HttpResponse::build(status).json(self)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// Standard error codes used across the API.
///
/// # Error Code Reference
///
/// | Code | HTTP Status | Description |
/// |------|-------------|-------------|
/// | `VALIDATION_ERROR` | 400 | Request data failed validation (invalid format, out of range, etc.) |
/// | `BAD_REQUEST` | 400 | Malformed request syntax or invalid parameters |
/// | `UNAUTHORIZED` | 401 | Missing or invalid authentication credentials |
/// | `FORBIDDEN` | 403 | Valid credentials but insufficient permissions |
/// | `NOT_FOUND` | 404 | Requested resource does not exist |
/// | `CONFLICT` | 409 | Request conflicts with current state (e.g., duplicate order) |
/// | `RATE_LIMITED` | 429 | Too many requests, retry after the specified delay |
/// | `INTERNAL_ERROR` | 500 | Unexpected server error (logged for investigation) |
/// | `SERVICE_UNAVAILABLE` | 503 | Service temporarily unavailable (maintenance, overload) |
/// | `TIMEOUT` | 504 | Upstream service timeout |
pub mod codes {
    /// Request data failed validation (e.g., negative price, invalid market_id).
    /// HTTP 400.
    pub const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

    /// Requested resource does not exist.
    /// HTTP 404.
    pub const NOT_FOUND: &str = "NOT_FOUND";

    /// Unexpected server error. These are logged and should be reported.
    /// HTTP 500.
    pub const INTERNAL_ERROR: &str = "INTERNAL_ERROR";

    /// Authentication required. Provide valid JWT or API key.
    /// HTTP 401.
    pub const UNAUTHORIZED: &str = "UNAUTHORIZED";

    /// Authenticated but not authorized for this action.
    /// HTTP 403.
    pub const FORBIDDEN: &str = "FORBIDDEN";

    /// Malformed request syntax or invalid request parameters.
    /// HTTP 400.
    pub const BAD_REQUEST: &str = "BAD_REQUEST";

    /// Request conflicts with current resource state (e.g., duplicate order ID).
    /// HTTP 409.
    pub const CONFLICT: &str = "CONFLICT";

    /// Rate limit exceeded. Check `Retry-After` header for when to retry.
    /// HTTP 429.
    pub const RATE_LIMITED: &str = "RATE_LIMITED";

    /// Service temporarily unavailable. Retry with exponential backoff.
    /// HTTP 503.
    pub const SERVICE_UNAVAILABLE: &str = "SERVICE_UNAVAILABLE";

    /// Upstream service timeout. The request may or may not have been processed.
    /// HTTP 504.
    pub const TIMEOUT: &str = "TIMEOUT";
}

/// Helper functions for common error responses
impl ApiError {
    /// 400 Bad Request - validation or format errors
    pub fn bad_request(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::BAD_REQUEST, message).into_response(StatusCode::BAD_REQUEST)
    }

    /// 400 Bad Request with validation details
    pub fn validation_error(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::VALIDATION_ERROR, message).into_response(StatusCode::BAD_REQUEST)
    }

    /// 401 Unauthorized - authentication required
    pub fn unauthorized(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::UNAUTHORIZED, message).into_response(StatusCode::UNAUTHORIZED)
    }

    /// 403 Forbidden - authenticated but not authorized
    pub fn forbidden(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::FORBIDDEN, message).into_response(StatusCode::FORBIDDEN)
    }

    /// 404 Not Found - resource doesn't exist
    pub fn not_found(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::NOT_FOUND, message).into_response(StatusCode::NOT_FOUND)
    }

    /// 409 Conflict - resource state conflict
    pub fn conflict(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::CONFLICT, message).into_response(StatusCode::CONFLICT)
    }

    /// 429 Too Many Requests - rate limited
    pub fn rate_limited(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::RATE_LIMITED, message).into_response(StatusCode::TOO_MANY_REQUESTS)
    }

    /// 500 Internal Server Error - unexpected server error
    pub fn internal_error(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::INTERNAL_ERROR, message).into_response(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// 503 Service Unavailable - service temporarily unavailable
    pub fn service_unavailable(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::SERVICE_UNAVAILABLE, message)
            .into_response(StatusCode::SERVICE_UNAVAILABLE)
    }

    /// 504 Gateway Timeout - upstream timeout
    pub fn timeout(message: impl Into<String>) -> HttpResponse {
        Self::new(codes::TIMEOUT, message).into_response(StatusCode::GATEWAY_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_serialization() {
        let error = ApiError::new("TEST_ERROR", "Test message");
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("TEST_ERROR"));
        assert!(json.contains("Test message"));
    }

    #[test]
    fn test_api_error_with_details() {
        let error = ApiError::new("TEST_ERROR", "Test message")
            .with_details(serde_json::json!({"field": "email"}));
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("email"));
    }

    #[test]
    fn test_error_display() {
        let error = ApiError::new("TEST_ERROR", "Test message");
        assert_eq!(error.to_string(), "[TEST_ERROR] Test message");
    }
}
