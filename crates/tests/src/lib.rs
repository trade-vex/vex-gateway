//! tests/src/lib.rs
//!
//! Integration tests are placed in the tests/ directory

use actix_web::http::StatusCode;
use vex_networking::client::GatewayError;

pub fn gateway_send_error(error: &GatewayError, failure_message: String) -> (StatusCode, String) {
    match error {
        GatewayError::Busy => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Matching engine is busy; please retry".to_string(),
        ),
        GatewayError::NotConnected => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Matching engine unavailable".to_string(),
        ),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, failure_message),
    }
}

/// The tests' Redis: `REDIS_URL`, or the default local port when it is unset. The Docker
/// development stack (`scripts/setup_redis.sh`) puts Redis on 6380, so run these tests against it
/// with `REDIS_URL=redis://127.0.0.1:6380`.
pub fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}
