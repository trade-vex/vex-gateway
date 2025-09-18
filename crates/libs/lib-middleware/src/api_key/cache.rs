use crate::api_key::security::ApiKeyConfig;
use crate::api_key::error::ApiKeyError;
use std::time::Duration;
use async_trait::async_trait;

#[async_trait]
pub trait ApiKeyCache: Send + Sync + 'static {
    async fn get(&self, key: &str) -> Result<Option<ApiKeyConfig>, ApiKeyError>;
    async fn set(&self, key: &str, config: &ApiKeyConfig, ttl: Duration) -> Result<(), ApiKeyError>;
    async fn delete(&self, key: &str) -> Result<(), ApiKeyError>;
    async fn ping(&self) -> Result<(), ApiKeyError>;
    fn stats(&self) -> CacheStats;
}

#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub errors: u64,
    pub sets: u64,
    pub deletes: u64,
    pub size: Option<usize>,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f64 / total as f64) * 100.0
        }
    }
}

#[derive(Clone)]
pub struct NoOpCache {
    stats: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl NoOpCache {
    pub fn new() -> Self {
        Self {
            stats: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl ApiKeyCache for NoOpCache {
    async fn get(&self, _key: &str) -> Result<Option<ApiKeyConfig>, ApiKeyError> {
        self.stats.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(None)
    }

    async fn set(&self, _key: &str, _config: &ApiKeyConfig, _ttl: Duration) -> Result<(), ApiKeyError> {
        self.stats.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn delete(&self, _key: &str) -> Result<(), ApiKeyError> {
        self.stats.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn ping(&self) -> Result<(), ApiKeyError> {
        Ok(())
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            hits: 0,
            misses: 0,
            errors: 0,
            sets: 0,
            deletes: 0,
            size: None,
        }
    }
}

impl Default for NoOpCache {
    fn default() -> Self {
        Self::new()
    }
}
