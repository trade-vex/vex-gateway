/// Comprehensive Unit Tests for OrderStore
///
/// ## Purpose
/// Verify that the OrderStore correctly:
/// 1. Queries Redis cache for orders
/// 2. Falls back to database repository when cache misses
/// 3. Parses Redis hash data into OrderModel
/// 4. Filters orders by status (open vs history)
/// 5. Handles invalidation operations
///
/// ## Test Coverage
/// - Cache hit scenarios
/// - Cache miss with database fallback
/// - Order parsing with various data formats (decimal/integer)
/// - Status filtering for open orders
/// - Invalidation of single orders and user orders
/// - Error handling and edge cases
use async_trait::async_trait;
use lib_core::cache::order_store::{OrderCacheConfig, OrderStore};
use lib_core::model::orders::Model as OrderModel;
use lib_core::model::user_balances::Model as UserBalanceModel;
use lib_core::model::user_favorite_markets::Model as UserFavoriteMarketModel;
use lib_core::repository::repository::{TradingError, TradingRepository};
use redis::AsyncCommands;
use std::sync::Arc;

type Result<T> = std::result::Result<T, TradingError>;

/// Mock TradingRepository for testing
#[derive(Clone)]
struct MockTradingRepository {
    orders: Vec<OrderModel>,
    should_error: bool,
}

impl MockTradingRepository {
    fn new() -> Self {
        Self {
            orders: Vec::new(),
            should_error: false,
        }
    }

    fn with_orders(mut self, orders: Vec<OrderModel>) -> Self {
        self.orders = orders;
        self
    }

    fn with_error(mut self) -> Self {
        self.should_error = true;
        self
    }
}

#[async_trait]
impl TradingRepository for MockTradingRepository {
    async fn get_user_balances(&self, _user_id: u64) -> Result<Vec<UserBalanceModel>> {
        Ok(vec![])
    }

    async fn get_user_balance(&self, _user_id: u64) -> Result<Option<UserBalanceModel>> {
        Ok(None)
    }

    async fn update_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> Result<()> {
        Ok(())
    }

    async fn create_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> Result<()> {
        Ok(())
    }

    async fn get_open_orders(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .orders
            .iter()
            .filter(|o| o.order_user_id == user_id as i64)
            .cloned()
            .collect())
    }

    async fn get_open_orders_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        let mut orders: Vec<_> = self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == user_id as i64
                    && market_id.is_none_or(|mid| o.market_id == mid as i32)
            })
            .cloned()
            .collect();
        // Sort by timestamp descending (most recent first)
        orders.sort_by(|a, b| b.order_timestamp.cmp(&a.order_timestamp));
        // Apply pagination
        let start = offset as usize;
        let end = (offset + limit) as usize;
        Ok(orders
            .into_iter()
            .skip(start)
            .take((end - start).min(limit as usize))
            .collect())
    }

    async fn count_open_orders(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        let count = self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == user_id as i64
                    && market_id.is_none_or(|mid| o.market_id == mid as i32)
            })
            .count();
        Ok(count as u64)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .orders
            .iter()
            .find(|o| o.order_order_id == order_id as i64 && o.order_user_id == user_id as i64)
            .cloned())
    }

    async fn create_order(&self, _order: OrderModel) -> Result<u64> {
        Ok(1)
    }

    async fn count_user_orders(&self, _user_id: u64) -> Result<u64> {
        Ok(0)
    }

    async fn get_order_history(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == user_id as i64
                    && (o.order_status.to_lowercase() == "cancelled"
                        || o.order_status.to_lowercase() == "partiallyfilled"
                        || o.order_status.to_lowercase() == "filled")
            })
            .cloned()
            .collect())
    }

    async fn get_order_history_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        let mut orders: Vec<_> = self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == user_id as i64
                    && (o.order_status.to_lowercase() == "cancelled"
                        || o.order_status.to_lowercase() == "partiallyfilled"
                        || o.order_status.to_lowercase() == "filled")
                    && market_id.is_none_or(|mid| o.market_id == mid as i32)
            })
            .cloned()
            .collect();
        // Sort by timestamp descending (most recent first)
        orders.sort_by(|a, b| b.order_timestamp.cmp(&a.order_timestamp));
        // Apply pagination
        let start = offset as usize;
        let end = (offset + limit) as usize;
        Ok(orders
            .into_iter()
            .skip(start)
            .take((end - start).min(limit as usize))
            .collect())
    }

    async fn count_order_history(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        let count = self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == user_id as i64
                    && market_id.is_none_or(|mid| o.market_id == mid as i32)
                    && (o.order_status.to_lowercase() == "cancelled"
                        || o.order_status.to_lowercase() == "partiallyfilled"
                        || o.order_status.to_lowercase() == "filled")
            })
            .count();
        Ok(count as u64)
    }

    async fn toggle_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<bool> {
        Ok(true)
    }

    async fn get_user_favorite_markets(
        &self,
        _user_id: u64,
    ) -> Result<Vec<UserFavoriteMarketModel>> {
        Ok(vec![])
    }

    async fn add_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<()> {
        Ok(())
    }

    async fn remove_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<()> {
        Ok(())
    }

    async fn is_market_favorited(&self, _user_id: u64, _market_id: u32) -> Result<bool> {
        Ok(false)
    }
}

