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
use lib_core::cache::order_store::{OrderCacheConfig, OrderData, OrderStore};
use lib_core::dto::trading::Side;
use lib_core::model::orders::Model as OrderModel;
use lib_core::model::user_balances::Model as UserBalanceModel;
use lib_core::model::user_favorite_markets::Model as UserFavoriteMarketModel;
use lib_core::repository::repository::{TradingError, TradingRepository};
use redis::AsyncCommands;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

#[test]
fn orders_with_an_unknown_stored_side_are_skipped_not_shown_as_bids() {
    let store = OrderStore::new(OrderCacheConfig::default());
    let order = |side: &str| create_test_order(1, 100, 50000, 1000, side, "STATUS_PLACED");

    let bid = store
        .to_response(order("SIDE_BID"))
        .expect("SIDE_BID is core's bid");
    assert_eq!(bid.side, Side::Bid);
    let ask = store
        .to_history_response(order("SIDE_ASK"))
        .expect("SIDE_ASK is core's ask");
    assert_eq!(ask.side, Side::Ask);

    // Anything else is left out, like a record whose other stored fields do not parse.
    for side in ["Bid", "buy", "sell", "SIDE_UNSPECIFIED", ""] {
        assert!(store.to_response(order(side)).is_none(), "{side:?} shown");
        assert!(
            store.to_history_response(order(side)).is_none(),
            "{side:?} shown"
        );
    }
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

#[tokio::test]
async fn cached_orders_are_filtered_by_core_status_and_time_in_force_names() {
    // The Redis order hashes hold vex-core's protobuf enum value names (STATUS_*, TIF_*).
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());
    let client = redis::Client::open(redis_url.clone()).expect("Redis URL should be valid");
    let mut connection = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!(
                "Skipping cached order filter test: unable to connect to {redis_url}: {error}"
            );
            return;
        }
    };
    let user_id = (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64;
    let first_order_id = (uuid::Uuid::new_v4().as_u128() as u64) >> 2;
    let set_key = format!("user:{user_id}:orders");
    let orders = [
        (1_u64, "STATUS_PLACED", "TIF_GTC"),
        (2, "STATUS_PROCESSING", "TIF_GTC"),
        (3, "STATUS_PARTIALLY_FILLED", "TIF_GTC"),
        (4, "STATUS_PARTIALLY_FILLED", "TIF_IOC"),
        (5, "STATUS_FILLED", "TIF_GTC"),
        (6, "STATUS_CANCELLED", "TIF_IOC"),
        (7, "STATUS_REJECTED", "TIF_GTC"),
    ];
    let mut keys = vec![set_key.clone()];
    for (offset, status, time_in_force) in orders {
        let order_id = first_order_id + offset;
        let key = format!("order:{order_id}");
        let fields = [
            ("market_id", "1".to_string()),
            ("order_order_id", order_id.to_string()),
            ("order_user_id", user_id.to_string()),
            ("order_price", "100".to_string()),
            ("order_size", "10".to_string()),
            ("order_filled_size", "4".to_string()),
            ("order_side", "SIDE_BID".to_string()),
            ("order_time_in_force", time_in_force.to_string()),
            ("order_status", status.to_string()),
            ("order_timestamp", offset.to_string()),
        ];
        let _: () = connection
            .hset_multiple(&key, &fields)
            .await
            .expect("order hash should be written");
        let _: usize = connection
            .sadd(&set_key, order_id.to_string())
            .await
            .expect("order should be indexed");
        keys.push(key);
    }

    let store = OrderStore::with_redis_cache(OrderCacheConfig { enabled: true }, redis_url, None)
        .await
        .expect("order store should connect to Redis");
    let offsets = |orders: &[OrderData]| {
        let mut offsets: Vec<u64> = orders
            .iter()
            .map(|order| order.order_order_id as u64 - first_order_id)
            .collect();
        offsets.sort_unstable();
        offsets
    };

    let open = store.get_user_orders(user_id).await.unwrap();
    assert_eq!(offsets(&open), [1, 2, 3]);
    let mut open_statuses: Vec<String> = open
        .into_iter()
        .map(|order| store.to_response(order).unwrap().status)
        .collect();
    open_statuses.sort_unstable();
    assert_eq!(open_statuses, ["PartiallyFilled", "Placed", "Processing"]);
    let open_page = store
        .get_user_orders_paginated(user_id, 0, 10, None)
        .await
        .unwrap();
    assert_eq!(offsets(&open_page), [1, 2, 3]);

    let history = store.get_user_order_history(user_id).await.unwrap();
    assert_eq!(offsets(&history), [3, 4, 5, 6]);
    let mut history_statuses: Vec<String> = history
        .into_iter()
        .map(|order| store.to_history_response(order).unwrap().status)
        .collect();
    history_statuses.sort_unstable();
    assert_eq!(
        history_statuses,
        ["Cancelled", "Filled", "PartiallyFilled", "PartiallyFilled"]
    );
    let history_page = store
        .get_user_order_history_paginated(user_id, 0, 10, None)
        .await
        .unwrap();
    assert_eq!(offsets(&history_page), [3, 4, 5, 6]);

    let _: () = connection
        .del(keys)
        .await
        .expect("test order keys should be removed");
}

