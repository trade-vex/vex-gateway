//! Unit tests for user handlers
//!
//! These tests use the actual Postgres implementation to test user data endpoints:
//! - Balances
//! - Orders (open and history)
//! - Balance and PnL charts
//! - Favorite markets

use actix_web::web;
use chrono::Utc;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use async_trait::async_trait;
use lib_api::handlers::user::{
    OrdersQuery, get_balances, get_favorite_markets, get_open_orders, get_order_history,
    get_portfolio_overview, toggle_favorite_market,
};
use lib_core::{
    BalancePoint, PnlResolution, PortfolioMetrics, PortfolioRepository,
    ToggleFavoriteMarketRequest,
    cache::{order_store::OrderCacheConfig, user_store::UserBalanceCacheConfig},
    model::{
        orders::Model as OrderModel, user_balances::Model as UserBalanceModel,
        user_favorite_markets::Model as UserFavoriteMarketModel,
    },
    repository::repository::{PostgresTradingRepository, TradingError, TradingRepository},
};
use lib_utils::{AuthenticatedUserId, uuid_to_u64};

/// Setup test database connection
async fn setup_test_db() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://postgres:postgres123@localhost:5432/trading_db".to_string()
    });

    let connection_future = Database::connect(&database_url);
    let timeout_duration = std::time::Duration::from_secs(5);

    match tokio::time::timeout(timeout_duration, connection_future).await {
        Ok(Ok(db)) => Ok(db),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(sea_orm::DbErr::Custom(
            "Database connection timeout".to_string(),
        )),
    }
}

/// Setup database with all required migrations
async fn setup_test_db_with_schema() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let db = setup_test_db().await?;

    // Read and execute migrations
    let migrations_dir = PathBuf::from("/Users/nikhilsharma/code/testing-setup/migrations");

    // Execute migrations in order
    let migration_files = vec![
        "001_create_users_table.sql",
        "004_create_asset_table.sql",
        "006_create_markets_table.sql",
        "003_create_events_table.sql",
        "005_create_deposit_withdraw_table.sql",
        "007_create_user_favorite_markets_table.sql",
    ];

    for migration_file in migration_files {
        let migration_path = migrations_dir.join(migration_file);
        if migration_path.exists() {
            let mut sql = fs::read_to_string(&migration_path).map_err(|e| {
                sea_orm::DbErr::Custom(format!(
                    "Failed to read migration {}: {}",
                    migration_file, e
                ))
            })?;

            // Fix TIMESTAMP (without timezone) to TIMESTAMPTZ for created_at columns to match SeaORM expectations
            // This is needed because SeaORM expects TIMESTAMPTZ when using DateTime<Utc>
            // Only replace TIMESTAMP that's not already TIMESTAMPTZ or TIMESTAMP WITH TIME ZONE
            sql = sql.replace(
                "created_at TIMESTAMP DEFAULT",
                "created_at TIMESTAMPTZ DEFAULT",
            );
            sql = sql.replace("created_at TIMESTAMP,", "created_at TIMESTAMPTZ,");

            // Execute the entire migration file as one statement
            // PostgreSQL can handle multiple statements separated by semicolons
            // This is necessary because INSERT statements with VALUES can span multiple lines
            if let Err(e) = db.execute_unprepared(&sql).await {
                // Ignore "already exists" errors for idempotency (tables, indexes, etc.)
                let error_str = e.to_string();
                if error_str.contains("syntax error") || error_str.contains("unterminated") {
                    // Syntax and parse errors should be escalated - they indicate real problems
                    eprintln!(
                        "ERROR: Migration {} has syntax/parse error: {}",
                        migration_file, e
                    );
                } else if !error_str.contains("already exists")
                    && !error_str.contains("duplicate")
                    && !error_str.contains("does not exist")
                {
                    // Only warn on non-idempotency errors
                    eprintln!("Warning: Migration {} failed: {}", migration_file, e);
                }
            }
        }
    }

    // Explicitly ensure reference data (assets and markets) is populated
    // This is critical because tests depend on this data existing
    // We run these INSERTs separately to ensure they execute even if migration had errors
    ensure_reference_data(&db).await;

    Ok(db)
}

/// Ensure reference data (assets and markets) is populated
/// This is called after migrations to guarantee the data exists for tests
async fn ensure_reference_data(db: &DatabaseConnection) {
    // Insert assets if they don't exist
    let assets_insert = r#"
        INSERT INTO assets (asset_id, name, native_scale, active) VALUES
        (1, 'USD', 1000000, true),
        (2, 'BTC', 100000000, true),
        (3, 'ETH', 1000000000000000000, true)
        ON CONFLICT (asset_id) DO NOTHING;
    "#;

    if let Err(e) = db.execute_unprepared(assets_insert).await {
        eprintln!("Warning: Failed to ensure assets data: {}", e);
    }

    // Insert markets if they don't exist
    let markets_insert = r#"
        INSERT INTO markets (market_id, base_asset_name, quote_asset_name, base_asset, quote_asset, base_scale_k, quote_scale_k, taker_fee, maker_fee, slippage, active) VALUES
        ((1 << 16) | 2, 'BTC', 'USD', 2, 1, 10000, 100, 0, 0, 150, true),
        ((1 << 16) | 3, 'ETH', 'USD', 3, 1, 1000000000000000, 100, 0, 0, 150, true)
        ON CONFLICT (market_id) DO NOTHING;
    "#;

    if let Err(e) = db.execute_unprepared(markets_insert).await {
        eprintln!("Warning: Failed to ensure markets data: {}", e);
    }
}

/// Require database connection, return None if unavailable (test will be skipped)
async fn require_database() -> Option<DatabaseConnection> {
    setup_test_db_with_schema().await.ok()
}

