use crate::model::orders::Model as OrderModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::{BasicKvStore, ExpiringKvStore};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

#[derive(Clone)]
pub struct OrderStore {
    cache: Option<RedisDb>,
    user_orders_cache: Option<RedisDb>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: OrderCacheConfig,
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

impl OrderStore {
    pub fn new(config: OrderCacheConfig) -> Self {
        Self {
            cache: None,
            user_orders_cache: None,
            trading_repo: None,
            config,
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
        }
    }

    pub async fn with_redis_cache(
        config: OrderCacheConfig,
        redis_url: String,
        trading_repo: Option<Arc<dyn TradingRepository>>,
    ) -> Result<Self, TradingError> {
        let cache = if config.enabled {
            let redis_db = RedisDb::connect(&redis_url).await.map_err(|e| {
                TradingError::Database(format!("Failed to connect to Redis: {}", e))
            })?;
            Some(redis_db.clone())
        } else {
            None
        };

        let user_orders_cache = if config.enabled {
            let redis_db = RedisDb::connect(&redis_url).await.map_err(|e| {
                TradingError::Database(format!("Failed to connect to Redis: {}", e))
            })?;
            Some(redis_db)
        } else {
            None
        };

        Ok(Self {
            cache,
            user_orders_cache,
            trading_repo,
            config,
        })
    }

    pub async fn get_order_by_id(
        &self,
        order_id: u64,
        user_id: Uuid,
    ) -> Result<Option<OrderModel>, TradingError> {
        // Cache: Check Redis if available
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let key = format!("order:{}", order_id);
                match cache.get(&key).await {
                    Ok(Some(order)) => {
                        debug!("Individual order cache hit for order ID: {}", order_id);
                        return Ok(Some(order));
                    }
                    Ok(None) => {
                        debug!("Individual order cache miss for order ID: {}", order_id);
                    }
                    Err(e) => {
                        warn!(
                            "Individual order cache error for order ID {}: {}",
                            order_id, e
                        );
                    }
                }
            }
        }

        // Database: Check repository
        if let Some(repo) = &self.trading_repo {
            match repo.get_order_by_id(order_id, user_id).await {
                Ok(Some(order)) => {
                    // Update cache if available
                    if self.config.enabled {
                        if let Some(cache) = &self.cache {
                            let key = format!("order:{}", order_id);
                            let ttl_secs = self.config.ttl.as_secs();
                            if let Err(e) = cache.set_ex(&key, &order, ttl_secs).await {
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
                let key = format!("user_orders:{}", user_id);
                match cache.get(&key).await {
                    Ok(Some(orders)) => {
                        debug!("User orders cache hit for user: {}", user_id);
                        return Ok(orders);
                    }
                    Ok(None) => {
                        debug!("User orders cache miss for user: {}", user_id);
                    }
                    Err(e) => {
                        warn!("User orders cache error for user {}: {}", user_id, e);
                    }
                }
            }
        }

        if let Some(repo) = &self.trading_repo {
            match repo.get_open_orders(user_id).await {
                Ok(orders) => {
                    debug!("Database hit for user orders: {}", user_id);

                    // Cache: Store in Redis if available
                    if self.config.enabled {
                        if let Some(cache) = &self.user_orders_cache {
                            let key = format!("user_orders:{}", user_id);
                            let ttl_secs = self.config.ttl.as_secs();
                            if let Err(e) = cache.set_ex(&key, &orders, ttl_secs).await {
                                warn!("Failed to cache user orders for user {}: {}", user_id, e);
                            }
                        }
                    }

                    return Ok(orders);
                }
                Err(e) => {
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
                let key = format!("order:{}", order_id);
                if let Err(e) = cache.delete(&key).await {
                    warn!(
                        "Failed to invalidate cache for order ID {}: {}",
                        order_id, e
                    );
                }
            }

            if let Some(cache) = &self.user_orders_cache {
                let key = format!("user_orders:{}", user_id);
                if let Err(e) = cache.delete(&key).await {
                    warn!(
                        "Failed to invalidate user orders cache for user {}: {}",
                        user_id, e
                    );
                }
            }
        }

        debug!(
            "Invalidated cache for order: {} and user orders: {}",
            order_id, user_id
        );
    }

    pub async fn invalidate_user_orders(&self, user_id: Uuid) {
        if self.config.enabled {
            if let Some(cache) = &self.user_orders_cache {
                let key = format!("user_orders:{}", user_id);
                if let Err(e) = cache.delete(&key).await {
                    warn!(
                        "Failed to invalidate user orders cache for user {}: {}",
                        user_id, e
                    );
                }
            }
        }

        debug!("Invalidated user orders cache for user: {}", user_id);
    }
}
