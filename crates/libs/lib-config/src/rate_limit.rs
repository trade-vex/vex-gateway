//! lib-config/src/rate_limit.rs

use crate::ConfigError;
use lib_middleware::rate_limit::Limit;
use serde::Deserialize;

/// The API's rate limits: the `[rate_limit]` section. Each budget allows each caller, an
/// authenticated user or an anonymous client IP, `max_requests` per window of `window_seconds`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RateLimitConfig {
    /// Placing orders (`POST /api/order`).
    pub orders: Limit,
    /// Every other route that has no limit of its own, cancelling orders included: a caller over
    /// the `orders` budget can still cancel its resting orders.
    pub general: Limit,
}

impl RateLimitConfig {
    /// Every setting must be greater than 0. The error names the setting.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        for (budget, limit) in [("orders", self.orders), ("general", self.general)] {
            for (key, value) in [
                ("max_requests", limit.max_requests),
                ("window_seconds", limit.window_seconds),
            ] {
                if value == 0 {
                    return Err(ConfigError::ValidationError(format!(
                        "rate_limit.{budget}.{key} must be greater than 0"
                    )));
                }
            }
        }
        Ok(())
    }
}