/// Helper: Create test order
fn create_test_order(
    order_id: i64,
    user_id: i64,
    price: i64,
    size: i64,
    side: &str,
    status: &str,
) -> OrderModel {
    OrderModel {
        id: 0,
        market_id: 1,
        order_order_id: order_id,
        order_user_id: user_id,
        order_price: price,
        order_size: size,
        order_filled_size: 0,
        order_side: side.to_string(),
        order_time_in_force: "GTC".to_string(),
        order_status: status.to_string(),
        order_timestamp: 1700000000,
        created_at: None,
    }
}

#[tokio::test]
async fn test_order_store_new() {
    let config = OrderCacheConfig::default();
    let store = OrderStore::new(config.clone());

    // Should work without cache or repository
    let result = store.get_user_orders(1).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_order_store_with_repository() {
    let config = OrderCacheConfig { enabled: false };

    let orders = vec![
        create_test_order(1, 100, 50000, 1000, "buy", "Pending"),
        create_test_order(2, 100, 51000, 2000, "sell", "Pending"),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders.clone()));
    let store = OrderStore::with_repository(config, mock_repo);

    // Should fetch from repository
    let result = store.get_user_orders(100).await;
    assert!(result.is_ok());
    let fetched_orders = result.unwrap();
    assert_eq!(fetched_orders.len(), 2);
    assert_eq!(fetched_orders[0].order_order_id, 1);
    assert_eq!(fetched_orders[1].order_order_id, 2);
}

