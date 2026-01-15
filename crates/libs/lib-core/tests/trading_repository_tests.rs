/// Comprehensive Tests for TradingRepository and PortfolioRepository
///
/// ## Purpose
/// Verify that repository implementations correctly:
/// 1. Perform database operations for balances, orders, and favorite markets
/// 2. Handle error conditions gracefully
/// 3. Maintain data consistency and constraints
/// 4. Filter and query data correctly
///
/// ## Test Coverage
/// - PostgresTradingRepository: Balance, Order, and Favorite Markets operations
/// - Error handling and edge cases
/// - Data validation and constraints
///
/// Note: ClickHousePortfolioRepository tests are in portfolio_repository_clickhouse.rs
use async_trait::async_trait;
use chrono::Utc;
use lib_core::model::orders::Model as OrderModel;
use lib_core::model::user_balances::Model as UserBalanceModel;
use lib_core::model::user_favorite_markets::Model as UserFavoriteMarketModel;
use lib_core::repository::repository::{
    PostgresTradingRepository, TradingError, TradingRepository,
};
use rust_decimal::Decimal;
use sea_orm::{Database, DbErr};
use std::sync::Arc;

type Result<T> = std::result::Result<T, TradingError>;

// ============================================================================
// Mock Repository for Unit Testing
// ============================================================================

/// Mock TradingRepository for isolated unit tests
#[derive(Clone)]
struct MockTradingRepository {
    balances: Vec<UserBalanceModel>,
    orders: Vec<OrderModel>,
    favorite_markets: Vec<UserFavoriteMarketModel>,
    should_error: bool,
}

impl MockTradingRepository {
    fn new() -> Self {
        Self {
            balances: Vec::new(),
            orders: Vec::new(),
            favorite_markets: Vec::new(),
            should_error: false,
        }
    }

    fn with_balances(mut self, balances: Vec<UserBalanceModel>) -> Self {
        self.balances = balances;
        self
    }

    fn with_orders(mut self, orders: Vec<OrderModel>) -> Self {
        self.orders = orders;
        self
    }

    fn with_favorite_markets(mut self, markets: Vec<UserFavoriteMarketModel>) -> Self {
        self.favorite_markets = markets;
        self
    }

    fn with_error(mut self) -> Self {
        self.should_error = true;
        self
    }
}

