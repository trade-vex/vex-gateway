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
