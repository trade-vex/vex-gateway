use crate::cache::scaling_cache::ScalingCache;
use crate::model::orders::Model as OrderModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct OrderStore {
    cache: Option<RedisDb>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: OrderCacheConfig,
    scaling_cache: Option<Arc<ScalingCache>>,
}

#[derive(Clone, Default)]
pub struct OrderCacheConfig {
    pub enabled: bool,
}

impl OrderStore {
    pub fn new(config: OrderCacheConfig) -> Self {
        Self {
            cache: None,
            trading_repo: None,
            config,
            scaling_cache: None,
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
            scaling_cache: None,
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
            scaling_cache: None,
        })
    }

    pub fn with_scaling_cache(mut self, scaling_cache: Arc<ScalingCache>) -> Self {
        self.scaling_cache = Some(scaling_cache);
        self
    }

    /// Descale an order model using the scaling cache
    fn descale_order(&self, mut order: OrderModel) -> OrderModel {
        if let Some(scaling_cache) = &self.scaling_cache {
            order.order_price =
                scaling_cache.scale_price(order.market_id as u32, order.order_price);
            order.order_size = scaling_cache.scale_volume(order.market_id as u32, order.order_size);
            order.filled_size =
                scaling_cache.scale_volume(order.market_id as u32, order.filled_size);
        }
        order
    }

    /// Descale a vector of orders
    fn descale_orders(&self, orders: Vec<OrderModel>) -> Vec<OrderModel> {
        if let Some(scaling_cache) = &self.scaling_cache {
            orders
                .into_iter()
                .map(|order| {
                    let mut descaled = order.clone();
                    descaled.order_price =
                        scaling_cache.scale_price(descaled.market_id as u32, descaled.order_price);
                    descaled.order_size =
                        scaling_cache.scale_volume(descaled.market_id as u32, descaled.order_size);
                    descaled.filled_size =
                        scaling_cache.scale_volume(descaled.market_id as u32, descaled.filled_size);
                    descaled
                })
                .collect()
        } else {
            orders
        }
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
                            return Ok(Some(self.descale_order(order)));
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
                    return Ok(Some(self.descale_order(order)));
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

    /// Get user's order history with filled_size from orders table only (paginated)
    /// Returns Cancelled, PartiallyFilled, and Filled orders
    pub async fn get_user_order_history_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>, TradingError> {
        // Try Redis first if cache enabled
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let set_key = format!("user:{}:orders", user_id);
                debug!("Redis: Querying orders from SET: {}", set_key);

                match cache.smembers_string(&set_key).await {
                    Ok(order_ids) if !order_ids.is_empty() => {
                        let mut orders = Vec::new();
                        for order_id_str in &order_ids {
                            let order_key = format!("order:{}", order_id_str);
                            if let Ok(hash) = cache.hgetall_string(&order_key).await {
                                if !hash.is_empty() {
                                    // Only include Cancelled, PartiallyFilled, Filled
                                    if let Some(status) = hash.get("status") {
                                        let status_lower = status.to_lowercase();
                                        if status_lower == "cancelled"
                                            || status_lower == "partiallyfilled"
                                            || status_lower == "filled"
                                        {
                                            if let Some(order) = Self::parse_order_hash(hash) {
                                                // Filter by market_id if provided
                                                if let Some(filter_market_id) = market_id {
                                                    if order.market_id as u32 != filter_market_id {
                                                        continue;
                                                    }
                                                }
                                                orders.push(order);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Sort by timestamp descending and apply pagination
                        orders.sort_by(|a, b| b.order_timestamp.cmp(&a.order_timestamp));
                        let start = offset as usize;
                        let end = (offset + limit) as usize;
                        let paginated_orders: Vec<_> = orders
                            .into_iter()
                            .skip(start)
                            .take((end - start).min(limit as usize))
                            .collect();

                        if !paginated_orders.is_empty() {
                            debug!(
                                "Redis cache hit: {} history orders (paginated)",
                                paginated_orders.len()
                            );
                            return Ok(self.descale_orders(paginated_orders));
                        }
                    }
                    Ok(_) => {
                        debug!("Redis cache miss for user orders: {}", user_id);
                    }
                    Err(e) => {
                        warn!("Redis error for user {}: {}", user_id, e);
                    }
                }
            }
        }

        // Database fallback
        if let Some(repo) = &self.trading_repo {
            let orders = repo
                .get_order_history_paginated(user_id, offset, limit, market_id)
                .await?;
            debug!("Postgres: Got {} history orders (paginated)", orders.len());
            return Ok(self.descale_orders(orders));
        }

        debug!(
            "No repository available for user order history: {}",
            user_id
        );
        Ok(vec![])
    }

    /// Get user's order history with filled_size from orders table only
    /// Returns Cancelled, PartiallyFilled, and Filled orders
    pub async fn get_user_order_history(
        &self,
        user_id: u64,
    ) -> Result<Vec<OrderModel>, TradingError> {
        // Try Redis first if cache enabled
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let set_key = format!("user:{}:orders", user_id);
                debug!("Redis: Querying orders from SET: {}", set_key);

                match cache.smembers_string(&set_key).await {
                    Ok(order_ids) if !order_ids.is_empty() => {
                        let mut orders = Vec::new();
                        for order_id_str in &order_ids {
                            let order_key = format!("order:{}", order_id_str);
                            if let Ok(hash) = cache.hgetall_string(&order_key).await {
                                if !hash.is_empty() {
                                    // Only include Cancelled, PartiallyFilled, Filled
                                    if let Some(status) = hash.get("status") {
                                        let status_lower = status.to_lowercase();
                                        if status_lower == "cancelled"
                                            || status_lower == "partiallyfilled"
                                            || status_lower == "filled"
                                        {
                                            if let Some(order) = Self::parse_order_hash(hash) {
                                                orders.push(order);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if !orders.is_empty() {
                            debug!(
                                "Redis cache hit: {} history orders (includes filled_size)",
                                orders.len()
                            );
                            return Ok(self.descale_orders(orders));
                        }
                    }
                    Ok(_) => {
                        debug!("Redis cache miss for user orders: {}", user_id);
                    }
                    Err(e) => {
                        warn!("Redis error for user {}: {}", user_id, e);
                    }
                }
            }
        }

        // Database fallback - Query orders_events table only (filled_size is already there)
        if let Some(repo) = &self.trading_repo {
            let orders = repo.get_order_history(user_id).await?;
            debug!(
                "Postgres: Got {} history orders (includes filled_size)",
                orders.len()
            );
            return Ok(self.descale_orders(orders));
        }

        debug!(
            "No repository available for user order history: {}",
            user_id
        );
        Ok(vec![])
    }

    /// Get user's open orders with caching (paginated)
    /// Uses Redis set user:{user_id}:orders -> fetches each order:{order_id} hash
    pub async fn get_user_orders_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>, TradingError> {
        // Cache: Query user:{user_id}:orders SET, then fetch each order hash
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let set_key = format!("user:{}:orders", user_id);

                match cache.smembers_string(&set_key).await {
                    Ok(order_ids) if !order_ids.is_empty() => {
                        let mut orders = Vec::new();
                        // Fetch each order hash
                        for order_id_str in order_ids {
                            let order_key = format!("order:{}", order_id_str);
                            match cache.hgetall_string(&order_key).await {
                                Ok(hash) if !hash.is_empty() => {
                                    // Filter orders by status - only include "open" orders
                                    if let Some(status) = hash.get("status") {
                                        let status_lower = status.to_lowercase();

                                        // Skip orders that are in terminal states (not open)
                                        if status_lower == "cancelled"
                                            || status_lower == "filled"
                                            || status_lower == "rejected"
                                        {
                                            continue;
                                        }

                                        // Only include: Placed, Pending, PartiallyFilled, Processing
                                        if status_lower != "placed"
                                            && status_lower != "pending"
                                            && status_lower != "partiallyfilled"
                                            && status_lower != "processing"
                                        {
                                            continue;
                                        }
                                    }

                                    if let Some(order) = Self::parse_order_hash(hash) {
                                        // Filter by market_id if provided
                                        if let Some(filter_market_id) = market_id {
                                            if order.market_id as u32 != filter_market_id {
                                                continue;
                                            }
                                        }
                                        orders.push(order);
                                    }
                                }
                                _ => continue,
                            }
                        }

                        // Sort by timestamp descending and apply pagination
                        orders.sort_by(|a, b| b.order_timestamp.cmp(&a.order_timestamp));
                        let start = offset as usize;
                        let end = (offset + limit) as usize;
                        let paginated_orders: Vec<_> = orders
                            .into_iter()
                            .skip(start)
                            .take((end - start).min(limit as usize))
                            .collect();

                        if !paginated_orders.is_empty() {
                            return Ok(self.descale_orders(paginated_orders));
                        }
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
            match repo
                .get_open_orders_paginated(user_id, offset, limit, market_id)
                .await
            {
                Ok(orders) => {
                    debug!("Database hit for user orders (paginated): {}", user_id);
                    return Ok(self.descale_orders(orders));
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        debug!("No repository available for user orders: {}", user_id);
        Ok(vec![])
    }

    /// Get user's open orders with caching
    /// Uses Redis set user:{user_id}:orders -> fetches each order:{order_id} hash
    pub async fn get_user_orders(&self, user_id: u64) -> Result<Vec<OrderModel>, TradingError> {
        // Cache: Query user:{user_id}:orders SET, then fetch each order hash
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let set_key = format!("user:{}:orders", user_id);

                debug!("OrderStore: Querying Redis SET: {}", set_key);

                match cache.smembers_string(&set_key).await {
                    Ok(order_ids) if !order_ids.is_empty() => {
                        debug!(
                            "User orders cache hit for user: {} ({} orders in SET)",
                            user_id,
                            order_ids.len()
                        );
                        debug!(
                            "Found {} order IDs in SET: {:?}",
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
                                    // Open orders: Placed, Pending, PartiallyFilled, Processing
                                    // Exclude: Filled, Cancelled, Rejected
                                    if let Some(status) = hash.get("status") {
                                        let status_lower = status.to_lowercase();

                                        // Skip orders that are in terminal states (not open)
                                        if status_lower == "cancelled"
                                            || status_lower == "filled"
                                            || status_lower == "rejected"
                                        {
                                            continue;
                                        }

                                        // Only include: Placed, Pending, PartiallyFilled, Processing
                                        if status_lower != "placed"
                                            && status_lower != "pending"
                                            && status_lower != "partiallyfilled"
                                            && status_lower != "processing"
                                        {
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

                        debug!("Returning {} orders after filtering", orders.len());

                        if !orders.is_empty() {
                            return Ok(self.descale_orders(orders));
                        }

                        // If all orders were filtered out (all cancelled), return empty vec
                        debug!("All orders were cancelled - returning empty list");
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
                    return Ok(self.descale_orders(orders));
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
    // Kafka writes: order_id, user_id, price, size, filled_size, side, timestamp, market_id, time_in_force, status
    // Parse all values as Decimal to handle full u64 range without overflow
    fn parse_order_hash(hash: HashMap<String, String>) -> Option<OrderModel> {
        let order_order_id = Decimal::from_str(hash.get("order_id")?).ok()?;
        let order_user_id = Decimal::from_str(hash.get("user_id")?).ok()?;
        let order_price = Decimal::from_str(hash.get("price")?).ok()?;
        let order_size = Decimal::from_str(hash.get("size")?).ok()?;
        let filled_size = Decimal::from_str(hash.get("filled_size").unwrap_or(&"0".to_string()))
            .unwrap_or(Decimal::ZERO);
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
            filled_size,
            order_side,
            order_timestamp,
            market_id,
            time_in_force,
            status,
            created_at: None,
        })
    }
}