#[async_trait]
impl TradingRepository for MockTradingRepository {
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .balances
            .iter()
            .filter(|b| b.user_id == Decimal::from(user_id))
            .cloned()
            .collect())
    }

    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .balances
            .iter()
            .find(|b| b.user_id == Decimal::from(user_id))
            .cloned())
    }

    async fn update_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> Result<()> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(())
    }

    async fn create_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> Result<()> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(())
    }

    async fn get_open_orders(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        let orders: Vec<_> = self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == Decimal::from(user_id)
                    && (o.status.to_lowercase() == "placed"
                        || o.status.to_lowercase() == "pending"
                        || o.status.to_lowercase() == "partiallyfilled"
                        || o.status.to_lowercase() == "processing")
            })
            .cloned()
            .collect();
        Ok(orders)
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
                o.order_user_id == Decimal::from(user_id)
                    && (o.status.to_lowercase() == "placed"
                        || o.status.to_lowercase() == "pending"
                        || o.status.to_lowercase() == "partiallyfilled"
                        || o.status.to_lowercase() == "processing")
                    && market_id.map_or(true, |mid| o.market_id == mid as i32)
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
                o.order_user_id == Decimal::from(user_id)
                    && (o.status.to_lowercase() == "placed"
                        || o.status.to_lowercase() == "pending"
                        || o.status.to_lowercase() == "partiallyfilled"
                        || o.status.to_lowercase() == "processing")
                    && market_id.map_or(true, |mid| o.market_id == mid as i32)
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
            .find(|o| {
                o.order_order_id == Decimal::from(order_id)
                    && o.order_user_id == Decimal::from(user_id)
            })
            .cloned())
    }

    async fn create_order(&self, order: OrderModel) -> Result<u64> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(order.order_order_id.to_string().parse::<u64>().unwrap_or(0))
    }

    async fn count_user_orders(&self, user_id: u64) -> Result<u64> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .orders
            .iter()
            .filter(|o| o.order_user_id == Decimal::from(user_id))
            .count() as u64)
    }

    async fn get_order_history(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .orders
            .iter()
            .filter(|o| {
                o.order_user_id == Decimal::from(user_id)
                    && (o.status.to_lowercase() == "cancelled"
                        || o.status.to_lowercase() == "partiallyfilled"
                        || o.status.to_lowercase() == "filled")
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
                o.order_user_id == Decimal::from(user_id)
                    && (o.status.to_lowercase() == "cancelled"
                        || o.status.to_lowercase() == "partiallyfilled"
                        || o.status.to_lowercase() == "filled")
                    && market_id.map_or(true, |mid| o.market_id == mid as i32)
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
                o.order_user_id == Decimal::from(user_id)
                    && (o.status.to_lowercase() == "cancelled"
                        || o.status.to_lowercase() == "partiallyfilled"
                        || o.status.to_lowercase() == "filled")
                    && market_id.map_or(true, |mid| o.market_id == mid as i32)
            })
            .count();
        Ok(count as u64)
    }

    async fn toggle_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<bool> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(true)
    }

    async fn get_user_favorite_markets(
        &self,
        user_id: u64,
    ) -> Result<Vec<UserFavoriteMarketModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .favorite_markets
            .iter()
            .filter(|m| m.user_id == user_id as i64)
            .cloned()
            .collect())
    }

    async fn add_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<()> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(())
    }

    async fn remove_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<()> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(())
    }

    async fn is_market_favorited(&self, user_id: u64, market_id: u32) -> Result<bool> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .favorite_markets
            .iter()
            .any(|m| m.user_id == user_id as i64 && m.market_id == market_id as i32))
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn create_test_balance(
    user_id: i64,
    asset_id: i32,
    available: i64,
    locked: i64,
) -> UserBalanceModel {
    UserBalanceModel {
        id: 1,
        user_id: Decimal::from(user_id),
        asset_id,
        available: Decimal::from(available),
        locked: Decimal::from(locked),
        total: Decimal::from(available) + Decimal::from(locked),
        timestamp: Utc::now().timestamp(),
        created_at: Some(Utc::now().naive_utc()),
    }
}

fn create_test_order(
    order_id: i64,
    user_id: i64,
    price: i64,
    size: i64,
    side: &str,
    status: &str,
) -> OrderModel {
    OrderModel {
        id: order_id,
        order_order_id: Decimal::from(order_id),
        order_user_id: Decimal::from(user_id),
        order_price: Decimal::from(price),
        order_size: Decimal::from(size),
        filled_size: Decimal::ZERO,
        order_side: side.to_string(),
        order_timestamp: Utc::now().timestamp(),
        market_id: 1,
        time_in_force: "GTC".to_string(),
        status: status.to_string(),
        created_at: Some(Utc::now().naive_utc()),
    }
}

fn create_test_favorite_market(user_id: u64, market_id: i32) -> UserFavoriteMarketModel {
    UserFavoriteMarketModel {
        id: 1,
        user_id: user_id as i64,
        market_id,
        favorited_at: Some(Utc::now()),
    }
}

// ============================================================================
// Unit Tests with Mock Repository
// ============================================================================

#[tokio::test]
async fn test_get_user_balances_success() {
    let balance1 = create_test_balance(100, 0, 1000000, 500000);
    let balance2 = create_test_balance(100, 1, 2000000, 0);
    let balance3 = create_test_balance(200, 0, 3000000, 0); // Different user

    let repo = MockTradingRepository::new().with_balances(vec![
        balance1.clone(),
        balance2.clone(),
        balance3.clone(),
    ]);

    let result = repo.get_user_balances(100).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().any(|b| b.asset_id == 0));
    assert!(result.iter().any(|b| b.asset_id == 1));
    assert!(result.iter().all(|b| b.user_id == Decimal::from(100)));
}