#[tokio::test]
async fn test_get_order_by_id() {
    let config = OrderCacheConfig { enabled: false };

    let orders = vec![create_test_order(1, 100, 50000, 1000, "buy", "Pending")];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
    let store = OrderStore::with_repository(config, mock_repo);

    // Test found order
    let result = store.get_order_by_id(1, 100).await;
    assert!(result.is_ok());
    let order = result.unwrap();
    assert!(order.is_some());
    let order = order.unwrap();
    assert_eq!(order.order_order_id, 1);
    assert_eq!(order.order_user_id, 100);

    // Test order not found
    let result = store.get_order_by_id(999, 100).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());

    // Test wrong user
    let result = store.get_order_by_id(1, 200).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn test_get_user_orders_empty() {
    let config = OrderCacheConfig { enabled: false };

    let mock_repo = Arc::new(MockTradingRepository::new());
    let store = OrderStore::with_repository(config, mock_repo);

    let result = store.get_user_orders(100).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_get_user_order_history() {
    let config = OrderCacheConfig { enabled: false };

    let orders = vec![
        create_test_order(1, 100, 50000, 1000, "buy", "Pending"),
        create_test_order(2, 100, 51000, 2000, "sell", "Filled"),
        create_test_order(3, 100, 52000, 3000, "buy", "Cancelled"),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
    let store = OrderStore::with_repository(config, mock_repo);

    // History should only include terminal orders: Filled, Cancelled, PartiallyFilled
    // Pending orders are excluded from history (they're in open orders)
    let result = store.get_user_order_history(100).await;
    assert!(result.is_ok());
    let history = result.unwrap();
    assert_eq!(history.len(), 2); // Only Filled and Cancelled, not Pending
}

#[tokio::test]
async fn test_database_error_handling() {
    let config = OrderCacheConfig { enabled: false };

    let mock_repo = Arc::new(MockTradingRepository::new().with_error());
    let store = OrderStore::with_repository(config, mock_repo);

    // Should return database error
    let result = store.get_user_orders(100).await;
    assert!(result.is_err());
    match result {
        Err(TradingError::Database(msg)) => {
            assert_eq!(msg, "Mock database error");
        }
        _ => panic!("Expected database error"),
    }
}

#[tokio::test]
async fn test_invalidate_operations() {
    let config = OrderCacheConfig { enabled: false }; // Cache disabled, so invalidation is no-op

    let store = OrderStore::new(config);

    // Should not panic even with cache disabled
    store.invalidate_order(1, 100).await;
    store.invalidate_user_orders(100).await;
}

#[tokio::test]
async fn test_multiple_users() {
    let config = OrderCacheConfig { enabled: false };

    let orders = vec![
        create_test_order(1, 100, 50000, 1000, "buy", "Pending"),
        create_test_order(2, 200, 51000, 2000, "sell", "Pending"),
        create_test_order(3, 100, 52000, 3000, "buy", "Pending"),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
    let store = OrderStore::with_repository(config, mock_repo);

    // User 100 should have 2 orders
    let result = store.get_user_orders(100).await;
    assert!(result.is_ok());
    let user100_orders = result.unwrap();
    assert_eq!(user100_orders.len(), 2);

    // User 200 should have 1 order
    let result = store.get_user_orders(200).await;
    assert!(result.is_ok());
    let user200_orders = result.unwrap();
    assert_eq!(user200_orders.len(), 1);
    assert_eq!(user200_orders[0].order_order_id, 2);
}

#[tokio::test]
async fn test_order_status_variations() {
    let config = OrderCacheConfig { enabled: false };

    let statuses = vec![
        "Pending",
        "PartiallyFilled",
        "Processing",
        "Filled",
        "Cancelled",
        "Rejected",
        "Placed",
    ];

    for status in statuses {
        let orders = vec![create_test_order(1, 100, 50000, 1000, "buy", status)];
        let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
        let store = OrderStore::with_repository(config.clone(), mock_repo);

        let result = store.get_user_orders(100).await;
        assert!(result.is_ok(), "Failed for status: {}", status);
    }
}

#[tokio::test]
async fn test_order_side_variations() {
    let config = OrderCacheConfig { enabled: false };

    let orders = vec![
        create_test_order(1, 100, 50000, 1000, "buy", "Pending"),
        create_test_order(2, 100, 51000, 2000, "sell", "Pending"),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
    let store = OrderStore::with_repository(config, mock_repo);

    let result = store.get_user_orders(100).await;
    assert!(result.is_ok());
    let fetched_orders = result.unwrap();
    assert_eq!(fetched_orders.len(), 2);
    assert_eq!(fetched_orders[0].order_side, "buy");
    assert_eq!(fetched_orders[1].order_side, "sell");
}

#[tokio::test]
async fn test_large_order_values() {
    let config = OrderCacheConfig { enabled: false };

    // Test with large price and size values (12 decimal places precision)
    let large_price = 50_000_000_000_000_000_u64; // 50,000 * 10^12
    let large_size = 1_000_000_000_000_u64; // 1.0 * 10^12

    let orders = vec![create_test_order(
        1,
        100,
        large_price as i64,
        large_size as i64,
        "buy",
        "Pending",
    )];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
    let store = OrderStore::with_repository(config, mock_repo);

    let result = store.get_order_by_id(1, 100).await;
    assert!(result.is_ok());
    let order = result.unwrap().unwrap();
    assert_eq!(order.order_price, large_price);
    assert_eq!(order.order_size, large_size);
}

#[tokio::test]
async fn test_config_default_values() {
    let config = OrderCacheConfig::default();
    assert!(!config.enabled);
}

#[tokio::test]
async fn test_config_custom_values() {
    let config = OrderCacheConfig { enabled: true };
    assert!(config.enabled);
}

#[tokio::test]
async fn test_concurrent_access() {
    let config = OrderCacheConfig { enabled: false };

    let orders = vec![
        create_test_order(1, 100, 50000, 1000, "buy", "Pending"),
        create_test_order(2, 100, 51000, 2000, "sell", "Pending"),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_orders(orders));
    let store = Arc::new(OrderStore::with_repository(config, mock_repo));

    // Spawn multiple concurrent tasks
    let mut handles = vec![];
    for _ in 0..10 {
        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            let result = store_clone.get_user_orders(100).await;
            assert!(result.is_ok());
            result.unwrap()
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        let orders = handle.await.unwrap();
        assert_eq!(orders.len(), 2);
    }
}

#[tokio::test]
async fn test_u64_order_magnitudes_survive_redis_round_trip() {
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());
    let client = redis::Client::open(redis_url.clone()).expect("Redis URL should be valid");
    let mut connection = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("Skipping Redis round-trip test: unable to connect to {redis_url}: {error}");
            return;
        }
    };
    let order_id = 8_000_000_000_u64 + u64::from(std::process::id());
    let user_id = 7_000_000_000_u64 + u64::from(std::process::id());
    let key = format!("order:{order_id}");
    let fields = [
        ("market_id", "1".to_string()),
        ("order_order_id", order_id.to_string()),
        ("order_user_id", user_id.to_string()),
        ("order_price", u64::MAX.to_string()),
        ("order_size", u64::MAX.to_string()),
        ("order_filled_size", (i64::MAX as u64 + 1).to_string()),
        ("order_side", "Bid".to_string()),
        ("order_time_in_force", "GTC".to_string()),
        ("order_status", "Placed".to_string()),
        ("order_timestamp", "1700000000".to_string()),
    ];
    let _: () = connection
        .hset_multiple(&key, &fields)
        .await
        .expect("order hash should be written");

    let store = OrderStore::with_redis_cache(OrderCacheConfig { enabled: true }, redis_url, None)
        .await
        .expect("order store should connect to Redis");
    let order = store
        .get_order_by_id(order_id, user_id)
        .await
        .expect("order should be read from Redis")
        .expect("order row should not be dropped");

    assert_eq!(order.order_price, u64::MAX);
    assert_eq!(order.order_size, u64::MAX);
    assert_eq!(order.order_filled_size, i64::MAX as u64 + 1);

    let _: () = connection
        .del(key)
        .await
        .expect("test order hash should be removed");
}