/// A TCP relay in front of the test Redis that counts the request flights its clients send. A
/// client waits for each reply before its next request, so N sequential commands take N flights
/// while a pipelined batch takes one. The returned URL reaches Redis through the relay.
async fn counting_redis_relay(redis_url: &str) -> (String, Arc<AtomicUsize>) {
    let info = redis::Client::open(redis_url)
        .expect("Redis URL should be valid")
        .get_connection_info()
        .clone();
    let redis::ConnectionAddr::Tcp(host, port) = info.addr().clone() else {
        panic!("the test Redis must be reached over plain TCP");
    };
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("relay should bind");
    let relay_url = format!(
        "redis://{}/{}",
        listener.local_addr().expect("relay should have an address"),
        info.redis_settings().db()
    );
    let flights = Arc::new(AtomicUsize::new(0));
    let counter = flights.clone();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let server = TcpStream::connect((host.as_str(), port))
                .await
                .expect("relay should reach Redis");
            let (mut client_read, mut client_write) = client.into_split();
            let (mut server_read, mut server_write) = server.into_split();
            tokio::spawn(async move { tokio::io::copy(&mut server_read, &mut client_write).await });
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0; 64 * 1024];
                while let Ok(read @ 1..) = client_read.read(&mut buffer).await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if server_write.write_all(&buffer[..read]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (relay_url, flights)
}