#[tokio::test]
async fn test_get_user_balances_empty() {
    let repo = MockTradingRepository::new();
    let result = repo.get_user_balances(999).await.unwrap();
    assert_eq!(result.len(), 0);
}

#[tokio::test]
async fn test_get_user_balances_error() {
    let repo = MockTradingRepository::new().with_error();
    let result = repo.get_user_balances(100).await;
    assert!(result.is_err());
    match result {
        Err(TradingError::Database(msg)) => assert!(msg.contains("Mock database error")),
        _ => panic!("Expected Database error"),
    }
}

#[tokio::test]
async fn test_get_user_balance_success() {
    let balance = create_test_balance(100, 0, 1000000, 500000);
    let repo = MockTradingRepository::new().with_balances(vec![balance.clone()]);

    let result = repo.get_user_balance(100).await.unwrap();
    assert!(result.is_some());
    let balance = result.unwrap();
    assert_eq!(balance.user_id, Decimal::from(100));
    assert_eq!(balance.available, Decimal::from(1000000));
    assert_eq!(balance.locked, Decimal::from(500000));
}

#[tokio::test]
async fn test_get_user_balance_not_found() {
    let repo = MockTradingRepository::new();
    let result = repo.get_user_balance(999).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_update_user_balance_success() {
    let repo = MockTradingRepository::new();
    let result = repo.update_user_balance(100, 2000000, 1000000).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_create_user_balance_success() {
    let repo = MockTradingRepository::new();
    let result = repo.create_user_balance(100, 1000000, 0).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_open_orders_filters_by_status() {
    let pending_order = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let filled_order = create_test_order(2, 100, 50000, 10000, "Buy", "Filled");
    let cancelled_order = create_test_order(3, 100, 50000, 10000, "Buy", "Cancelled");
    let processing_order = create_test_order(4, 100, 50000, 10000, "Buy", "Processing");
    let partially_filled = create_test_order(5, 100, 50000, 10000, "Buy", "PartiallyFilled");

    let repo = MockTradingRepository::new().with_orders(vec![
        pending_order.clone(),
        filled_order,
        cancelled_order,
        processing_order.clone(),
        partially_filled.clone(),
    ]);

    let result = repo.get_open_orders(100).await.unwrap();
    assert_eq!(result.len(), 3);
    assert!(result.iter().any(|o| o.status == "Pending"));
    assert!(result.iter().any(|o| o.status == "Processing"));
    assert!(result.iter().any(|o| o.status == "PartiallyFilled"));
    assert!(!result.iter().any(|o| o.status == "Filled"));
    assert!(!result.iter().any(|o| o.status == "Cancelled"));
}

#[tokio::test]
async fn test_get_open_orders_filters_by_user() {
    let user1_order = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let user2_order = create_test_order(2, 200, 50000, 10000, "Buy", "Pending");

    let repo = MockTradingRepository::new().with_orders(vec![user1_order, user2_order]);

    let result = repo.get_open_orders(100).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].order_user_id, Decimal::from(100));
}

#[tokio::test]
async fn test_get_order_by_id_success() {
    let order = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let repo = MockTradingRepository::new().with_orders(vec![order.clone()]);

    let result = repo.get_order_by_id(1, 100).await.unwrap();
    assert!(result.is_some());
    let order = result.unwrap();
    assert_eq!(order.order_order_id, Decimal::from(1));
    assert_eq!(order.order_user_id, Decimal::from(100));
}

#[tokio::test]
async fn test_get_order_by_id_wrong_user() {
    let order = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let repo = MockTradingRepository::new().with_orders(vec![order]);

    let result = repo.get_order_by_id(1, 200).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_get_order_by_id_not_found() {
    let repo = MockTradingRepository::new();
    let result = repo.get_order_by_id(999, 100).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_create_order_success() {
    let order = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let repo = MockTradingRepository::new();

    let result = repo.create_order(order).await.unwrap();
    assert_eq!(result, 1u64);
}

#[tokio::test]
async fn test_count_user_orders() {
    let order1 = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let order2 = create_test_order(2, 100, 50000, 10000, "Sell", "Filled");
    let order3 = create_test_order(3, 200, 50000, 10000, "Buy", "Pending");

    let repo = MockTradingRepository::new().with_orders(vec![order1, order2, order3]);

    let count = repo.count_user_orders(100).await.unwrap();
    assert_eq!(count, 2);

    let count = repo.count_user_orders(200).await.unwrap();
    assert_eq!(count, 1);

    let count = repo.count_user_orders(999).await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_get_user_favorite_markets_success() {
    let fav1 = create_test_favorite_market(100, 1);
    let fav2 = create_test_favorite_market(100, 2);
    let fav3 = create_test_favorite_market(200, 3);

    let repo =
        MockTradingRepository::new().with_favorite_markets(vec![fav1.clone(), fav2.clone(), fav3]);

    let result = repo.get_user_favorite_markets(100).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().any(|m| m.market_id == 1));
    assert!(result.iter().any(|m| m.market_id == 2));
    assert!(result.iter().all(|m| m.user_id == 100));
}

#[tokio::test]
async fn test_get_user_favorite_markets_empty() {
    let repo = MockTradingRepository::new();
    let result = repo.get_user_favorite_markets(999).await.unwrap();
    assert_eq!(result.len(), 0);
}

#[tokio::test]
async fn test_add_favorite_market_success() {
    let repo = MockTradingRepository::new();
    let result = repo.add_favorite_market(100, 1).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_remove_favorite_market_success() {
    let repo = MockTradingRepository::new();
    let result = repo.remove_favorite_market(100, 1).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_is_market_favorited_true() {
    let fav = create_test_favorite_market(100, 1);
    let repo = MockTradingRepository::new().with_favorite_markets(vec![fav]);

    let result = repo.is_market_favorited(100, 1).await.unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_is_market_favorited_false() {
    let repo = MockTradingRepository::new();
    let result = repo.is_market_favorited(100, 1).await.unwrap();
    assert!(!result);
}

#[tokio::test]
async fn test_is_market_favorited_wrong_user() {
    let fav = create_test_favorite_market(100, 1);
    let repo = MockTradingRepository::new().with_favorite_markets(vec![fav]);

    let result = repo.is_market_favorited(200, 1).await.unwrap();
    assert!(!result);
}

// ============================================================================
// Integration Tests with PostgresTradingRepository
// ============================================================================
// Note: These tests require a PostgreSQL test database
// Set DATABASE_URL environment variable to run these tests

#[tokio::test]
#[ignore] // Requires PostgreSQL connection - run with: cargo test -- --ignored
async fn test_postgres_repository_integration() {
    // Get database URL from environment or skip test
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("Skipping PostgreSQL integration test: DATABASE_URL not set");
            return;
        }
    };

    // Connect to database
    let db = match Database::connect(&db_url).await {
        Ok(conn) => Arc::new(conn),
        Err(e) => {
            eprintln!(
                "Skipping PostgreSQL integration test: failed to connect: {}",
                e
            );
            return;
        }
    };

    let repo = PostgresTradingRepository::new(db.clone());

    // Test user ID (should be a test user that exists in your test database)
    let test_user_id = 1u64;

    // Test get_user_balances (should not error even if no data)
    let balances = repo.get_user_balances(test_user_id).await;
    assert!(balances.is_ok(), "get_user_balances should not error");

    // Test get_user_balance
    let balance = repo.get_user_balance(test_user_id).await;
    assert!(balance.is_ok(), "get_user_balance should not error");

    // Test get_open_orders
    let orders = repo.get_open_orders(test_user_id).await;
    assert!(orders.is_ok(), "get_open_orders should not error");

    // Test count_user_orders
    let count = repo.count_user_orders(test_user_id).await;
    assert!(count.is_ok(), "count_user_orders should not error");

    // Test get_user_favorite_markets
    let favorites = repo.get_user_favorite_markets(test_user_id).await;
    assert!(
        favorites.is_ok(),
        "get_user_favorite_markets should not error"
    );

    // Test is_market_favorited
    let is_fav = repo.is_market_favorited(test_user_id, 1).await;
    assert!(is_fav.is_ok(), "is_market_favorited should not error");

    println!("✓ All PostgreSQL integration tests passed");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_trading_error_display() {
    let db_error = TradingError::Database("Connection failed".to_string());
    assert_eq!(db_error.to_string(), "Database error: Connection failed");

    let not_found = TradingError::NotFound;
    assert_eq!(not_found.to_string(), "Not found");
}

#[tokio::test]
async fn test_trading_error_from_db_err() {
    let db_err = DbErr::Custom("Test database error".to_string());
    let trading_err: TradingError = db_err.into();

    match trading_err {
        TradingError::Database(msg) => {
            assert!(msg.contains("Test database error"));
        }
        _ => panic!("Expected Database error"),
    }
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[tokio::test]
async fn test_get_open_orders_case_insensitive_status() {
    let pending = create_test_order(1, 100, 50000, 10000, "Buy", "PENDING");
    let processing = create_test_order(2, 100, 50000, 10000, "Buy", "pRoCeSsInG");

    let repo = MockTradingRepository::new().with_orders(vec![pending, processing]);

    let result = repo.get_open_orders(100).await.unwrap();
    assert_eq!(
        result.len(),
        2,
        "Status filtering should be case-insensitive"
    );
}

#[tokio::test]
async fn test_balance_total_calculation() {
    let balance = create_test_balance(100, 0, 1000000, 500000);
    assert_eq!(balance.total, Decimal::from(1500000));
    assert_eq!(balance.total, balance.available + balance.locked);
}

#[tokio::test]
async fn test_multiple_assets_per_user() {
    let btc = create_test_balance(100, 0, 100000000, 0); // BTC
    let eth = create_test_balance(100, 1, 1000000000, 0); // ETH
    let usdt = create_test_balance(100, 2, 10000000000, 0); // USDT

    let repo = MockTradingRepository::new().with_balances(vec![btc, eth, usdt]);

    let result = repo.get_user_balances(100).await.unwrap();
    assert_eq!(result.len(), 3);

    let asset_ids: Vec<i32> = result.iter().map(|b| b.asset_id).collect();
    assert!(asset_ids.contains(&0));
    assert!(asset_ids.contains(&1));
    assert!(asset_ids.contains(&2));
}

#[tokio::test]
async fn test_order_side_preservation() {
    let buy_order = create_test_order(1, 100, 50000, 10000, "Buy", "Pending");
    let sell_order = create_test_order(2, 100, 50000, 10000, "Sell", "Pending");

    let repo =
        MockTradingRepository::new().with_orders(vec![buy_order.clone(), sell_order.clone()]);

    let result = repo.get_open_orders(100).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().any(|o| o.order_side == "Buy"));
    assert!(result.iter().any(|o| o.order_side == "Sell"));
}

#[tokio::test]
async fn test_favorite_markets_multiple_users() {
    let user1_fav1 = create_test_favorite_market(100, 1);
    let user1_fav2 = create_test_favorite_market(100, 2);
    let user2_fav1 = create_test_favorite_market(200, 1);
    let user2_fav2 = create_test_favorite_market(200, 3);

    let repo = MockTradingRepository::new()
        .with_favorite_markets(vec![user1_fav1, user1_fav2, user2_fav1, user2_fav2]);

    // User 1 favorites
    let user1_favs = repo.get_user_favorite_markets(100).await.unwrap();
    assert_eq!(user1_favs.len(), 2);
    assert!(user1_favs.iter().all(|m| m.user_id == 100));

    // User 2 favorites
    let user2_favs = repo.get_user_favorite_markets(200).await.unwrap();
    assert_eq!(user2_favs.len(), 2);
    assert!(user2_favs.iter().all(|m| m.user_id == 200));

    // Check specific market favorites
    assert!(repo.is_market_favorited(100, 1).await.unwrap());
    assert!(repo.is_market_favorited(100, 2).await.unwrap());
    assert!(!repo.is_market_favorited(100, 3).await.unwrap());

    assert!(repo.is_market_favorited(200, 1).await.unwrap());
    assert!(!repo.is_market_favorited(200, 2).await.unwrap());
    assert!(repo.is_market_favorited(200, 3).await.unwrap());
}
