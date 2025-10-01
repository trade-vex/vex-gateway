use crate::repository::TradingError;
use std::time::Duration;
use async_trait::async_trait;

#[async_trait]
pub trait Cache<T>: Send + Sync + 'static 
where
    T: Clone + Send + Sync + 'static,
{
    async fn get_by_id(&self, key: &str) -> Result<Option<T>, TradingError>;
    
    async fn set_by_id(&self, key: &str, data: &T, ttl: Duration) -> Result<(), TradingError>;
    
    async fn delete_by_id(&self, key: &str) -> Result<(), TradingError>;
    
    async fn ping(&self) -> Result<(), TradingError>;
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
pub struct NoOpCache<T> {
    stats: std::sync::Arc<std::sync::atomic::AtomicU64>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> NoOpCache<T> {
    pub fn new() -> Self {
        Self {
            stats: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<T> Cache<T> for NoOpCache<T> 
where
    T: Clone + Send + Sync + 'static,
{
    async fn get_by_id(&self, _key: &str) -> Result<Option<T>, TradingError> {
        self.stats.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(None)
    }

    async fn set_by_id(&self, _key: &str, _data: &T, _ttl: Duration) -> Result<(), TradingError> {
        Ok(())
    }

    async fn delete_by_id(&self, _key: &str) -> Result<(), TradingError> {
        Ok(())
    }

    async fn ping(&self) -> Result<(), TradingError> {
        Ok(())
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            hits: 0,
            misses: self.stats.load(std::sync::atomic::Ordering::Relaxed),
            errors: 0,
            sets: 0,
            deletes: 0,
            size: None,
        }
    }
}
