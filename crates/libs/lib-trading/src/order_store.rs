use crate::cache::Cache;
use crate::repository::{TradingRepository, TradingError};
use crate::orders::Model as OrderModel;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

/// Statistics for individual order cache
#[derive(Debug, Clone)]
pub struct IndividualOrderCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub errors: u64,
}

/// Statistics for user orders cache
#[derive(Debug, Clone)]
pub struct UserOrdersCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub errors: u64,
}

/// Statistics for database operations
#[derive(Debug, Clone)]
pub struct DatabaseStats {
    pub hits: u64,
    pub errors: u64,
}

/// Combined statistics for OrderStore
#[derive(Debug, Clone)]
pub struct OrderCacheStats {
    pub individual_cache: IndividualOrderCacheStats,
    pub user_orders_cache: UserOrdersCacheStats,
    pub database: DatabaseStats,
}

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
    // Individual order cache stats
    pub individual_cache_hits: std::sync::atomic::AtomicU64,
    pub individual_cache_misses: std::sync::atomic::AtomicU64,
    pub individual_cache_errors: std::sync::atomic::AtomicU64,
    
    // User orders cache stats
    pub user_orders_cache_hits: std::sync::atomic::AtomicU64,
    pub user_orders_cache_misses: std::sync::atomic::AtomicU64,
    pub user_orders_cache_errors: std::sync::atomic::AtomicU64,
    
    // Database stats (shared)
    pub db_hits: std::sync::atomic::AtomicU64,
    pub db_errors: std::sync::atomic::AtomicU64,
}

impl Default for OrderCacheMetrics {
    fn default() -> Self {
        Self {
            individual_cache_hits: std::sync::atomic::AtomicU64::new(0),
            individual_cache_misses: std::sync::atomic::AtomicU64::new(0),
            individual_cache_errors: std::sync::atomic::AtomicU64::new(0),
            user_orders_cache_hits: std::sync::atomic::AtomicU64::new(0),
            user_orders_cache_misses: std::sync::atomic::AtomicU64::new(0),
            user_orders_cache_errors: std::sync::atomic::AtomicU64::new(0),
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
                match cache.get_by_id(&order_id.to_string()).await {
                    Ok(Some(order)) => {
                        self.metrics.individual_cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Individual order cache hit for order ID: {}", order_id);
                        return Ok(Some(order));
                    }
                    Ok(None) => {
                        self.metrics.individual_cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("Individual order cache miss for order ID: {}", order_id);
                    }
                    Err(e) => {
                        self.metrics.individual_cache_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!("Individual order cache error for order ID {}: {}", order_id, e);
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
                            if let Err(e) = cache.set_by_id(&order_id.to_string(), &order, self.config.ttl).await {
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
                match cache.get_by_id(&user_id.to_string()).await {
                    Ok(Some(orders)) => {
                        self.metrics.user_orders_cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("User orders cache hit for user: {}", user_id);
                        return Ok(orders);
                    }
                    Ok(None) => {
                        self.metrics.user_orders_cache_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug!("User orders cache miss for user: {}", user_id);
                    }
                    Err(e) => {
                        self.metrics.user_orders_cache_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        warn!("User orders cache error for user {}: {}", user_id, e);
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
                            if let Err(e) = cache.set_by_id(&user_id.to_string(), &orders, self.config.ttl).await {
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
                if let Err(e) = cache.delete_by_id(&order_id.to_string()).await {
                    warn!("Failed to invalidate cache for order ID {}: {}", order_id, e);
                }
            }
            
            if let Some(cache) = &self.user_orders_cache {
                if let Err(e) = cache.delete_by_id(&user_id.to_string()).await {
                    warn!("Failed to invalidate user orders cache for user {}: {}", user_id, e);
                }
            }
        }

        debug!("Invalidated cache for order: {} and user orders: {}", order_id, user_id);
    }

    pub async fn invalidate_user_orders(&self, user_id: Uuid) {
        if self.config.enabled {
            if let Some(cache) = &self.user_orders_cache {
                if let Err(e) = cache.delete_by_id(&user_id.to_string()).await {
                    warn!("Failed to invalidate user orders cache for user {}: {}", user_id, e);
                }
            }
        }

        debug!("Invalidated user orders cache for user: {}", user_id);
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> OrderCacheStats {
        OrderCacheStats {
            individual_cache: IndividualOrderCacheStats {
                hits: self.metrics.individual_cache_hits.load(std::sync::atomic::Ordering::Relaxed),
                misses: self.metrics.individual_cache_misses.load(std::sync::atomic::Ordering::Relaxed),
                errors: self.metrics.individual_cache_errors.load(std::sync::atomic::Ordering::Relaxed),
            },
            user_orders_cache: UserOrdersCacheStats {
                hits: self.metrics.user_orders_cache_hits.load(std::sync::atomic::Ordering::Relaxed),
                misses: self.metrics.user_orders_cache_misses.load(std::sync::atomic::Ordering::Relaxed),
                errors: self.metrics.user_orders_cache_errors.load(std::sync::atomic::Ordering::Relaxed),
            },
            database: DatabaseStats {
                hits: self.metrics.db_hits.load(std::sync::atomic::Ordering::Relaxed),
                errors: self.metrics.db_errors.load(std::sync::atomic::Ordering::Relaxed),
            },
        }
    }
}
