use crate::repository::repository::{TradingRepository, TradingError};
use crate::model::user_balances::Model as UserBalanceModel;
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::{BasicKvStore, ExpiringKvStore};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;


#[derive(Clone)]
pub struct UserBalanceStore {
    cache: Option<RedisDb>,
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
        redis_url: String,
        trading_repo: Option<Arc<dyn TradingRepository>>,
    ) -> Result<Self, TradingError> {
        let cache = if config.enabled {
            let redis_db = RedisDb::connect(&redis_url).await
                .map_err(|e| TradingError::Database(format!("Failed to connect to Redis: {}", e)))?;
            Some(redis_db)
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

    pub async fn get_user_balances(&self, user_id: Uuid) -> Result<Vec<UserBalanceModel>, TradingError> {
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let key = format!("user_balances:{}", user_id);
                match cache.get(&key).await {
                    Ok(Some(balances)) => {
                        self.metrics.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache hit for user balances ID: {}", user_id);
                        return Ok(balances);
                    }
                    Ok(None) => {
                        self.metrics.cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache miss for user balances ID: {}", user_id);
                    }
                    Err(e) => {
                        self.metrics.cache_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!("Cache error for user balances ID {}: {}", user_id, e);
                    }
                }
            }
        }

        if let Some(repo) = &self.trading_repo {
            match repo.get_user_balances(user_id).await {
                Ok(balances) => {
                    self.metrics.db_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    
                    // Update cache if available
                    if self.config.enabled {
                        if let Some(cache) = &self.cache {
                            let key = format!("user_balances:{}", user_id);
                            let ttl_secs = self.config.ttl.as_secs();
                            if let Err(e) = cache.set_ex(&key, &balances, ttl_secs).await {
                                warn!("Failed to update cache for user balances ID {}: {}", user_id, e);
                            }
                        }
                    }
                    
                    debug!("Database hit for user balances ID: {}", user_id);
                    return Ok(balances);
                }
                Err(e) => {
                    self.metrics.db_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(e);
                }
            }
        }

        debug!("User balances not found: {}", user_id);
        Ok(vec![])
    }

    pub async fn invalidate_user_balances(&self, user_id: Uuid) {
        // Remove from cache if available
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let key = format!("user_balances:{}", user_id);
                if let Err(e) = cache.delete(&key).await {
                    warn!("Failed to invalidate cache for user balances ID {}: {}", user_id, e);
                }
            }
        }

        debug!("Invalidated cache for user balances: {}", user_id);
    }
}
