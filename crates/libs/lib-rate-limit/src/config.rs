use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub max_requests: u32,
    pub window_size_seconds: u64,
    pub key_prefix: String,
    pub enabled: bool,
    pub error_message: Option<String>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 1000,
            window_size_seconds: 60,
            key_prefix: "rate_limit".to_string(),
            enabled: true,
            error_message: Some("Rate limit exceeded. Please try again later.".to_string()),
        }
    }
}

impl RateLimitConfig {
    pub fn new(max_requests: u32, window_size_seconds: u64) -> Self {
        Self {
            max_requests,
            window_size_seconds,
            ..Default::default()
        }
    }

    pub fn window_duration(&self) -> Duration {
        Duration::from_secs(self.window_size_seconds)
    }

    pub fn requests_per_minute(requests: u32) -> Self {
        Self::new(requests, 60)
    }
}
