use crate::error::RateLimitError;
use redis::{AsyncCommands, Client};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

#[derive(Clone)]
pub struct RedisRateLimitStore {
    client: Client,
    key_prefix: String,
}

impl RedisRateLimitStore {
    /// create a new Redis rate limit store
    pub async fn new(redis_url: &str, key_prefix: String) -> Result<Self, RateLimitError> {
        let client = Client::open(redis_url)
            .map_err(|e| RateLimitError::RedisConnection(e.to_string()))?;

        Ok(Self {
            client,
            key_prefix,
        })
    }

    /// get the current request count for a user in the current window
    pub async fn get_request_count(&self, user_id: &str, window_start: u64) -> Result<u32, RateLimitError> {
        let mut conn = self.client.get_async_connection().await
            .map_err(|e| RateLimitError::RedisConnection(e.to_string()))?;

        let key = self.build_key(user_id, window_start);
        
        let count: u32 = conn.get(&key).await.unwrap_or(0);
        debug!("Current request count for user {} in window {}: {}", user_id, window_start, count);
        
        Ok(count)
    }

    /// increment the request count for a user in the current window
    pub async fn increment_request_count(
        &self, 
        user_id: &str, 
        window_start: u64, 
        window_duration: Duration
    ) -> Result<u32, RateLimitError> {
        let mut conn = self.client.get_async_connection().await
            .map_err(|e| RateLimitError::RedisConnection(e.to_string()))?;

        let key: String = self.build_key(user_id, window_start);
        
        let count: u32 = conn.incr(&key, 1).await?;
        
        if count == 1 {
            conn.expire(&key, window_duration.as_secs() as i64).await?;
            debug!("Set expiration for key {} to {} seconds", key, window_duration.as_secs());
        }
        
        debug!("Incremented request count for user {} in window {}: {}", user_id, window_start, count);
        Ok(count)
    }

    /// check if a user has exceeded the rate limit
    pub async fn is_rate_limited(
        &self, 
        user_id: &str, 
        window_start: u64, 
        max_requests: u32,
        window_duration: Duration
    ) -> Result<bool, RateLimitError> {
        let count = self.increment_request_count(user_id, window_start, window_duration).await?;
        Ok(count > max_requests)
    }

    /// get the current window start timestamp
    pub fn get_current_window_start(&self, window_size_seconds: u64) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        now - (now % window_size_seconds)
    }

    fn build_key(&self, user_id: &str, window_start: u64) -> String {
        format!("{}:{}:{}", self.key_prefix, user_id, window_start)
    }

    pub async fn ping(&self) -> Result<(), RateLimitError> {
        let mut conn = self.client.get_async_connection().await
            .map_err(|e| RateLimitError::RedisConnection(e.to_string()))?;

        let _: String = conn.get("ping").await?;
        Ok(())
    }
}
