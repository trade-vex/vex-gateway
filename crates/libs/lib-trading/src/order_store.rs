use crate::cache::{Cache, CacheStats};
use crate::repository::{TradingRepository, TradingError};
use crate::orders::Model as OrderModel;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

#[derive(Clone)]
pub struct OrderStore {
    cache: Option<Arc<dyn Cache<OrderModel>>>,
    user_orders_cache: Option<Arc<dyn Cache<Vec<OrderModel>>>>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: OrderCacheConfig,
    metrics: Arc<OrderCacheMetrics>,
}

#[derive(Clone)]
pub struct OrderCacheConfig {
    pub ttl: Duration,
    pub enabled: bool,
}

impl Default for OrderCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(1800),
            enabled: false,
        }
    }
}

pub struct OrderCacheMetrics {
    pub cache_hits: std::sync::atomic::AtomicU64,
    pub cache_misses: std::sync::atomic::AtomicU64,
    pub cache_errors: std::sync::atomic::AtomicU64,
    pub db_hits: std::sync::atomic::AtomicU64,
    pub db_errors: std::sync::atomic::AtomicU64,
}

impl Default for OrderCacheMetrics {
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

impl OrderStore {
    pub fn new(config: OrderCacheConfig) -> Self {
        Self {
            cache: None,
            user_orders_cache: None,
            trading_repo: None,
            config,
            metrics: Arc::new(OrderCacheMetrics::default()),
        }
    }

    pub fn with_repository(
        config: OrderCacheConfig,
        trading_repo: Arc<dyn TradingRepository>,
    ) -> Self {
        Self {
            cache: None,
            user_orders_cache: None,
            trading_repo: Some(trading_repo),
            config,
            metrics: Arc::new(OrderCacheMetrics::default()),
        }
    }

    pub async fn with_redis_cache(
        config: OrderCacheConfig,
        redis_config: crate::redis_cache::RedisCacheConfig,
        trading_repo: Option<Arc<dyn TradingRepository>>,
    ) -> Result<Self, TradingError> {
        let cache = if config.enabled {
            let redis_cache = crate::redis_cache::RedisCache::<OrderModel>::new(redis_config.clone()).await?;
            Some(Arc::new(redis_cache) as Arc<dyn Cache<OrderModel>>)
        } else {
            None
        };

        let user_orders_cache = if config.enabled {
            let redis_cache = crate::redis_cache::RedisCache::<Vec<OrderModel>>::new(redis_config).await?;
            Some(Arc::new(redis_cache) as Arc<dyn Cache<Vec<OrderModel>>>)
        } else {
            None
        };

        Ok(Self {
            cache,
            user_orders_cache,
            trading_repo,
            config,
            metrics: Arc::new(OrderCacheMetrics::default()),
        })
    }

    pub async fn get_order_by_id(&self, order_id: u64, user_id: Uuid) -> Result<Option<OrderModel>, TradingError> {
        // Cache: Check Redis if available
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                match cache.get_by_id(&user_id).await {
                    Ok(Some(order)) => {
                        self.metrics.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache hit for order ID: {}", order_id);
                        return Ok(Some(order));
                    }
                    Ok(None) => {
                        self.metrics.cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache miss for order ID: {}", order_id);
                    }
                    Err(e) => {
                        self.metrics.cache_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!("Cache error for order ID {}: {}", order_id, e);
                    }
                }
            }
        }

        // Database: Check repository
        if let Some(repo) = &self.trading_repo {
            match repo.get_order_by_id(order_id, user_id).await {
                Ok(Some(order)) => {
                    self.metrics.db_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    
                    // Update cache if available
                    if self.config.enabled {
                        if let Some(cache) = &self.cache {
                            if let Err(e) = cache.set_by_id(&user_id, &order, self.config.ttl).await {
                                warn!("Failed to update cache for order ID {}: {}", order_id, e);
                            }
                        }
                    }
                    
                    debug!("Database hit for order ID: {}", order_id);
                    return Ok(Some(order));
                }
                Ok(None) => {
                    debug!("Order not found in database: {}", order_id);
                    return Ok(None);
                }
                Err(e) => {
                    self.metrics.db_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(e);
                }
            }
        }

        debug!("Order not found: {}", order_id);
        Ok(None)
    }

    /// Get user's open orders with caching
    pub async fn get_user_orders(&self, user_id: Uuid) -> Result<Vec<OrderModel>, TradingError> {
        // Cache: Check Redis if available
        if self.config.enabled {
            if let Some(cache) = &self.user_orders_cache {
                match cache.get_by_id(&user_id).await {
                    Ok(Some(orders)) => {
                        self.metrics.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache hit for user orders: {}", user_id);
                        return Ok(orders);
                    }
                    Ok(None) => {
                        self.metrics.cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Cache miss for user orders: {}", user_id);
                    }
                    Err(e) => {
                        self.metrics.cache_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!("Cache error for user orders {}: {}", user_id, e);
                    }
                }
            }
        }

        if let Some(repo) = &self.trading_repo {
            match repo.get_open_orders(user_id).await {
                Ok(orders) => {
                    self.metrics.db_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    debug!("Database hit for user orders: {}", user_id);
                    
                    // Cache: Store in Redis if available
                    if self.config.enabled {
                        if let Some(cache) = &self.user_orders_cache {
                            if let Err(e) = cache.set_by_id(&user_id, &orders, self.config.ttl).await {
                                warn!("Failed to cache user orders for user {}: {}", user_id, e);
                            }
                        }
                    }
                    
                    return Ok(orders);
                }
                Err(e) => {
                    self.metrics.db_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Err(e);
                }
            }
        }

        debug!("No repository available for user orders: {}", user_id);
        Ok(vec![])
    }

    pub async fn invalidate_order(&self, order_id: u64, user_id: Uuid) {
        // Remove from cache if available
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                if let Err(e) = cache.delete_by_id(&user_id).await {
                    warn!("Failed to invalidate cache for order ID {}: {}", order_id, e);
                }
            }
            
            if let Some(cache) = &self.user_orders_cache {
                if let Err(e) = cache.delete_by_id(&user_id).await {
                    warn!("Failed to invalidate user orders cache for user {}: {}", user_id, e);
                }
            }
        }

        debug!("Invalidated cache for order: {} and user orders: {}", order_id, user_id);
    }

    pub async fn invalidate_user_orders(&self, user_id: Uuid) {
        if self.config.enabled {
            if let Some(cache) = &self.user_orders_cache {
                if let Err(e) = cache.delete_by_id(&user_id).await {
                    warn!("Failed to invalidate user orders cache for user {}: {}", user_id, e);
                }
            }
        }

        debug!("Invalidated user orders cache for user: {}", user_id);
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