#[tokio::test]
async fn cached_order_lists_read_every_order_hash_in_one_round_trip() {
    // Issue #114: the indexed orders' hashes are read in one pipelined HGETALL batch rather than
    // one round trip each. Missing and unparsable orders are still skipped, and each list keeps
    // its order: index order, or newest first when paginated.
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());
    let client = redis::Client::open(redis_url.clone()).expect("Redis URL should be valid");
    let mut connection = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!(
                "Skipping cached order round-trip test: unable to connect to {redis_url}: {error}"
            );
            return;
        }
    };
    let user_id = (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64;
    let first_order_id = (uuid::Uuid::new_v4().as_u128() as u64) >> 2;
    let set_key = format!("user:{user_id}:orders");
    // (offset, market, status, time in force, price); an order's timestamp is 10 * offset.
    let orders = [
        (1_u64, "1", "STATUS_PLACED", "TIF_GTC", "100"),
        (2, "2", "STATUS_PARTIALLY_FILLED", "TIF_GTC", "100"),
        (3, "1", "STATUS_PARTIALLY_FILLED", "TIF_IOC", "100"),
        (4, "2", "STATUS_FILLED", "TIF_GTC", "100"),
        (5, "1", "STATUS_CANCELLED", "TIF_GTC", "100"),
        (6, "1", "STATUS_PROCESSING", "TIF_GTC", "100"),
        (7, "1", "STATUS_REJECTED", "TIF_GTC", "100"),
        // Unparsable: an open and a history order whose price is not a u64.
        (8, "1", "STATUS_PLACED", "TIF_GTC", "-1"),
        (9, "2", "STATUS_FILLED", "TIF_GTC", "1.5"),
        (10, "2", "STATUS_PLACED", "TIF_GTC", "100"),
    ];
    // Missing: indexed without an order hash.
    let missing = [11_u64, 12];
    let mut keys = vec![set_key.clone()];
    for (offset, market_id, status, time_in_force, price) in orders {
        let order_id = first_order_id + offset;
        let key = format!("order:{order_id}");
        let fields = [
            ("market_id", market_id.to_string()),
            ("order_order_id", order_id.to_string()),
            ("order_user_id", user_id.to_string()),
            ("order_price", price.to_string()),
            ("order_size", "10".to_string()),
            ("order_filled_size", "4".to_string()),
            ("order_side", "SIDE_BID".to_string()),
            ("order_time_in_force", time_in_force.to_string()),
            ("order_status", status.to_string()),
            ("order_timestamp", (10 * offset).to_string()),
        ];
        let _: () = connection
            .hset_multiple(&key, &fields)
            .await
            .expect("order hash should be written");
        keys.push(key);
    }
    let indexed: Vec<String> = orders
        .iter()
        .map(|order| order.0)
        .chain(missing)
        .map(|offset| (first_order_id + offset).to_string())
        .collect();
    let _: usize = connection
        .sadd(&set_key, &indexed)
        .await
        .expect("orders should be indexed");

    let index_order: Vec<u64> = connection
        .smembers::<_, Vec<String>>(&set_key)
        .await
        .expect("order index should be read")
        .iter()
        .map(|order_id| order_id.parse::<u64>().unwrap() - first_order_id)
        .collect();
    let in_index_order = |wanted: &[u64]| -> Vec<u64> {
        index_order
            .iter()
            .copied()
            .filter(|offset| wanted.contains(offset))
            .collect()
    };
    let offsets = |orders: Vec<OrderData>| -> Vec<u64> {
        orders
            .iter()
            .map(|order| order.order_order_id as u64 - first_order_id)
            .collect()
    };

    let (relay_url, flights) = counting_redis_relay(&redis_url).await;
    let store = OrderStore::with_redis_cache(OrderCacheConfig { enabled: true }, relay_url, None)
        .await
        .expect("order store should connect to Redis");
    flights.store(0, Ordering::SeqCst);
    let mut round_trips = Vec::new();

    let open = store.get_user_orders(user_id).await.unwrap();
    round_trips.push(flights.swap(0, Ordering::SeqCst));
    assert_eq!(offsets(open), in_index_order(&[1, 2, 6, 10]));

    let open_page = store
        .get_user_orders_paginated(user_id, 1, 2, None)
        .await
        .unwrap();
    round_trips.push(flights.swap(0, Ordering::SeqCst));
    assert_eq!(offsets(open_page), [6, 2]);

    let open_in_market = store
        .get_user_orders_paginated(user_id, 0, 10, Some(1))
        .await
        .unwrap();
    round_trips.push(flights.swap(0, Ordering::SeqCst));
    assert_eq!(offsets(open_in_market), [6, 1]);

    let history = store.get_user_order_history(user_id).await.unwrap();
    round_trips.push(flights.swap(0, Ordering::SeqCst));
    assert_eq!(offsets(history), in_index_order(&[2, 3, 4, 5]));

    let history_page = store
        .get_user_order_history_paginated(user_id, 1, 2, None)
        .await
        .unwrap();
    round_trips.push(flights.swap(0, Ordering::SeqCst));
    assert_eq!(offsets(history_page), [4, 3]);

    let history_in_market = store
        .get_user_order_history_paginated(user_id, 0, 10, Some(2))
        .await
        .unwrap();
    round_trips.push(flights.swap(0, Ordering::SeqCst));
    assert_eq!(offsets(history_in_market), [4, 2]);

    // Each read is the index's SMEMBERS plus one pipelined HGETALL batch for its 12 orders.
    assert_eq!(round_trips, [2; 6]);

    let _: () = connection
        .del(keys)
        .await
        .expect("test order keys should be removed");
}