/// Set up the tables exercised by the latest-event repository tests.
async fn require_latest_event_test_database() -> Option<DatabaseConnection> {
    let db = require_database().await?;
    let schema = r#"
        CREATE SCHEMA IF NOT EXISTS trading;

        CREATE TABLE IF NOT EXISTS trading.orders_events (
            id BIGSERIAL PRIMARY KEY,
            market_id INTEGER NOT NULL,
            order_order_id BIGINT NOT NULL,
            order_user_id BIGINT NOT NULL,
            order_price BIGINT NOT NULL,
            order_size BIGINT NOT NULL,
            order_filled_size BIGINT NOT NULL,
            order_side TEXT NOT NULL,
            order_time_in_force TEXT NOT NULL,
            order_status TEXT NOT NULL,
            order_timestamp BIGINT NOT NULL,
            created_at TIMESTAMP NULL
        );

        CREATE TABLE IF NOT EXISTS trading.balances_events (
            id BIGSERIAL PRIMARY KEY,
            user_id BIGINT NOT NULL,
            asset_id INTEGER NOT NULL,
            available BIGINT NOT NULL,
            locked BIGINT NOT NULL,
            total BIGINT NOT NULL,
            timestamp BIGINT NOT NULL,
            created_at TIMESTAMP NULL
        );
    "#;

    if let Err(error) = db.execute_unprepared(schema).await {
        eprintln!("Skipping latest-event tests: failed to create test tables: {error}");
        return None;
    }

    Some(db)
}

/// Cleanup test data for a specific user
async fn cleanup_test_data(db: &DatabaseConnection, user_id: u64) {
    // Clean up in reverse dependency order
    let cleanup_queries = vec![
        format!(
            "DELETE FROM user_favorite_markets WHERE user_id = {}",
            user_id
        ),
        format!(
            "DELETE FROM trading.orders_events WHERE order_user_id = {}",
            user_id
        ),
        format!(
            "DELETE FROM trading.balances_events WHERE user_id = {}",
            user_id
        ),
        format!(
            "DELETE FROM trading.deposits_events WHERE user_id = {}",
            user_id
        ),
        format!(
            "DELETE FROM trading.withdrawals_events WHERE user_id = {}",
            user_id
        ),
    ];

    for query in cleanup_queries {
        if let Err(e) = db.execute_unprepared(&query).await {
            eprintln!("Warning: Cleanup query failed: {} - {}", query, e);
        }
    }
}

/// Create a test user (UUID)
fn create_test_user() -> Uuid {
    Uuid::new_v4()
}

/// The user of one database test. Its `uuid_to_u64` is `id`, so it always fits the BIGINT user
/// columns (a random UUID's exceeds `i64::MAX` half the time) and every run uses the same rows.
/// Each test passes its own `id`, so tests running in parallel never share rows.
fn database_test_user(id: u32) -> Uuid {
    Uuid::from_u128(id.into())
}

/// Create AuthenticatedUserId from UUID
fn create_authenticated_user_id(uuid: Uuid) -> AuthenticatedUserId {
    AuthenticatedUserId(uuid)
}

/// Insert test balance data
async fn insert_test_balance(
    db: &DatabaseConnection,
    user_id: u64,
    asset_id: i32,
    available: u64,
    locked: u64,
) -> Result<(), sea_orm::DbErr> {
    let total = available + locked;
    let timestamp = Utc::now().timestamp();

    insert_test_balance_at(db, user_id, asset_id, available, locked, total, timestamp).await
}

#[allow(clippy::too_many_arguments)]
async fn insert_test_balance_at(
    db: &DatabaseConnection,
    user_id: u64,
    asset_id: i32,
    available: u64,
    locked: u64,
    total: u64,
    timestamp: i64,
) -> Result<(), sea_orm::DbErr> {
    // Note: created_at is optional, so we'll set it to NULL to avoid type mismatch issues
    // The table uses TIMESTAMP but SeaORM expects TIMESTAMPTZ for DateTime<Utc>
    let sql = format!(
        r#"
        INSERT INTO trading.balances_events (user_id, asset_id, available, locked, total, timestamp, created_at)
        VALUES ({}, {}, {}, {}, {}, {}, NULL)
        "#,
        user_id, asset_id, available, locked, total, timestamp
    );

    db.execute_unprepared(&sql).await?;
    Ok(())
}

