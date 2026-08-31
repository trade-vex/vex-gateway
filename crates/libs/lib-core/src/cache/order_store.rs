use crate::cache::scaling_cache::ScalingCache;
use crate::model::orders::Model as OrderModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, warn};

#[derive(Debug, Clone, PartialEq)]
pub struct OrderData {
    pub market_id: i32,
    pub order_order_id: i64,
    pub order_user_id: i64,
    pub order_price: u64,
    pub order_size: u64,
    pub order_filled_size: u64,
    pub order_side: String,
    pub order_time_in_force: String,
    pub order_status: String,
    pub order_timestamp: i64,
}

impl From<OrderModel> for OrderData {
    fn from(model: OrderModel) -> Self {
        Self {
            market_id: model.market_id,
            order_order_id: model.order_order_id,
            order_user_id: model.order_user_id,
            order_price: model.order_price as u64,
            order_size: model.order_size as u64,
            order_filled_size: model.order_filled_size as u64,
            order_side: model.order_side,
            order_time_in_force: model.order_time_in_force,
            order_status: model.order_status,
            order_timestamp: model.order_timestamp,
        }
    }
}

// Status and time-in-force values are vex-core's protobuf enum value names (trading.proto
// `Status`, `TimeInForce`), which the Kafka Connect sinks write verbatim to Redis and Postgres.
// These match the repository's OPEN_ORDER_FILTER and HISTORY_ORDER_FILTER.

/// Order history: cancelled, partially filled and filled orders.
fn is_history_status(status: &str) -> bool {
    matches!(
        status,
        "STATUS_CANCELLED" | "STATUS_PARTIALLY_FILLED" | "STATUS_FILLED"
    )
}

