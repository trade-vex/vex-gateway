use crate::api_key::{
    cache::{ApiKeyCache, CacheStats},
    error::ApiKeyError,
    security::ApiKeyConfig,
};
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands, Client, ErrorKind, RedisError};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// this function redacts an API key for safe logging by showing only the last 4 characters
/// or "***" for short keys to avoid leaking sensitive data
fn redact_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 4 {
        "***".into()
    } else {
        format!("***{}", &key[key.len().saturating_sub(4)..])
    }
}

#[derive(Clone)]
pub struct RedisApiKeyCache {
    conn: ConnectionManager,
    key_prefix: String,
    stats: Arc<CacheStatsInternal>,
}

#[derive(Debug, Default)]
struct CacheStatsInternal {
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
    errors: std::sync::atomic::AtomicU64,
    sets: std::sync::atomic::AtomicU64,
    deletes: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone)]
pub struct RedisCacheConfig {
    pub url: String,
    pub key_prefix: String,
    pub retry_count: u32,
}

impl Default for RedisCacheConfig {
    fn default() -> Self {
        Self {
            url: "redis://localhost:6379".to_string(),
            key_prefix: "api_key:".to_string(),
            retry_count: 3,
        }
    }
}

impl RedisApiKeyCache {
    pub async fn new(config: RedisCacheConfig) -> Result<Self, ApiKeyError> {
        let client = Client::open(config.url.clone())
            .map_err(|e| ApiKeyError::Internal(format!("Failed to create Redis client: {}", e)))?;

        let conn = ConnectionManager::new(client).await.map_err(|e| {
            ApiKeyError::Internal(format!("Failed to create Redis connection: {}", e))
        })?;

        Ok(Self {
            conn,
            key_prefix: config.key_prefix,
            stats: Arc::new(CacheStatsInternal::default()),
        })
    }

    async fn execute_with_retry<F, T>(&self, f: F) -> Result<T, ApiKeyError>
    where
        F: Fn(
            &mut ConnectionManager,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::result::Result<T, RedisError>> + Send + '_>,
        >,
    {
        let mut last_error = None;
        let mut conn = self.conn.clone();

        for attempt in 0..=3 {
            match f(&mut conn).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = Some(e);
                    if !self.is_retryable(last_error.as_ref().unwrap()) {
                        break;
                    }
                    if attempt < 3 {
                        debug!(
                            "Redis operation failed, retrying (attempt {}/3): {}",
                            attempt + 1,
                            last_error.as_ref().unwrap()
                        );
                        tokio::time::sleep(Duration::from_millis(100 * (attempt + 1) as u64)).await;
                    }
                }
            }
        }

        self.stats
            .errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(ApiKeyError::Internal(format!(
            "Redis operation failed after retries: {}",
            last_error.unwrap()
        )))
    }

    fn is_retryable(&self, error: &RedisError) -> bool {
        matches!(error.kind(), ErrorKind::Io | ErrorKind::Server(_))
    }

    fn build_key(&self, key: &str) -> String {
        format!("{}{}", self.key_prefix, key)
    }
}

#[async_trait]
impl ApiKeyCache for RedisApiKeyCache {
    async fn get(&self, key: &str) -> Result<Option<ApiKeyConfig>, ApiKeyError> {
        let cache_key = self.build_key(key);

        let result = self
            .execute_with_retry(|conn| {
                let cache_key = cache_key.clone();
                Box::pin(async move {
                    let data: Option<Vec<u8>> = conn.get(&cache_key).await?;
                    Ok(data)
                })
            })
            .await?;

        match result {
            Some(data) => {
                self.stats
                    .hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match bincode::deserialize::<ApiKeyConfig>(&data) {
                    Ok(config) => {
                        debug!("Cache hit for API key: {}", redact_key(key));
                        Ok(Some(config))
                    }
                    Err(e) => {
                        warn!(
                            "Failed to deserialize cached API key {}: {}",
                            redact_key(key),
                            e
                        );
                        self.stats
                            .misses
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        Ok(None)
                    }
                }
            }
            None => {
                self.stats
                    .misses
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!("Cache miss for API key: {}", redact_key(key));
                Ok(None)
            }
        }
    }

    async fn set(
        &self,
        key: &str,
        config: &ApiKeyConfig,
        ttl: Duration,
    ) -> Result<(), ApiKeyError> {
        let cache_key = self.build_key(key);

        let serialized = bincode::serialize(config).map_err(|e| {
            ApiKeyError::Serialization(format!("Failed to serialize API key config: {}", e))
        })?;

        self.execute_with_retry(|conn| {
            let cache_key = cache_key.clone();
            let serialized = serialized.clone();
            let ttl_secs = ttl.as_secs();
            Box::pin(async move {
                if ttl_secs > 0 {
                    conn.set_ex::<_, _, ()>(&cache_key, serialized, ttl_secs)
                        .await?;
                } else {
                    conn.set::<_, _, ()>(&cache_key, serialized).await?;
                }
                Ok(())
            })
        })
        .await?;

        self.stats
            .sets
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        debug!(
            "Cached API key: {} with TTL: {}s",
            redact_key(key),
            ttl.as_secs()
        );
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), ApiKeyError> {
        let cache_key = self.build_key(key);

        self.execute_with_retry(|conn| {
            let cache_key = cache_key.clone();
            Box::pin(async move {
                let _: i32 = conn.del(&cache_key).await?;
                Ok(())
            })
        })
        .await?;

        self.stats
            .deletes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        debug!("Deleted cached API key: {}", redact_key(key));
        Ok(())
    }

    async fn ping(&self) -> Result<(), ApiKeyError> {
        self.execute_with_retry(|conn| {
            Box::pin(async move {
                let _: String = redis::cmd("PING").query_async(conn).await?;
                Ok(())
            })
        })
        .await?;
        Ok(())
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.stats.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.stats.misses.load(std::sync::atomic::Ordering::Relaxed),
            errors: self.stats.errors.load(std::sync::atomic::Ordering::Relaxed),
            sets: self.stats.sets.load(std::sync::atomic::Ordering::Relaxed),
            deletes: self
                .stats
                .deletes
                .load(std::sync::atomic::Ordering::Relaxed),
            size: None,
        }
    }
}