/// Insert test order data
#[allow(clippy::too_many_arguments)]
async fn insert_test_order(
    db: &DatabaseConnection,
    order_id: u64,
    user_id: u64,
    market_id: i32,
    price: u64,
    size: u64,
    side: &str,
    status: &str,
) -> Result<(), sea_orm::DbErr> {
    let timestamp = Utc::now().timestamp();

    insert_test_order_at(
        db, order_id, user_id, market_id, price, size, side, status, timestamp,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_test_order_at(
    db: &DatabaseConnection,
    order_id: u64,
    user_id: u64,
    market_id: i32,
    price: u64,
    size: u64,
    side: &str,
    status: &str,
    timestamp: i64,
) -> Result<(), sea_orm::DbErr> {
    // Note: created_at is optional; order_filled_size defaults to 0 for open/history tests.
    // Side, time in force and status are vex-core's enum names (SIDE_*, TIF_*, STATUS_*).
    let sql = format!(
        r#"
        INSERT INTO trading.orders_events 
        (market_id, order_order_id, order_user_id, order_price, order_size, order_filled_size, order_side, order_time_in_force, order_status, order_timestamp, created_at)
        VALUES ({}, {}, {}, {}, {}, 0, '{}', 'TIF_GTC', '{}', {}, NULL)
        "#,
        market_id, order_id, user_id, price, size, side, status, timestamp
    );

    db.execute_unprepared(&sql).await?;
    Ok(())
}

/// Mock TradingRepository that returns errors for testing error handling
#[derive(Clone)]
struct ErrorMockTradingRepository {
    error_method: Option<&'static str>, // Method name that should return error
    total_count: u64,
}

impl ErrorMockTradingRepository {
    fn new() -> Self {
        Self {
            error_method: None,
            total_count: 0,
        }
    }

    fn with_error_on(mut self, method: &'static str) -> Self {
        self.error_method = Some(method);
        self
    }

    fn with_total_count(mut self, total_count: u64) -> Self {
        self.total_count = total_count;
        self
    }

    fn should_error(&self, method: &str) -> bool {
        self.error_method.is_some_and(|m| m == method)
    }
}

#[async_trait]
impl TradingRepository for ErrorMockTradingRepository {
    async fn get_user_balances(
        &self,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<Vec<UserBalanceModel>> {
        if self.should_error("get_user_balances") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(vec![])
    }

    async fn get_user_balance(
        &self,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<Option<UserBalanceModel>> {
        if self.should_error("get_user_balance") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(None)
    }

    async fn update_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> lib_core::repository::repository::Result<()> {
        if self.should_error("update_user_balance") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(())
    }

    async fn create_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> lib_core::repository::repository::Result<()> {
        if self.should_error("create_user_balance") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(())
    }

    async fn get_open_orders(
        &self,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<Vec<OrderModel>> {
        if self.should_error("get_open_orders") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(vec![])
    }

    async fn get_open_orders_paginated(
        &self,
        _user_id: u64,
        _offset: u64,
        _limit: u64,
        _market_id: Option<u32>,
    ) -> lib_core::repository::repository::Result<Vec<OrderModel>> {
        if self.should_error("get_open_orders_paginated") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(vec![])
    }

    async fn count_open_orders(
        &self,
        _user_id: u64,
        _market_id: Option<u32>,
    ) -> lib_core::repository::repository::Result<u64> {
        if self.should_error("count_open_orders") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(self.total_count)
    }

    async fn get_order_by_id(
        &self,
        _order_id: u64,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<Option<OrderModel>> {
        if self.should_error("get_order_by_id") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(None)
    }

    async fn create_order(
        &self,
        _order: OrderModel,
    ) -> lib_core::repository::repository::Result<u64> {
        if self.should_error("create_order") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(0)
    }

    async fn count_user_orders(
        &self,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<u64> {
        if self.should_error("count_user_orders") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(0)
    }

    async fn get_order_history(
        &self,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<Vec<OrderModel>> {
        if self.should_error("get_order_history") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(vec![])
    }

    async fn get_order_history_paginated(
        &self,
        _user_id: u64,
        _offset: u64,
        _limit: u64,
        _market_id: Option<u32>,
    ) -> lib_core::repository::repository::Result<Vec<OrderModel>> {
        if self.should_error("get_order_history_paginated") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(vec![])
    }

    async fn count_order_history(
        &self,
        _user_id: u64,
        _market_id: Option<u32>,
    ) -> lib_core::repository::repository::Result<u64> {
        if self.should_error("count_order_history") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(self.total_count)
    }

    async fn get_user_favorite_markets(
        &self,
        _user_id: u64,
    ) -> lib_core::repository::repository::Result<Vec<UserFavoriteMarketModel>> {
        if self.should_error("get_user_favorite_markets") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(vec![])
    }

    async fn add_favorite_market(
        &self,
        _user_id: u64,
        _market_id: u32,
    ) -> lib_core::repository::repository::Result<()> {
        if self.should_error("add_favorite_market") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(())
    }

    async fn remove_favorite_market(
        &self,
        _user_id: u64,
        _market_id: u32,
    ) -> lib_core::repository::repository::Result<()> {
        if self.should_error("remove_favorite_market") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(())
    }

    async fn is_market_favorited(
        &self,
        _user_id: u64,
        _market_id: u32,
    ) -> lib_core::repository::repository::Result<bool> {
        if self.should_error("is_market_favorited") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(false)
    }

    async fn toggle_favorite_market(
        &self,
        _user_id: u64,
        _market_id: u32,
    ) -> lib_core::repository::repository::Result<bool> {
        if self.should_error("toggle_favorite_market") {
            return Err(TradingError::Database(
                "Database connection failed".to_string(),
            ));
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lib_core::cache::{order_store::OrderStore, user_store::UserBalanceStore};
    use std::sync::Mutex;
    use tracing::{
        Event, Metadata, Subscriber,
        field::{Field, Visit},
        span::{Attributes, Id, Record},
    };

    const DATABASE_DETAIL: &str = "SELECT secret FROM api_keys";

    struct ErrorPortfolioRepository;

    #[async_trait]
    impl PortfolioRepository for ErrorPortfolioRepository {
        async fn get_portfolio(
            &self,
            _user_id: u64,
            _now_ts_secs: i64,
            _resolution: PnlResolution,
        ) -> lib_core::repository::repository::Result<PortfolioMetrics> {
            Err(TradingError::Database(format!(
                "relation missing while running {DATABASE_DETAIL}"
            )))
        }

        async fn get_balance_chart(
            &self,
            _user_id: u64,
            _now_ts_secs: i64,
            _resolution: PnlResolution,
        ) -> lib_core::repository::repository::Result<Vec<BalancePoint>> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone)]
    struct CapturingSubscriber {
        events: Arc<Mutex<Vec<String>>>,
    }

    struct FieldVisitor<'a> {
        output: &'a mut String,
    }

    impl Visit for FieldVisitor<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.output
                .push_str(&format!(" {}={value:?}", field.name()));
        }
    }

    impl Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut output = String::new();
            event.record(&mut FieldVisitor {
                output: &mut output,
            });
            self.events.lock().unwrap().push(output);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[tokio::test]
    async fn test_extreme_page_returns_empty_page_without_overflow() {
        let user = create_authenticated_user_id(create_test_user());
        let repo = Arc::new(ErrorMockTradingRepository::new());
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            repo.clone(),
        ));

        let open_response = get_open_orders(
            user.clone(),
            web::Query(OrdersQuery {
                page: Some(usize::MAX),
                limit: Some(100),
                market_id: None,
            }),
            web::Data::new(order_store.clone()),
            web::Data::new(repo.clone() as Arc<dyn TradingRepository>),
        )
        .await
        .unwrap();
        let open_body = actix_web::body::to_bytes(open_response.into_body())
            .await
            .unwrap();
        let open_json: serde_json::Value = serde_json::from_slice(&open_body).unwrap();

        assert_eq!(open_json["page"], 10_000);
        assert_eq!(open_json["limit"], 100);
        assert_eq!(open_json["orders"], serde_json::json!([]));

        let history_response = get_order_history(
            user,
            web::Query(OrdersQuery {
                page: Some(usize::MAX),
                limit: Some(100),
                market_id: None,
            }),
            web::Data::new(order_store),
            web::Data::new(repo as Arc<dyn TradingRepository>),
        )
        .await
        .unwrap();
        let history_body = actix_web::body::to_bytes(history_response.into_body())
            .await
            .unwrap();
        let history_json: serde_json::Value = serde_json::from_slice(&history_body).unwrap();

        assert_eq!(history_json["page"], 10_000);
        assert_eq!(history_json["limit"], 100);
        assert_eq!(history_json["orders"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn test_has_next_is_false_at_max_page() {
        let user = create_authenticated_user_id(create_test_user());
        let repo = Arc::new(ErrorMockTradingRepository::new().with_total_count(1_000_001));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            repo.clone(),
        ));

        let open_response = get_open_orders(
            user.clone(),
            web::Query(OrdersQuery {
                page: Some(10_000),
                limit: Some(100),
                market_id: None,
            }),
            web::Data::new(order_store.clone()),
            web::Data::new(repo.clone() as Arc<dyn TradingRepository>),
        )
        .await
        .unwrap();
        let open_body = actix_web::body::to_bytes(open_response.into_body())
            .await
            .unwrap();
        let open_json: serde_json::Value = serde_json::from_slice(&open_body).unwrap();

        assert_eq!(open_json["page"], 10_000);
        assert_eq!(open_json["total_pages"], 10_001);
        assert_eq!(open_json["has_next"], false);

        let history_response = get_order_history(
            user,
            web::Query(OrdersQuery {
                page: Some(10_000),
                limit: Some(100),
                market_id: None,
            }),
            web::Data::new(order_store),
            web::Data::new(repo as Arc<dyn TradingRepository>),
        )
        .await
        .unwrap();
        let history_body = actix_web::body::to_bytes(history_response.into_body())
            .await
            .unwrap();
        let history_json: serde_json::Value = serde_json::from_slice(&history_body).unwrap();

        assert_eq!(history_json["page"], 10_000);
        assert_eq!(history_json["total_pages"], 10_001);
        assert_eq!(history_json["has_next"], false);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_database_error_is_generic_for_client_and_detailed_in_log() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            events: events.clone(),
        };
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let user = create_authenticated_user_id(create_test_user());
        let repo: Arc<dyn PortfolioRepository> = Arc::new(ErrorPortfolioRepository);

        let response = get_portfolio_overview(user, web::Data::new(repo))
            .await
            .unwrap();
        assert_eq!(response.status(), 500);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let body_text = std::str::from_utf8(&body).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["error"], "failed to retrieve portfolio");
        assert_eq!(
            json["message"],
            "An internal error occurred. Please try again later."
        );
        assert!(!body_text.contains(DATABASE_DETAIL));
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.contains(DATABASE_DETAIL))
        );
    }

    #[tokio::test]
    async fn test_postgres_order_queries_use_latest_event_per_order() {
        let Some(db) = require_latest_event_test_database().await else {
            return;
        };
        let user_id = uuid_to_u64(create_test_user()) & i64::MAX as u64;
        let timestamp = Utc::now().timestamp();

        cleanup_test_data(&db, user_id).await;

        for status in ["STATUS_PLACED", "STATUS_PARTIALLY_FILLED", "STATUS_FILLED"] {
            insert_test_order_at(
                &db, 91001, user_id, 65538, 5_000_000, 100, "SIDE_BID", status, timestamp,
            )
            .await
            .unwrap();
        }
        for status in ["STATUS_PLACED", "STATUS_FILLED"] {
            insert_test_order_at(
                &db, 91002, user_id, 65538, 5_100_000, 200, "SIDE_ASK", status, timestamp,
            )
            .await
            .unwrap();
        }
        for status in ["STATUS_PLACED", "STATUS_PARTIALLY_FILLED"] {
            insert_test_order_at(
                &db, 91003, user_id, 65539, 3_000_000, 150, "SIDE_BID", status, timestamp,
            )
            .await
            .unwrap();
        }

        let repo = PostgresTradingRepository::new(Arc::new(db.clone()));
        let history = repo
            .get_order_history_paginated(user_id, 0, 10, None)
            .await
            .unwrap();
        let transitioned: Vec<_> = history
            .iter()
            .filter(|order| order.order_order_id == 91001)
            .collect();

        assert_eq!(transitioned.len(), 1);
        assert_eq!(transitioned[0].order_status, "STATUS_FILLED");
        assert_eq!(repo.count_order_history(user_id, None).await.unwrap(), 3);
        assert_eq!(history.len(), 3);
        assert_eq!(repo.count_user_orders(user_id).await.unwrap(), 3);
        assert_eq!(repo.get_order_history(user_id).await.unwrap().len(), 3);
        assert_eq!(
            repo.get_order_by_id(91001, user_id)
                .await
                .unwrap()
                .unwrap()
                .order_status,
            "STATUS_FILLED"
        );

        let open_orders = repo
            .get_open_orders_paginated(user_id, 0, 10, None)
            .await
            .unwrap();
        assert_eq!(open_orders.len(), 1);
        assert_eq!(open_orders[0].order_order_id, 91003);
        assert_eq!(repo.count_open_orders(user_id, None).await.unwrap(), 1);
        assert_eq!(repo.get_open_orders(user_id).await.unwrap().len(), 1);
        assert!(
            !open_orders
                .iter()
                .any(|order| order.order_order_id == 91002)
        );

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_postgres_open_orders_exclude_partially_filled_ioc() {
        let Some(db) = require_latest_event_test_database().await else {
            return;
        };
        let user_id = uuid_to_u64(create_test_user()) & i64::MAX as u64;
        let timestamp = Utc::now().timestamp();

        cleanup_test_data(&db, user_id).await;

        // The engine cancels an IOC remainder, so a partially filled IOC order is not open.
        for (order_id, time_in_force) in [(92001, "TIF_IOC"), (92002, "TIF_GTC")] {
            let sql = format!(
                r#"
                INSERT INTO trading.orders_events
                (market_id, order_order_id, order_user_id, order_price, order_size, order_filled_size, order_side, order_time_in_force, order_status, order_timestamp, created_at)
                VALUES (65538, {order_id}, {user_id}, 5000000, 100, 40, 'SIDE_BID', '{time_in_force}', 'STATUS_PARTIALLY_FILLED', {timestamp}, NULL)
                "#
            );
            db.execute_unprepared(&sql).await.unwrap();
        }

        let repo = PostgresTradingRepository::new(Arc::new(db.clone()));
        let open_orders = repo.get_open_orders(user_id).await.unwrap();
        assert_eq!(open_orders.len(), 1);
        assert_eq!(open_orders[0].order_order_id, 92002);
        assert_eq!(repo.count_open_orders(user_id, None).await.unwrap(), 1);

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_postgres_balances_use_latest_event_per_asset() {
        let Some(db) = require_latest_event_test_database().await else {
            return;
        };
        let user_id = uuid_to_u64(create_test_user()) & i64::MAX as u64;
        let timestamp = Utc::now().timestamp();

        cleanup_test_data(&db, user_id).await;
        insert_test_balance_at(&db, user_id, 1, 1_000, 100, 1_100, timestamp)
            .await
            .unwrap();
        insert_test_balance_at(&db, user_id, 1, 700, 400, 1_100, timestamp)
            .await
            .unwrap();
        insert_test_balance_at(&db, user_id, 2, 2_000, 0, 2_000, timestamp)
            .await
            .unwrap();

        let repo = PostgresTradingRepository::new(Arc::new(db.clone()));
        let balances = repo.get_user_balances(user_id).await.unwrap();

        assert_eq!(balances.len(), 2);
        let asset = balances
            .iter()
            .find(|balance| balance.asset_id == 1)
            .unwrap();
        assert_eq!(asset.available, 700);
        assert_eq!(asset.locked, 400);

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_balances_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_001);
        let user_id = uuid_to_u64(user);

        // Clean up before test
        cleanup_test_data(&db, user_id).await;

        // Insert test balances
        insert_test_balance(&db, user_id, 1, 1000000, 50000)
            .await
            .unwrap(); // USD
        insert_test_balance(&db, user_id, 2, 5000000, 100000)
            .await
            .unwrap(); // BTC

        // Setup stores
        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let balance_store = Arc::new(UserBalanceStore::with_repository(
            UserBalanceCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);

        // Call handler
        let result = get_balances(authenticated_user, web::Data::new(balance_store.clone())).await;

        match result {
            Ok(response) => {
                let status = response.status();
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();

                if status != 200 {
                    let error_response: serde_json::Value =
                        serde_json::from_slice(&bytes).unwrap_or_default();
                    panic!(
                        "Handler returned status {}: {}",
                        status,
                        serde_json::to_string_pretty(&error_response).unwrap_or_default()
                    );
                }

                // Parse response
                let balances_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(balances_response["total_assets"], 2);
                assert!(balances_response["balances"].is_array());
                let balances = balances_response["balances"].as_array().unwrap();
                assert_eq!(balances.len(), 2);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_balances_empty() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_002);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let balance_store = Arc::new(UserBalanceStore::with_repository(
            UserBalanceCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);

        let result = get_balances(authenticated_user, web::Data::new(balance_store.clone())).await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let balances_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(balances_response["total_assets"], 0);
                assert!(balances_response["balances"].is_array());
                let balances = balances_response["balances"].as_array().unwrap();
                assert_eq!(balances.len(), 0);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_open_orders_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_003);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert test orders - mix of open and filled
        insert_test_order(
            &db,
            1001,
            user_id,
            65538,
            5000000,
            100,
            "SIDE_BID",
            "STATUS_PLACED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            1002,
            user_id,
            65538,
            5100000,
            200,
            "SIDE_ASK",
            "STATUS_PROCESSING",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            1003,
            user_id,
            65539,
            3000000,
            150,
            "SIDE_BID",
            "STATUS_FILLED",
        )
        .await
        .unwrap(); // Should not appear

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_open_orders(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let orders_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(orders_response["orders"].is_array());
                let orders = orders_response["orders"].as_array().unwrap();
                // Should have 2 open orders (Placed and Processing), not the Filled one
                assert_eq!(orders.len(), 2);
                assert_eq!(orders_response["total"], 2);
                assert_eq!(orders_response["page"], 1);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_open_orders_pagination() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_004);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert multiple orders
        for i in 0..5 {
            insert_test_order(
                &db,
                2000 + i,
                user_id,
                65538,
                5000000 + (i * 100000),
                100,
                "SIDE_BID",
                "STATUS_PLACED",
            )
            .await
            .unwrap();
        }

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(2), // Only 2 per page
            market_id: None,
        });

        let result = get_open_orders(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let orders_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(orders_response["total"], 5);
                assert_eq!(orders_response["page"], 1);
                assert_eq!(orders_response["limit"], 2);
                assert_eq!(orders_response["total_pages"], 3); // 5 orders / 2 per page = 3 pages
                assert_eq!(orders_response["has_next"], true);
                assert_eq!(orders_response["has_prev"], false);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_open_orders_all_statuses() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_005);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert test orders with all open statuses and some terminal statuses
        insert_test_order(
            &db,
            4001,
            user_id,
            65538,
            5000000,
            100,
            "SIDE_BID",
            "STATUS_PLACED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            4002,
            user_id,
            65538,
            5100000,
            200,
            "SIDE_ASK",
            "STATUS_PROCESSING",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            4003,
            user_id,
            65538,
            5200000,
            150,
            "SIDE_BID",
            "STATUS_PARTIALLY_FILLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            4004,
            user_id,
            65539,
            3000000,
            200,
            "SIDE_ASK",
            "STATUS_PROCESSING",
        )
        .await
        .unwrap();
        // Terminal statuses - should not appear in open orders
        insert_test_order(
            &db,
            4005,
            user_id,
            65538,
            5300000,
            250,
            "SIDE_BID",
            "STATUS_FILLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            4006,
            user_id,
            65538,
            5400000,
            300,
            "SIDE_ASK",
            "STATUS_CANCELLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            4007,
            user_id,
            65539,
            3500000,
            100,
            "SIDE_BID",
            "STATUS_REJECTED",
        )
        .await
        .unwrap();

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_open_orders(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let orders_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(orders_response["orders"].is_array());
                let orders = orders_response["orders"].as_array().unwrap();
                // Should have 4 open orders: Placed, PartiallyFilled and two Processing
                // Should NOT include: Filled, Cancelled, Rejected
                assert_eq!(orders.len(), 4);
                assert_eq!(orders_response["total"], 4);

                // Verify all returned orders have open statuses
                let statuses: Vec<String> = orders
                    .iter()
                    .map(|o| o["status"].as_str().unwrap().to_string())
                    .collect();

                // Check that all open statuses are present
                assert!(statuses.iter().any(|s| s.eq_ignore_ascii_case("Placed")));
                assert!(
                    statuses
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case("PartiallyFilled"))
                );
                assert!(
                    statuses
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case("Processing"))
                );

                // Verify terminal statuses are NOT present
                assert!(!statuses.iter().any(|s| s.eq_ignore_ascii_case("Filled")));
                assert!(!statuses.iter().any(|s| s.eq_ignore_ascii_case("Cancelled")));
                assert!(!statuses.iter().any(|s| s.eq_ignore_ascii_case("Rejected")));
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_order_history_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_006);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert test orders - mix of history and open
        insert_test_order(
            &db,
            3001,
            user_id,
            65538,
            5000000,
            100,
            "SIDE_BID",
            "STATUS_FILLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            3002,
            user_id,
            65538,
            5100000,
            200,
            "SIDE_ASK",
            "STATUS_CANCELLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            3003,
            user_id,
            65539,
            3000000,
            150,
            "SIDE_BID",
            "STATUS_PARTIALLY_FILLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            3004,
            user_id,
            65538,
            5200000,
            250,
            "SIDE_ASK",
            "STATUS_PLACED",
        )
        .await
        .unwrap(); // Should not appear

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_order_history(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let history_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(history_response["orders"].is_array());
                let orders = history_response["orders"].as_array().unwrap();
                // Should have 3 history orders (Filled, Cancelled, PartiallyFilled), not the Placed one
                assert_eq!(orders.len(), 3);
                assert_eq!(history_response["total"], 3);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_order_history_with_rejected() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_007);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert test orders - terminal statuses
        // Note: Repository currently only includes Filled, Cancelled, and PartiallyFilled
        // Rejected is a terminal state but is NOT currently included in history (may be a bug)
        insert_test_order(
            &db,
            5001,
            user_id,
            65538,
            5000000,
            100,
            "SIDE_BID",
            "STATUS_FILLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            5002,
            user_id,
            65538,
            5100000,
            200,
            "SIDE_ASK",
            "STATUS_CANCELLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            5003,
            user_id,
            65539,
            3000000,
            150,
            "SIDE_BID",
            "STATUS_PARTIALLY_FILLED",
        )
        .await
        .unwrap();
        insert_test_order(
            &db,
            5004,
            user_id,
            65538,
            5200000,
            250,
            "SIDE_ASK",
            "STATUS_REJECTED",
        )
        .await
        .unwrap();
        // Open status - should not appear in history
        insert_test_order(
            &db,
            5005,
            user_id,
            65538,
            5300000,
            300,
            "SIDE_BID",
            "STATUS_PLACED",
        )
        .await
        .unwrap();

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_order_history(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let history_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(history_response["orders"].is_array());
                let orders = history_response["orders"].as_array().unwrap();
                // Should have 3 history orders: Filled, Cancelled, PartiallyFilled
                // Note: Rejected is NOT currently included (repository filters it out)
                // Should NOT include: Placed (open status), Rejected (not in repository filter)
                assert_eq!(orders.len(), 3);
                assert_eq!(history_response["total"], 3);

                // Verify all returned orders have terminal statuses that are included
                let statuses: Vec<String> = orders
                    .iter()
                    .map(|o| o["status"].as_str().unwrap().to_string())
                    .collect();

                // Check that included terminal statuses are present
                assert!(statuses.iter().any(|s| s.eq_ignore_ascii_case("Filled")));
                assert!(statuses.iter().any(|s| s.eq_ignore_ascii_case("Cancelled")));
                assert!(
                    statuses
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case("PartiallyFilled"))
                );

                // Verify Rejected is NOT present (current repository implementation excludes it)
                assert!(!statuses.iter().any(|s| s.eq_ignore_ascii_case("Rejected")));

                // Verify open statuses are NOT present
                assert!(!statuses.iter().any(|s| s.eq_ignore_ascii_case("Placed")));
                assert!(
                    !statuses
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case("Processing"))
                );
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_open_orders_with_market_id_filter() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_008);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert test orders across different markets
        insert_test_order(
            &db,
            6001,
            user_id,
            65538,
            5000000,
            100,
            "SIDE_BID",
            "STATUS_PLACED",
        )
        .await
        .unwrap(); // Market 65538
        insert_test_order(
            &db,
            6002,
            user_id,
            65538,
            5100000,
            200,
            "SIDE_ASK",
            "STATUS_PROCESSING",
        )
        .await
        .unwrap(); // Market 65538
        insert_test_order(
            &db,
            6003,
            user_id,
            65539,
            3000000,
            150,
            "SIDE_BID",
            "STATUS_PLACED",
        )
        .await
        .unwrap(); // Market 65539 - should be filtered out
        insert_test_order(
            &db,
            6004,
            user_id,
            65538,
            5200000,
            250,
            "SIDE_ASK",
            "STATUS_PARTIALLY_FILLED",
        )
        .await
        .unwrap(); // Market 65538

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: Some(65538), // Filter by market_id
        });

        let result = get_open_orders(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let orders_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(orders_response["orders"].is_array());
                let orders = orders_response["orders"].as_array().unwrap();
                // Should have 3 orders from market 65538 (Placed, Processing, PartiallyFilled)
                // Should NOT include order from market 65539
                assert_eq!(orders.len(), 3);
                assert_eq!(orders_response["total"], 3);

                // Verify all returned orders are from the correct market
                for order in orders {
                    assert_eq!(order["market_id"], 65538);
                }
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_order_history_with_market_id_filter() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_009);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert test orders across different markets
        insert_test_order(
            &db,
            7001,
            user_id,
            65538,
            5000000,
            100,
            "SIDE_BID",
            "STATUS_FILLED",
        )
        .await
        .unwrap(); // Market 65538
        insert_test_order(
            &db,
            7002,
            user_id,
            65538,
            5100000,
            200,
            "SIDE_ASK",
            "STATUS_CANCELLED",
        )
        .await
        .unwrap(); // Market 65538
        insert_test_order(
            &db,
            7003,
            user_id,
            65539,
            3000000,
            150,
            "SIDE_BID",
            "STATUS_PARTIALLY_FILLED",
        )
        .await
        .unwrap(); // Market 65539 - should be filtered out
        insert_test_order(
            &db,
            7004,
            user_id,
            65538,
            5200000,
            250,
            "SIDE_ASK",
            "STATUS_FILLED",
        )
        .await
        .unwrap(); // Market 65538

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            trading_repo.clone(),
        ));

        let authenticated_user = create_authenticated_user_id(user);
        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: Some(65538), // Filter by market_id
        });

        let result = get_order_history(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let history_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(history_response["orders"].is_array());
                let orders = history_response["orders"].as_array().unwrap();
                // Should have 3 orders from market 65538 (Filled, Cancelled, Filled)
                // Should NOT include order from market 65539
                assert_eq!(orders.len(), 3);
                assert_eq!(history_response["total"], 3);

                // Verify all returned orders are from the correct market
                for order in orders {
                    assert_eq!(order["market_id"], 65538);
                }
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_favorite_markets_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_010);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // Insert favorite markets
        // Note: PostgreSQL BIGINT is signed (i64), so we need to cast u64 to i64
        // Mask to lower 63 bits to avoid producing negative values from high-bit u64s
        let user_id_i64 = (user_id & 0x7FFF_FFFF_FFFF_FFFF) as i64;
        let market_ids = vec![65538, 65539]; // BTC/USD and ETH/USD
        for market_id in &market_ids {
            let sql = format!(
                "INSERT INTO user_favorite_markets (user_id, market_id) VALUES ({}, {})",
                user_id_i64, market_id
            );
            db.execute_unprepared(&sql).await.unwrap();
        }

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));

        let authenticated_user = create_authenticated_user_id(user);

        let result = get_favorite_markets(
            authenticated_user,
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let favorites_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(favorites_response["market_ids"].is_array());
                let market_ids_response = favorites_response["market_ids"].as_array().unwrap();
                assert_eq!(market_ids_response.len(), 2);
                assert!(market_ids_response.contains(&serde_json::json!(65538)));
                assert!(market_ids_response.contains(&serde_json::json!(65539)));
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_get_favorite_markets_empty() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_011);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));

        let authenticated_user = create_authenticated_user_id(user);

        let result = get_favorite_markets(
            authenticated_user,
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let favorites_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert!(favorites_response["market_ids"].is_array());
                let market_ids_response = favorites_response["market_ids"].as_array().unwrap();
                assert_eq!(market_ids_response.len(), 0);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_toggle_favorite_market_add() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_012);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));

        let authenticated_user = create_authenticated_user_id(user);
        let request = web::Json(ToggleFavoriteMarketRequest { market_id: 65538 });

        let result = toggle_favorite_market(
            authenticated_user,
            request,
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let toggle_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(toggle_response["success"], true);
                assert_eq!(toggle_response["action"], "added");
                assert_eq!(toggle_response["market_id"], 65538);

                // Verify it was actually added
                let favorites = trading_repo
                    .get_user_favorite_markets(user_id)
                    .await
                    .unwrap();
                assert_eq!(favorites.len(), 1);
                assert_eq!(favorites[0].market_id, 65538);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    #[ignore = "required test schema is unavailable in CI"]
    async fn test_toggle_favorite_market_remove() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_013);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        // First add a favorite
        // Note: PostgreSQL BIGINT is signed (i64), so we need to cast u64 to i64
        // Mask to lower 63 bits to avoid producing negative values from high-bit u64s
        let user_id_i64 = (user_id & 0x7FFF_FFFF_FFFF_FFFF) as i64;
        let sql = format!(
            "INSERT INTO user_favorite_markets (user_id, market_id) VALUES ({}, {})",
            user_id_i64, 65538
        );
        db.execute_unprepared(&sql).await.unwrap();

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));

        let authenticated_user = create_authenticated_user_id(user);
        let request = web::Json(ToggleFavoriteMarketRequest { market_id: 65538 });

        // Now remove it
        let result = toggle_favorite_market(
            authenticated_user,
            request,
            web::Data::new(trading_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let toggle_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(toggle_response["success"], true);
                assert_eq!(toggle_response["action"], "removed");
                assert_eq!(toggle_response["market_id"], 65538);

                // Verify it was actually removed
                let favorites = trading_repo
                    .get_user_favorite_markets(user_id)
                    .await
                    .unwrap();
                assert_eq!(favorites.len(), 0);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    // ============================================================================
    // Error Case Tests
    // ============================================================================

    #[tokio::test]
    async fn test_get_balances_database_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // Create mock repository that returns error
        let mock_repo =
            Arc::new(ErrorMockTradingRepository::new().with_error_on("get_user_balances"));
        let balance_store = Arc::new(UserBalanceStore::with_repository(
            UserBalanceCacheConfig::default(),
            mock_repo.clone(),
        ));

        let result = get_balances(authenticated_user, web::Data::new(balance_store.clone())).await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(error_response["error"], "failed to retrieve balances");
                assert!(error_response["message"].is_string());
                // Handlers return sanitized generic error message, not internal DB errors
                assert_eq!(
                    error_response["message"],
                    "An internal error occurred. Please try again later."
                );
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_get_open_orders_count_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // Create mock repository that returns error on count_open_orders
        let mock_repo =
            Arc::new(ErrorMockTradingRepository::new().with_error_on("count_open_orders"));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            mock_repo.clone(),
        ));

        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_open_orders(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(mock_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(error_response["error"], "failed to retrieve open orders");
                assert!(error_response["message"].is_string());
                // Handlers return sanitized generic error message, not internal DB errors
                assert_eq!(
                    error_response["message"],
                    "An internal error occurred. Please try again later."
                );
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_get_open_orders_store_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // Create mock repository that returns error on get_open_orders_paginated
        let mock_repo =
            Arc::new(ErrorMockTradingRepository::new().with_error_on("get_open_orders_paginated"));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            mock_repo.clone(),
        ));

        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_open_orders(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(mock_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(error_response["error"], "failed to retrieve open orders");
                assert!(error_response["message"].is_string());
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_get_order_history_count_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // Create mock repository that returns error on count_order_history
        let mock_repo =
            Arc::new(ErrorMockTradingRepository::new().with_error_on("count_order_history"));
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            mock_repo.clone(),
        ));

        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_order_history(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(mock_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(error_response["error"], "failed to retrieve order history");
                assert!(error_response["message"].is_string());
                // Handlers return sanitized generic error message, not internal DB errors
                assert_eq!(
                    error_response["message"],
                    "An internal error occurred. Please try again later."
                );
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_get_order_history_store_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // Create mock repository that returns error on get_order_history_paginated
        let mock_repo = Arc::new(
            ErrorMockTradingRepository::new().with_error_on("get_order_history_paginated"),
        );
        let order_store = Arc::new(OrderStore::with_repository(
            OrderCacheConfig::default(),
            mock_repo.clone(),
        ));

        let query = web::Query(OrdersQuery {
            page: Some(1),
            limit: Some(20),
            market_id: None,
        });

        let result = get_order_history(
            authenticated_user,
            query,
            web::Data::new(order_store.clone()),
            web::Data::new(mock_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(error_response["error"], "failed to retrieve order history");
                assert!(error_response["message"].is_string());
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_get_favorite_markets_database_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // Create mock repository that returns error
        let mock_repo =
            Arc::new(ErrorMockTradingRepository::new().with_error_on("get_user_favorite_markets"));

        let result = get_favorite_markets(
            authenticated_user,
            web::Data::new(mock_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(
                    error_response["error"],
                    "failed to retrieve favorite markets"
                );
                assert!(error_response["message"].is_string());
                // Handlers return sanitized generic error message, not internal DB errors
                assert_eq!(
                    error_response["message"],
                    "An internal error occurred. Please try again later."
                );
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_toggle_favorite_market_error() {
        let user = create_test_user();
        let authenticated_user = create_authenticated_user_id(user);

        // The handler makes a single atomic repository call; fail that one.
        let mock_repo =
            Arc::new(ErrorMockTradingRepository::new().with_error_on("toggle_favorite_market"));

        let request = web::Json(ToggleFavoriteMarketRequest { market_id: 65538 });

        let result = toggle_favorite_market(
            authenticated_user,
            request,
            web::Data::new(mock_repo.clone() as Arc<dyn TradingRepository>),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 500);

                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let error_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(error_response["error"], "failed to toggle favorite market");
                assert!(error_response["message"].is_string());
                // Handlers return sanitized generic error message, not internal DB errors
                assert_eq!(
                    error_response["message"],
                    "An internal error occurred. Please try again later."
                );
            }
            Err(e) => {
                panic!(
                    "Handler should return Ok(HttpResponse) even on error, got: {}",
                    e
                );
            }
        }
    }
}
