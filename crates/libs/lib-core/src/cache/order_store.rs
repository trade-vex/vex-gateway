use crate::model::orders::Model as OrderModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct OrderStore {
    cache: Option<RedisDb>,
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
            Some(redis_db)
        } else {
            None
        };

        Ok(Self {
            cache,
            trading_repo,
            config,
        })
    }

    pub async fn get_order_by_id(
        &self,
        order_id: u64,
        user_id: u64,
    ) -> Result<Option<OrderModel>, TradingError> {
        // Cache: Check Redis hash: order:{order_id}
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let key = format!("order:{}", order_id);

                match cache.hgetall_string(&key).await {
                    Ok(hash) if !hash.is_empty() => {
                        // Parse hash to OrderModel
                        if let Some(order) = Self::parse_order_hash(hash) {
                            debug!("Order cache hit for order ID: {}", order_id);
                            return Ok(Some(order));
                        }
                    }
                    Ok(_) => {
                        debug!("Order cache miss for order ID: {}", order_id);
                    }
                    Err(e) => {
                        warn!("Order cache error for order ID {}: {}", order_id, e);
                    }
                }
            }
        }

        // Database: Check repository
        if let Some(repo) = &self.trading_repo {
            match repo.get_order_by_id(order_id, user_id).await {
                Ok(Some(order)) => {
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

    /// Get user's order history with caching (all orders including filled, cancelled, etc.)
    /// Uses Redis set user:{user_id}:orders -> fetches each order:{order_id} hash
    /// Unlike get_user_orders, this includes ALL orders regardless of status
    pub async fn get_user_order_history(
        &self,
        user_id: u64,
    ) -> Result<Vec<OrderModel>, TradingError> {
        // Cache: Query user:{user_id}:orders SET, then fetch each order hash
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let set_key = format!("user:{}:orders", user_id);

                println!(
                    "🔍 OrderStore: Querying Redis SET for order history: {}",
                    set_key
                );

                match cache.smembers_string(&set_key).await {
                    Ok(order_ids) if !order_ids.is_empty() => {
                        debug!(
                            "User order history cache hit for user: {} ({} orders in SET)",
                            user_id,
                            order_ids.len()
                        );
                        println!(
                            "   Found {} order IDs in SET: {:?}",
                            order_ids.len(),
                            order_ids
                        );

                        let mut orders = Vec::new();
                        // Fetch each order hash - include ALL orders regardless of status
                        for order_id_str in order_ids {
                            let order_key = format!("order:{}", order_id_str);
                            match cache.hgetall_string(&order_key).await {
                                Ok(hash) if !hash.is_empty() => {
                                    println!(
                                        "   Fetched hash for order {}: {:?}",
                                        order_id_str, hash
                                    );

                                    // Include ALL orders - don't filter by status
                                    if let Some(order) = Self::parse_order_hash(hash) {
                                        println!(
                                            "   ✓ Adding order {} to history results",
                                            order_id_str
                                        );
                                        orders.push(order);
                                    }
                                }
                                _ => continue,
                            }
                        }

                        println!("   Returning {} orders in history", orders.len());
                        return Ok(orders);
                    }
                    Ok(_) => {
                        debug!("User order history cache miss for user: {}", user_id);
                    }
                    Err(e) => {
                        warn!("User order history cache error for user {}: {}", user_id, e);
                    }
                }
            }
        }

        // Database fallback - get all orders (not just open ones)
        if let Some(repo) = &self.trading_repo {
            match repo.get_open_orders(user_id).await {
                Ok(orders) => {
                    debug!("Database hit for user order history: {}", user_id);
                    return Ok(orders);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        debug!("No repository available for user order history: {}", user_id);
        Ok(vec![])
    }

    /// Get user's open orders with caching
    /// Uses Redis set user:{user_id}:orders -> fetches each order:{order_id} hash
    pub async fn get_user_orders(&self, user_id: u64) -> Result<Vec<OrderModel>, TradingError> {
        // Cache: Query user:{user_id}:orders SET, then fetch each order hash
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let set_key = format!("user:{}:orders", user_id);

                println!("🔍 OrderStore: Querying Redis SET: {}", set_key);

                match cache.smembers_string(&set_key).await {
                    Ok(order_ids) if !order_ids.is_empty() => {
                        debug!(
                            "User orders cache hit for user: {} ({} orders in SET)",
                            user_id,
                            order_ids.len()
                        );
                        println!(
                            "   Found {} order IDs in SET: {:?}",
                            order_ids.len(),
                            order_ids
                        );

                        let mut orders = Vec::new();
                        // Fetch each order hash
                        for order_id_str in order_ids {
                            let order_key = format!("order:{}", order_id_str);
                            match cache.hgetall_string(&order_key).await {
                                Ok(hash) if !hash.is_empty() => {
                                    // Filter orders by status - only include "open" orders
                                    // Open orders: Pending, PartiallyFilled, Processing
                                    // Exclude: Filled, Cancelled, Rejected, Placed
                                    if let Some(status) = hash.get("status") {
                                        
                                        let status_lower = status.to_lowercase();
                                        
                                        // Skip orders that are not in "open" states
                                        if status_lower == "cancelled" 
                                            || status_lower == "filled" 
                                            || status_lower == "rejected" 
                                            || status_lower == "placed" {
                                            continue;
                                        }
                                        
                                        // Only include: Pending, PartiallyFilled, Processing
                                        if status_lower != "pending" 
                                            && status_lower != "partiallyfilled" 
                                            && status_lower != "processing" {
                                            continue;
                                        }
                                    }

                                    if let Some(order) = Self::parse_order_hash(hash) {
                                        orders.push(order);
                                    }
                                }
                                _ => continue,
                            }
                        }

                        println!("   Returning {} orders after filtering", orders.len());

                        if !orders.is_empty() {
                            return Ok(orders);
                        }

                        // If all orders were filtered out (all cancelled), return empty vec
                        println!("   All orders were cancelled - returning empty list");
                        return Ok(vec![]);
                    }
                    Ok(_) => {
                        debug!("User orders cache miss for user: {}", user_id);
                    }
                    Err(e) => {
                        warn!("User orders cache error for user {}: {}", user_id, e);
                    }
                }
            }
        }

        // Database fallback
        if let Some(repo) = &self.trading_repo {
            match repo.get_open_orders(user_id).await {
                Ok(orders) => {
                    debug!("Database hit for user orders: {}", user_id);
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

    pub async fn invalidate_order(&self, order_id: u64, user_id: u64) {
        if !self.config.enabled {
            return;
        }

        if let Some(cache) = &self.cache {
            // Delete order:{order_id} hash
            let order_key = format!("order:{}", order_id);
            if let Err(e) = cache.del_key(&order_key).await {
                warn!("Failed to delete order hash {}: {}", order_key, e);
            }

            // Remove from user:{user_id}:orders set
            let user_set_key = format!("user:{}:orders", user_id);
            if let Err(e) = cache
                .srem_string(&user_set_key, &order_id.to_string())
                .await
            {
                warn!(
                    "Failed to remove from user orders set {}: {}",
                    user_set_key, e
                );
            }

            debug!(
                "Invalidated cache for order: {} and user: {}",
                order_id, user_id
            );
        }
    }

    pub async fn invalidate_user_orders(&self, user_id: u64) {
        if !self.config.enabled {
            return;
        }

        if let Some(cache) = &self.cache {
            let user_set_key = format!("user:{}:orders", user_id);

            // Get all order IDs first
            match cache.smembers_string(&user_set_key).await {
                Ok(order_ids) => {
                    // Delete each order hash
                    for order_id_str in &order_ids {
                        let order_key = format!("order:{}", order_id_str);
                        if let Err(e) = cache.del_key(&order_key).await {
                            warn!("Failed to delete order hash {}: {}", order_key, e);
                        }
                    }

                    // Delete the set itself
                    if let Err(e) = cache.del_key(&user_set_key).await {
                        warn!("Failed to delete user orders set {}: {}", user_set_key, e);
                    }

                    debug!(
                        "Invalidated {} orders for user: {}",
                        order_ids.len(),
                        user_id
                    );
                }
                Err(e) => {
                    warn!("Failed to get order IDs for user {}: {}", user_id, e);
                }
            }
        }
    }

    // Helper: Parse order hash from Redis into OrderModel
    // Kafka writes: order_id, user_id, price, size, side, timestamp, market_id, time_in_force, status
    // Note: Redis stores price and size as decimal strings (e.g., "0.1", "0.001"), need to parse as f64 first
    fn parse_order_hash(hash: HashMap<String, String>) -> Option<OrderModel> {
        let order_order_id = hash.get("order_id")?.parse::<u64>().ok()?;
        let order_user_id = hash.get("user_id")?.parse::<u64>().ok()?;
        
        // Parse price - Redis stores as decimal string (e.g., "0.1"), but DB expects integer
        let price_str = hash.get("price")?;
        let order_price = if let Ok(price_i64) = price_str.parse::<i64>() {
            price_i64
        } else if let Ok(price_f64) = price_str.parse::<f64>() {
            // Convert decimal to integer with 12 decimal places precision
            (price_f64 * 1_000_000_000_000.0) as i64
        } else {
            return None;
        };
        
        // Parse size - same logic as price
        let size_str = hash.get("size")?;
        let order_size = if let Ok(size_i64) = size_str.parse::<i64>() {
            size_i64
        } else if let Ok(size_f64) = size_str.parse::<f64>() {
            (size_f64 * 1_000_000_000_000.0) as i64
        } else {
            return None;
        };
        
        let order_side = hash.get("side")?.clone();
        let order_timestamp = hash.get("timestamp")?.parse::<i64>().ok()?;
        let market_id = hash.get("market_id")?.parse::<i32>().ok()?;
        let time_in_force = hash.get("time_in_force")?.clone();
        let status = hash.get("status")?.clone();

        Some(OrderModel {
            id: 0, // Auto-generated by DB
            order_order_id,
            order_user_id,
            order_price,
            order_size,
            order_side,
            order_timestamp,
            market_id,
            time_in_force,
            status,
            created_at: None,
        })
    }
}