/// Open orders: placed and processing ones, and partially filled ones except IOC, whose
/// remainder the engine cancels.
fn is_open_order(status: &str, time_in_force: Option<&str>) -> bool {
    match status {
        "STATUS_PLACED" | "STATUS_PROCESSING" => true,
        "STATUS_PARTIALLY_FILLED" => time_in_force != Some("TIF_IOC"),
        _ => false,
    }
}

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

    /// Convert the stored status (STATUS_*) to the API's form
    fn normalize_status(status: &str) -> String {
        match status {
            "STATUS_REJECTED" => "Rejected",
            "STATUS_PLACED" => "Placed",
            "STATUS_CANCELLED" => "Cancelled",
            "STATUS_PARTIALLY_FILLED" => "PartiallyFilled",
            "STATUS_FILLED" => "Filled",
            "STATUS_PROCESSING" => "Processing",
            "STATUS_PROCESSED" => "Processed",
            // An unknown value is passed through rather than guessed at.
            other => other,
        }
        .to_string()
    }

    /// The stored side, or `None` (logged) when it is not one of core's `Side` names. Such a
    /// record is skipped, as `parse_order_hash` skips one whose other stored fields do not parse:
    /// guessing a side would show the user an order on the wrong side of the book. The handlers'
    /// `total` comes from the repository's count, so a page holding a skipped record is short.
    fn parse_side(model: &OrderData) -> Option<crate::dto::trading::Side> {
        match model.order_side.parse() {
            Ok(side) => Some(side),
            Err(_) => {
                error!(
                    order_id = model.order_order_id,
                    order_side = %model.order_side,
                    "Skipping stored order with an unknown order_side"
                );
                None
            }
        }
    }

    /// Convert order data to OrderResponse with scaling applied (open orders: size = remaining).
    /// `None` when the stored side is unknown; see `parse_side`.
    pub fn to_response(
        &self,
        model: impl Into<OrderData>,
    ) -> Option<crate::dto::trading::OrderResponse> {
        use crate::dto::trading::OrderResponse;
        use rust_decimal::Decimal;

        let model = model.into();
        let side = Self::parse_side(&model)?;
        let market_id = model.market_id as u32;
        let remaining = model.order_size.saturating_sub(model.order_filled_size);

        // Apply scaling if scaling cache is available
        let (price, size) = if let Some(scaling_cache) = &self.scaling_cache {
            let price_decimal =
                scaling_cache.scale_price(market_id, Decimal::from(model.order_price));
            let size_decimal = scaling_cache.scale_volume(market_id, Decimal::from(remaining));
            (price_decimal, size_decimal)
        } else {
            (Decimal::from(model.order_price), Decimal::from(remaining))
        };

        Some(OrderResponse {
            order_id: model.order_order_id,
            user_id: model.order_user_id,
            market_id: model.market_id,
            price,
            size,
            side,
            timestamp: model.order_timestamp,
            time_in_force: model.order_time_in_force,
            status: Self::normalize_status(&model.order_status),
        })
    }

    /// Convert order data to OrderHistoryResponse with scaling applied.
    /// `None` when the stored side is unknown; see `parse_side`.
    pub fn to_history_response(
        &self,
        model: impl Into<OrderData>,
    ) -> Option<crate::dto::trading::OrderHistoryResponse> {
        use crate::dto::trading::OrderHistoryResponse;
        use rust_decimal::Decimal;

        let model = model.into();
        let side = Self::parse_side(&model)?;
        let market_id = model.market_id as u32;

        // Apply scaling if scaling cache is available (size = original, filled = order_filled_size)
        let (price, size, filled) = if let Some(scaling_cache) = &self.scaling_cache {
            let price_decimal =
                scaling_cache.scale_price(market_id, Decimal::from(model.order_price));
            let size_decimal =
                scaling_cache.scale_volume(market_id, Decimal::from(model.order_size));
            let filled_decimal =
                scaling_cache.scale_volume(market_id, Decimal::from(model.order_filled_size));
            (price_decimal, size_decimal, filled_decimal)
        } else {
            (
                Decimal::from(model.order_price),
                Decimal::from(model.order_size),
                Decimal::from(model.order_filled_size),
            )
        };

        Some(OrderHistoryResponse {
            order_id: model.order_order_id,
            user_id: model.order_user_id,
            market_id: model.market_id,
            price,
            size,
            filled,
            side,
            timestamp: model.order_timestamp,
            time_in_force: model.order_time_in_force,
            status: Self::normalize_status(&model.order_status),
        })
    }

    pub async fn get_order_by_id(
        &self,
        order_id: u64,
        user_id: u64,
    ) -> Result<Option<OrderData>, TradingError> {
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
                    return Ok(Some(order.into()));
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

    /// Get user's order history from orders table (paginated)
    /// Returns Cancelled, PartiallyFilled, and Filled orders
    pub async fn get_user_order_history_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderData>, TradingError> {
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
                                    if let Some(status) = hash.get("order_status") {
                                        if is_history_status(status) {
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
                            return Ok(paginated_orders);
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
            return Ok(orders.into_iter().map(Into::into).collect());
        }

        debug!(
            "No repository available for user order history: {}",
            user_id
        );
        Ok(vec![])
    }

    /// Get user's order history from orders table
    /// Returns Cancelled, PartiallyFilled, and Filled orders
    pub async fn get_user_order_history(
        &self,
        user_id: u64,
    ) -> Result<Vec<OrderData>, TradingError> {
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
                                    if let Some(status) = hash.get("order_status") {
                                        if is_history_status(status) {
                                            if let Some(order) = Self::parse_order_hash(hash) {
                                                orders.push(order);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if !orders.is_empty() {
                            debug!("Redis cache hit: {} history orders", orders.len());
                            return Ok(orders);
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

        // Database fallback - Query orders_events table
        if let Some(repo) = &self.trading_repo {
            let orders = repo.get_order_history(user_id).await?;
            debug!("Postgres: Got {} history orders", orders.len());
            return Ok(orders.into_iter().map(Into::into).collect());
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
    ) -> Result<Vec<OrderData>, TradingError> {
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
                                    if let Some(status) = hash.get("order_status") {
                                        let time_in_force =
                                            hash.get("order_time_in_force").map(String::as_str);
                                        if !is_open_order(status, time_in_force) {
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
                            return Ok(paginated_orders);
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
                    return Ok(orders.into_iter().map(Into::into).collect());
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
    pub async fn get_user_orders(&self, user_id: u64) -> Result<Vec<OrderData>, TradingError> {
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
                                    if let Some(status) = hash.get("order_status") {
                                        let time_in_force =
                                            hash.get("order_time_in_force").map(String::as_str);
                                        if !is_open_order(status, time_in_force) {
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
                            return Ok(orders);
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
                    return Ok(orders.into_iter().map(Into::into).collect());
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        debug!("No repository available for user orders: {}", user_id);
        Ok(vec![])
    }

    /// Parse order hash from Redis into OrderData
    /// Redis keys from Kafka sink: orders:{order_id}
    /// Fields: market_id, order_order_id, order_user_id, order_price, order_size, order_filled_size,
    ///   order_side, order_time_in_force, order_status, order_timestamp
    fn parse_order_hash(hash: HashMap<String, String>) -> Option<OrderData> {
        // Parse with order_ prefix (from Kafka Flatten transform)
        let order_order_id = hash.get("order_order_id")?.parse::<i64>().ok()?;
        let order_user_id = hash.get("order_user_id")?.parse::<i64>().ok()?;
        let order_price = hash.get("order_price")?.parse::<u64>().ok()?;
        let order_size = hash.get("order_size")?.parse::<u64>().ok()?;
        let order_filled_size = hash
            .get("order_filled_size")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let order_side = hash.get("order_side")?.clone();
        let order_timestamp = hash.get("order_timestamp")?.parse::<i64>().ok()?;
        let market_id = hash.get("market_id")?.parse::<i32>().ok()?;
        let order_time_in_force = hash.get("order_time_in_force")?.clone();
        let order_status = hash.get("order_status")?.clone();

        Some(OrderData {
            market_id,
            order_order_id,
            order_user_id,
            order_price,
            order_size,
            order_filled_size,
            order_side,
            order_time_in_force,
            order_status,
            order_timestamp,
        })
    }
}
