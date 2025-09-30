use crate::cache::{Cache, CacheStats};
use crate::repository::{TradingRepository, TradingError};
use crate::user_balances::Model as UserBalanceModel;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;


#[derive(Clone)]
pub struct UserBalanceStore {
    cache: Option<Arc<dyn Cache<UserBalanceModel>>>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: UserBalanceCacheConfig,
    metrics: Arc<UserBalanceCacheMetrics>,
}

#[derive(Clone)]
pub struct UserBalanceCacheConfig {
    pub ttl: Duration,
    pub enabled: bool,
}

impl Default for UserBalanceCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(1800),
            enabled: false,
        }
    }
}

pub struct UserBalanceCacheMetrics {
    pub cache_hits: std::sync::atomic::AtomicU64,
    pub cache_misses: std::sync::atomic::AtomicU64,
    pub cache_errors: std::sync::atomic::AtomicU64,
    pub db_hits: std::sync::atomic::AtomicU64,
    pub db_errors: std::sync::atomic::AtomicU64,
}

impl Default for UserBalanceCacheMetrics {
    fn default() -> Self {
        Self {
            cache_hits: std::sync::atomic::AtomicU64::new(0),
            cache_misses: std::sync::atomic::AtomicU64::new(0),
            cache_errors: std::sync::atomic::AtomicU64::new(0),
            db_hits: std::sync::atomic::AtomicU64::new(0),
            db_errors: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl UserBalanceStore {
    /// Create new store 
    pub fn new(config: UserBalanceCacheConfig) -> Self {
        Self {
            cache: None,
            trading_repo: None,
            config,
            metrics: Arc::new(UserBalanceCacheMetrics::default()),
        }
    }

    pub fn with_repository(
        config: UserBalanceCacheConfig,
        trading_repo: Arc<dyn TradingRepository>,
    ) -> Self {
        Self {
            cache: None,
            trading_repo: Some(trading_repo),
            config,
            metrics: Arc::new(UserBalanceCacheMetrics::default()),
        }
    }

    pub async fn with_redis_cache(
        config: UserBalanceCacheConfig,
        redis_config: crate::redis_cache::RedisCacheConfig,
        trading_repo: Option<Arc<dyn TradingRepository>>,
    ) -> Result<Self, TradingError> {
        let cache = if config.enabled {
            let redis_cache = crate::redis_cache::RedisCache::<UserBalanceModel>::new(redis_config).await?;
            Some(Arc::new(redis_cache) as Arc<dyn Cache<UserBalanceModel>>)
        } else {
            None
        };

        Ok(Self {
            cache,
            trading_repo,
            config,
            metrics: Arc::new(UserBalanceCacheMetrics::default()),
        })
    }

    pub async fn get_user_balance_by_id(&self, user_id: Uuid) -> Result<Option<UserBalanceModel>, TradingError> {
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                match cache.get_by_id(&user_id).await {
                    Ok(Some(balance)) => {
                        self.metrics.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache hit for user balance ID: {}", user_id);
                        return Ok(Some(balance));
                    }
                    Ok(None) => {
                        self.metrics.cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache miss for user balance ID: {}", user_id);
                    }
                    Err(e) => {
                        self.metrics.cache_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!("Cache error for user balance ID {}: {}", user_id, e);
                    }
                }
            }
        }

        if let Some(repo) = &self.trading_repo {
            match repo.get_user_balance(user_id).await {
                Ok(Some(balance)) => {
                    self.metrics.db_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    
                    // Update cache if available
                    if self.config.enabled {
                        if let Some(cache) = &self.cache {
                            if let Err(e) = cache.set_by_id(&user_id, &balance, self.config.ttl).await {
                                warn!("Failed to update cache for user balance ID {}: {}", user_id, e);
                            }
                        }
                    }
                    
                    debug!("Database hit for user balance ID: {}", user_id);
                    return Ok(Some(balance));
                }
                Ok(None) => {
                    debug!("User balance not found in database: {}", user_id);
                    return Ok(None);
                }
                Err(e) => {
                    self.metrics.db_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(e);
                }
            }
        }

        debug!("User balance not found: {}", user_id);
        Ok(None)
    }


    pub async fn invalidate_user_balance(&self, user_id: Uuid) {
        // Remove from cache if available
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                if let Err(e) = cache.delete_by_id(&user_id).await {
                    warn!("Failed to invalidate cache for user balance ID {}: {}", user_id, e);
                }
            }
        }

        debug!("Invalidated cache for user balance: {}", user_id);
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStats {
        CacheStats {
            hits: self.metrics.cache_hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.metrics.cache_misses.load(std::sync::atomic::Ordering::Relaxed),
            errors: self.metrics.cache_errors.load(std::sync::atomic::Ordering::Relaxed),
            sets: 0, // Not tracked in this implementation
            deletes: 0, // Not tracked in this implementation
            size: None, // Not available
        }
    }
}
