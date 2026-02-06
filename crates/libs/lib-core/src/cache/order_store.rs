use crate::cache::scaling_cache::ScalingCache;
use crate::model::orders::Model as OrderModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use std::collections::HashMap;
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

    /// Convert status from database format (STATUS_*) to user-friendly format
    fn normalize_status(status: &str) -> String {
        match status {
            "STATUS_FILLED" => "Filled".to_string(),
            "STATUS_CANCELLED" => "Cancelled".to_string(),
            "STATUS_PARTIALLY_FILLED" => "PartiallyFilled".to_string(),
            "STATUS_PLACED" => "Placed".to_string(),
            "STATUS_PENDING" => "Pending".to_string(),
            "STATUS_PROCESSING" => "Processing".to_string(),
            "STATUS_REJECTED" => "Rejected".to_string(),
            // Handle lowercase variants (from Redis cache)
            "filled" => "Filled".to_string(),
            "cancelled" => "Cancelled".to_string(),
            "partiallyfilled" => "PartiallyFilled".to_string(),
            "placed" => "Placed".to_string(),
            "pending" => "Pending".to_string(),
            "processing" => "Processing".to_string(),
            "rejected" => "Rejected".to_string(),
            // Return as-is if already in correct format or unknown
            _ => {
                // If it doesn't start with STATUS_, assume it's already normalized
                if status.starts_with("STATUS_") {
                    // Convert STATUS_XYZ to Xyz format
                    let without_prefix = status.strip_prefix("STATUS_").unwrap_or(status);
                    // Convert to PascalCase
                    let mut result = String::new();
                    let mut capitalize_next = true;
                    for ch in without_prefix.chars() {
                        if ch == '_' {
                            capitalize_next = true;
                        } else if capitalize_next {
                            result.push(ch.to_uppercase().next().unwrap_or(ch));
                            capitalize_next = false;
                        } else {
                            result.push(ch.to_lowercase().next().unwrap_or(ch));
                        }
                    }
                    result
                } else {
                    status.to_string()
                }
            }
        }
    }

    /// Convert OrderModel to OrderResponse with scaling applied
    pub fn to_response(&self, model: OrderModel) -> crate::dto::trading::OrderResponse {
        use crate::dto::trading::{OrderResponse, Side};
        use rust_decimal::Decimal;

        let market_id = model.market_id as u32;

        // Apply scaling if scaling cache is available
        let (price, size) = if let Some(scaling_cache) = &self.scaling_cache {
            let price_decimal =
                scaling_cache.scale_price(market_id, Decimal::from(model.order_price));
            let size_decimal =
                scaling_cache.scale_volume(market_id, Decimal::from(model.order_size));
            (price_decimal, size_decimal)
        } else {
            (
                Decimal::from(model.order_price),
                Decimal::from(model.order_size),
            )
        };

        let side = model.order_side.parse().unwrap_or_else(|_| {
            warn!(
                order_id = model.order_order_id,
                order_side = %model.order_side,
                "Invalid order_side value, defaulting to Bid"
            );
            Side::Bid
        });

        OrderResponse {
            order_id: model.order_order_id,
            user_id: model.order_user_id,
            market_id: model.market_id,
            price,
            size,
            side,
            timestamp: model.order_timestamp,
            time_in_force: model.order_time_in_force,
            status: Self::normalize_status(&model.order_status),
        }
    }

    /// Convert OrderModel to OrderHistoryResponse with scaling applied
    pub fn to_history_response(
        &self,
        model: OrderModel,
    ) -> crate::dto::trading::OrderHistoryResponse {
        use crate::dto::trading::{OrderHistoryResponse, Side};
        use rust_decimal::Decimal;

        let market_id = model.market_id as u32;

        // Apply scaling if scaling cache is available
        let (price, size) = if let Some(scaling_cache) = &self.scaling_cache {
            let price_decimal =
                scaling_cache.scale_price(market_id, Decimal::from(model.order_price));
            let size_decimal =
                scaling_cache.scale_volume(market_id, Decimal::from(model.order_size));
            (price_decimal, size_decimal)
        } else {
            (
                Decimal::from(model.order_price),
                Decimal::from(model.order_size),
            )
        };

        let side = model.order_side.parse().unwrap_or_else(|_| {
            warn!(
                order_id = model.order_order_id,
                order_side = %model.order_side,
                "Invalid order_side value, defaulting to Bid"
            );
            Side::Bid
        });

        OrderHistoryResponse {
            order_id: model.order_order_id,
            user_id: model.order_user_id,
            market_id: model.market_id,
            price,
            size,
            side,
            timestamp: model.order_timestamp,
            time_in_force: model.order_time_in_force,
            status: Self::normalize_status(&model.order_status),
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

    /// Get user's order history from orders table (paginated)
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
                                    if let Some(status) = hash.get("order_status") {
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
            return Ok(orders);
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
                                    if let Some(status) = hash.get("order_status") {
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
            return Ok(orders);
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
                                    if let Some(status) = hash.get("order_status") {
                                        let status_lower = status.to_lowercase();

                                        // Skip orders that are in terminal states (not open)
                                        if status_lower == "cancelled"
                                            || status_lower == "filled"
                                            || status_lower == "rejected"
                                        {
                                            continue;
                                        }

                                        // Skip PartiallyFilled IOC orders
                                        if status_lower == "partiallyfilled" {
                                            if let Some(tif) = hash.get("order_time_in_force") {
                                                let tif_lower = tif.to_lowercase();
                                                if tif_lower == "ioc" || tif_lower == "tif_ioc" {
                                                    continue;
                                                }
                                            }
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
                                    if let Some(status) = hash.get("order_status") {
                                        let status_lower = status.to_lowercase();

                                        // Skip orders that are in terminal states (not open)
                                        if status_lower == "cancelled"
                                            || status_lower == "filled"
                                            || status_lower == "rejected"
                                        {
                                            continue;
                                        }

                                        // Skip PartiallyFilled IOC orders
                                        if status_lower == "partiallyfilled" {
                                            if let Some(tif) = hash.get("order_time_in_force") {
                                                let tif_lower = tif.to_lowercase();
                                                if tif_lower == "ioc" || tif_lower == "tif_ioc" {
                                                    continue;
                                                }
                                            }
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

    /// Parse order hash from Redis into OrderModel
    /// Redis keys from Kafka sink: orders:{order_id}
    /// Fields are flattened from protobuf with `order_` prefix for nested Order struct:
    ///   market_id, order_order_id, order_user_id, order_price, order_size,
    ///   order_side, order_time_in_force, order_status, order_timestamp
    fn parse_order_hash(hash: HashMap<String, String>) -> Option<OrderModel> {
        // Parse with order_ prefix (from Kafka Flatten transform)
        let order_order_id = hash.get("order_order_id")?.parse::<i64>().ok()?;
        let order_user_id = hash.get("order_user_id")?.parse::<i64>().ok()?;
        let order_price = hash.get("order_price")?.parse::<i64>().ok()?;
        let order_size = hash.get("order_size")?.parse::<i64>().ok()?;
        let order_side = hash.get("order_side")?.clone();
        let order_timestamp = hash.get("order_timestamp")?.parse::<i64>().ok()?;
        let market_id = hash.get("market_id")?.parse::<i32>().ok()?;
        let order_time_in_force = hash.get("order_time_in_force")?.clone();
        let order_status = hash.get("order_status")?.clone();

        Some(OrderModel {
            id: 0, // Auto-generated by DB
            market_id,
            order_order_id,
            order_user_id,
            order_price,
            order_size,
            order_side,
            order_time_in_force,
            order_status,
            order_timestamp,
            created_at: None,
        })
    }
}
