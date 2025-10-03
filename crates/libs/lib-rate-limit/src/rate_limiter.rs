use crate::config::RateLimitConfig;
use crate::error::RateLimitError;
use crate::redis_store::RedisRateLimitStore;
use async_trait::async_trait;

/// trait for rate limiting implementations
#[async_trait]
pub trait RateLimiter: Send + Sync {
    async fn is_allowed(&self, user_id: &str) -> Result<bool, RateLimitError>;    
    async fn get_current_count(&self, user_id: &str) -> Result<u32, RateLimitError>;
    async fn get_remaining_requests(&self, user_id: &str) -> Result<u32, RateLimitError>;
    async fn get_window_reset_time(&self, user_id: &str) -> Result<u64, RateLimitError>;
}

/// fixed window rate limiter implementation
pub struct FixedWindowRateLimiter {
    store: RedisRateLimitStore,
    config: RateLimitConfig,
}

impl FixedWindowRateLimiter {
    /// create a new fixed window rate limiter
    pub fn new(store: RedisRateLimitStore, config: RateLimitConfig) -> Self {
        Self { store, config }
    }

    /// get the current window start timestamp
    fn get_current_window_start(&self) -> u64 {
        self.store.get_current_window_start(self.config.window_size_seconds)
    }
}

#[async_trait]
impl RateLimiter for FixedWindowRateLimiter {
    async fn is_allowed(&self, user_id: &str) -> Result<bool, RateLimitError> {
        if !self.config.enabled {
            return Ok(true);
        }

        let window_start = self.get_current_window_start();
        
        let is_limited = self.store.is_rate_limited(
            user_id,
            window_start,
            self.config.max_requests,
            self.config.window_duration(),
        ).await?;

        Ok(!is_limited)
    }

    async fn get_current_count(&self, user_id: &str) -> Result<u32, RateLimitError> {
        let window_start = self.get_current_window_start();
        self.store.get_request_count(user_id, window_start).await
    }

    async fn get_remaining_requests(&self, user_id: &str) -> Result<u32, RateLimitError> {
        let current_count = self.get_current_count(user_id).await?;
        if current_count >= self.config.max_requests {
            Ok(0)
        } else {
            Ok(self.config.max_requests - current_count)
        }
    }

    async fn get_window_reset_time(&self, _user_id: &str) -> Result<u64, RateLimitError> {
        let window_start = self.get_current_window_start();
        let window_end = window_start + self.config.window_size_seconds;
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Ok(window_end - now)
    }
}
