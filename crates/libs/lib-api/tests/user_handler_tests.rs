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
use std::path::Path;
use std::sync::Arc;
use tokio::sync::OnceCell;
use uuid::Uuid;

use async_trait::async_trait;
use lib_api::handlers::user::{
    OrdersQuery, get_balances, get_favorite_markets, get_open_orders, get_order_history,
    get_portfolio_overview, toggle_favorite_market,
};
use lib_core::{
    BalancePoint, PnlResolution, PortfolioMetrics, PortfolioRepository,
    ToggleFavoriteMarketRequest,
    model::{
        orders::Model as OrderModel, user_balances::Model as UserBalanceModel,
        user_favorite_markets::Model as UserFavoriteMarketModel,
    },
    repository::repository::{PostgresTradingRepository, TradingError, TradingRepository},
};
use lib_utils::{AuthenticatedUserId, uuid_to_u64};

fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://postgres:postgres123@localhost:5432/trading_db".to_string()
    })
}

/// Setup test database connection
async fn setup_test_db() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let connection_future = Database::connect(database_url());
    let timeout_duration = std::time::Duration::from_secs(5);

    match tokio::time::timeout(timeout_duration, connection_future).await {
        Ok(Ok(db)) => Ok(db),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(sea_orm::DbErr::Custom(
            "Database connection timeout".to_string(),
        )),
    }
}

/// Creates the tests' database afresh, `<DATABASE_URL's database>_user_handler_tests`, as a
/// deployment does: the read model's Postgres migrations, then its development seeds. Returns
/// its URL.
async fn create_schema_database(server: &DatabaseConnection) -> String {
    let mut url = url::Url::parse(&database_url()).expect("DATABASE_URL is a URL");
    let name = format!("{}_user_handler_tests", url.path().trim_start_matches('/'));
    for statement in [
        format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#),
        format!(r#"CREATE DATABASE "{name}""#),
    ] {
        server.execute_unprepared(&statement).await.unwrap();
    }
    url.set_path(&name);
    let db = Database::connect(url.as_str()).await.unwrap();

    let readmodel = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../readmodel");
    let mut scripts: Vec<_> = fs::read_dir(readmodel.join("migrations/postgres"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".up.sql"))
        .collect();
    scripts.sort();
    scripts.push(readmodel.join("seeds/development.sql"));
    for script in scripts {
        let sql = fs::read_to_string(&script).unwrap();
        if let Err(error) = db.execute_unprepared(&sql).await {
            panic!("{}: {error}", script.display());
        }
    }

    url.into()
}

/// A connection to the tests' database, which the first caller creates; None, so that the test
/// is skipped, when no Postgres server answers at DATABASE_URL.
async fn require_database() -> Option<DatabaseConnection> {
    static SCHEMA_DATABASE: OnceCell<String> = OnceCell::const_new();
    let server = setup_test_db().await.ok()?;
    let url = SCHEMA_DATABASE
        .get_or_init(|| create_schema_database(&server))
        .await;
    Some(Database::connect(url).await.unwrap())
}

/// Cleanup test data for a specific user
async fn cleanup_test_data(db: &DatabaseConnection, user_id: u64) {
    let user_id = user_id as i64;
    let cleanup_queries = vec![
        format!("DELETE FROM user_favorite_markets WHERE user_id = {user_id}"),
        format!("DELETE FROM trading.orders WHERE order_user_id = {user_id}"),
        format!("DELETE FROM trading.order_events WHERE order_user_id = {user_id}"),
        format!("DELETE FROM trading.balances WHERE user_id = {user_id}"),
        format!("DELETE FROM trading.balance_events WHERE user_id = {user_id}"),
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

/// Insert a user's latest balance of an asset, stamped now.
async fn insert_test_balance(
    db: &DatabaseConnection,
    user_id: u64,
    asset_id: i32,
    available: u64,
    locked: u64,
) -> Result<(), sea_orm::DbErr> {
    let total = available + locked;
    let timestamp = Utc::now().timestamp();
    insert_balance_row(
        db,
        "trading.balances",
        user_id,
        asset_id,
        available,
        locked,
        total,
        timestamp,
    )
    .await
}

/// Insert a user's balance of an asset, as Kafka Connect would: `table` is `trading.balances` or
/// its history, `trading.balance_events`. `u64` values are stored with the same bits.
#[allow(clippy::too_many_arguments)]
async fn insert_balance_row(
    db: &DatabaseConnection,
    table: &str,
    user_id: u64,
    asset_id: i32,
    available: u64,
    locked: u64,
    total: u64,
    event_ts: i64,
) -> Result<(), sea_orm::DbErr> {
    let sql = format!(
        "INSERT INTO {table} (user_id, asset_id, available, locked, total, event_ts) \
         VALUES ({}, {asset_id}, {}, {}, {}, {event_ts})",
        user_id as i64, available as i64, locked as i64, total as i64,
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
    insert_order_row(
        db,
        "trading.orders",
        OrderRow {
            order_id,
            user_id,
            market_id,
            price,
            size,
            filled: 0,
            side,
            time_in_force: "TIF_GTC",
            status,
            event_ts: Utc::now().timestamp(),
        },
    )
    .await
}

/// An order as Kafka Connect writes it to `trading.orders` or its history, `trading.order_events`.
/// Side, time in force and status are vex-core's enum names (SIDE_*, TIF_*, STATUS_*).
struct OrderRow<'a> {
    order_id: u64,
    user_id: u64,
    market_id: i32,
    price: u64,
    size: u64,
    filled: u64,
    side: &'a str,
    time_in_force: &'a str,
    status: &'a str,
    event_ts: i64,
}

/// Inserts `order` into `table`, with `u64` values stored with the same bits.
async fn insert_order_row(
    db: &DatabaseConnection,
    table: &str,
    order: OrderRow<'_>,
) -> Result<(), sea_orm::DbErr> {
    let sql = format!(
        "INSERT INTO {table} (order_order_id, order_user_id, order_price, order_size, \
         order_filled_size, order_side, order_time_in_force, order_status, event_ts, market_id) \
         VALUES ({}, {}, {}, {}, {}, '{}', '{}', '{}', {}, {})",
        order.order_id as i64,
        order.user_id as i64,
        order.price as i64,
        order.size as i64,
        order.filled as i64,
        order.side,
        order.time_in_force,
        order.status,
        order.event_ts,
        order.market_id,
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
    use lib_core::cache::{
        order_store::OrderStore, scaling_cache::ScalingCache, user_store::UserBalanceStore,
    };
    use std::sync::Mutex;
    use tracing::{
        Event, Metadata, Subscriber,
        field::{Field, Visit},
        span::{Attributes, Id, Record},
    };

    const DATABASE_DETAIL: &str = "SELECT secret FROM api_keys";

    /// A scaling cache that knows no market or asset: the stores built on it answer the stored
    /// integers, which the tests compare.
    fn unscaled() -> Arc<ScalingCache> {
        Arc::new(ScalingCache::new(Arc::new(DatabaseConnection::default())))
    }

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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

    /// A user above `i64::MAX`, whose id the `BIGINT` user columns hold with the same bits, as
    /// `uuid_to_u64` gives half of all users.
    fn user_above_i64_max() -> (Uuid, u64) {
        let user = Uuid::from_u128(u128::from(uuid_to_u64(create_test_user()) | 1 << 63));
        (user, uuid_to_u64(user))
    }

    fn order_ids(orders: &[OrderModel]) -> Vec<i64> {
        orders.iter().map(|order| order.order_order_id).collect()
    }

    #[tokio::test]
    async fn test_postgres_orders_come_from_the_latest_state_table() {
        let Some(db) = require_database().await else {
            return;
        };
        let (_, user_id) = user_above_i64_max();
        cleanup_test_data(&db, user_id).await;

        // trading.orders holds each order's latest state; its history must not leak in.
        for (order_id, market_id, status, event_ts) in [
            (91001, 65538, "STATUS_FILLED", 3),
            (91002, 65538, "STATUS_PLACED", 2),
            (91003, 65539, "STATUS_PARTIALLY_FILLED", 1),
            (91004, 65539, "STATUS_CANCELLED", 4),
        ] {
            let row = |status, event_ts| OrderRow {
                order_id,
                user_id,
                market_id,
                price: 5_000_000,
                size: 100,
                filled: 0,
                side: "SIDE_BID",
                time_in_force: "TIF_GTC",
                status,
                event_ts,
            };
            insert_order_row(&db, "trading.orders", row(status, event_ts))
                .await
                .unwrap();
            insert_order_row(&db, "trading.order_events", row("STATUS_PLACED", 0))
                .await
                .unwrap();
        }

        let repo = PostgresTradingRepository::new(Arc::new(db.clone()));
        let open = repo
            .get_open_orders_paginated(user_id, 0, 10, None)
            .await
            .unwrap();
        // The most recently changed first.
        assert_eq!(order_ids(&open), [91002, 91003]);
        assert!(
            open.iter()
                .all(|order| order.order_user_id as u64 == user_id)
        );
        assert_eq!(repo.count_open_orders(user_id, None).await.unwrap(), 2);
        let history = repo
            .get_order_history_paginated(user_id, 0, 10, None)
            .await
            .unwrap();
        assert_eq!(order_ids(&history), [91004, 91001, 91003]);
        assert_eq!(repo.count_order_history(user_id, None).await.unwrap(), 3);

        // A market and a page of one.
        let open_in_market = repo
            .get_open_orders_paginated(user_id, 0, 10, Some(65539))
            .await
            .unwrap();
        assert_eq!(order_ids(&open_in_market), [91003]);
        assert_eq!(
            repo.count_open_orders(user_id, Some(65539)).await.unwrap(),
            1
        );
        let history_in_market = repo
            .get_order_history_paginated(user_id, 0, 10, Some(65538))
            .await
            .unwrap();
        assert_eq!(order_ids(&history_in_market), [91001]);
        assert_eq!(
            repo.count_order_history(user_id, Some(65538))
                .await
                .unwrap(),
            1
        );
        let second = repo
            .get_order_history_paginated(user_id, 1, 1, None)
            .await
            .unwrap();
        assert_eq!(order_ids(&second), [91001]);

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_postgres_open_orders_exclude_partially_filled_ioc() {
        let Some(db) = require_database().await else {
            return;
        };
        let user_id = uuid_to_u64(create_test_user()) & i64::MAX as u64;
        cleanup_test_data(&db, user_id).await;

        // The engine cancels an IOC remainder, so a partially filled IOC order is not open.
        for (order_id, time_in_force) in [(92001, "TIF_IOC"), (92002, "TIF_GTC")] {
            let order = OrderRow {
                order_id,
                user_id,
                market_id: 65538,
                price: 5_000_000,
                size: 100,
                filled: 40,
                side: "SIDE_BID",
                time_in_force,
                status: "STATUS_PARTIALLY_FILLED",
                event_ts: Utc::now().timestamp(),
            };
            insert_order_row(&db, "trading.orders", order)
                .await
                .unwrap();
        }

        let repo = PostgresTradingRepository::new(Arc::new(db.clone()));
        let open_orders = repo
            .get_open_orders_paginated(user_id, 0, 10, None)
            .await
            .unwrap();
        assert_eq!(order_ids(&open_orders), [92002]);
        assert_eq!(repo.count_open_orders(user_id, None).await.unwrap(), 1);

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_postgres_balances_come_from_the_latest_state_table() {
        let Some(db) = require_database().await else {
            return;
        };
        let (_, user_id) = user_above_i64_max();
        cleanup_test_data(&db, user_id).await;

        // trading.balances holds each asset's latest balance; its history must not leak in.
        insert_balance_row(&db, "trading.balances", user_id, 2, 2_000, 0, 2_000, 20)
            .await
            .unwrap();
        insert_balance_row(&db, "trading.balances", user_id, 1, 700, 400, 1_100, 10)
            .await
            .unwrap();
        for (asset_id, available, event_ts) in [(1, 1_000, 5), (1, 900, 30), (3, 50, 1)] {
            insert_balance_row(
                &db,
                "trading.balance_events",
                user_id,
                asset_id,
                available,
                0,
                available,
                event_ts,
            )
            .await
            .unwrap();
        }

        let repo = PostgresTradingRepository::new(Arc::new(db.clone()));
        let balances = repo.get_user_balances(user_id).await.unwrap();

        let rows: Vec<_> = balances
            .iter()
            .map(|balance| {
                (
                    balance.user_id as u64,
                    balance.asset_id,
                    balance.available,
                    balance.locked,
                )
            })
            .collect();
        assert_eq!(rows, [(user_id, 1, 700, 400), (user_id, 2, 2_000, 0)]);

        cleanup_test_data(&db, user_id).await;
    }

    /// GET /balances, /orders/open and /orders/history, routed as the gateway routes them and
    /// with the scaling cache the gateway loads, answer from the read model's Postgres tables
    /// alone, with `u64` values above `i64::MAX` exact.
    #[tokio::test]
    async fn test_http_endpoints_serve_the_read_model() {
        use actix_web::{App, HttpMessage, dev::Service, test};
        use lib_auth::middleware::jwt_auth::AuthenticatedUser;
        use rust_decimal::Decimal;

        let Some(db) = require_database().await else {
            return;
        };
        let (user, user_id) = user_above_i64_max();
        cleanup_test_data(&db, user_id).await;

        // 10 ETH (asset 3, 10^18 units each: above i64::MAX) and 1 USD available, 0.5 locked.
        let ten_eth = 10_000_000_000_000_000_000_u64;
        insert_balance_row(&db, "trading.balances", user_id, 3, ten_eth, 0, ten_eth, 7)
            .await
            .unwrap();
        insert_balance_row(
            &db,
            "trading.balances",
            user_id,
            1,
            1_000_000,
            500_000,
            1_500_000,
            8,
        )
        .await
        .unwrap();
        // A bid for 2 ETH at 2,500 USD on ETH/USD (65539, quote_scale_k 10), 0.5 ETH filled, and
        // a filled one; both order ids above i64::MAX.
        let (open_id, filled_id) = ((1_u64 << 63) + 5, (1_u64 << 63) + 6);
        for (order_id, status, filled, event_ts) in [
            (
                open_id,
                "STATUS_PARTIALLY_FILLED",
                500_000_000_000_000_000,
                9,
            ),
            (filled_id, "STATUS_FILLED", 2_000_000_000_000_000_000, 10),
        ] {
            let order = OrderRow {
                order_id,
                user_id,
                market_id: 65539,
                price: 25_000,
                size: 2_000_000_000_000_000_000,
                filled,
                side: "SIDE_BID",
                time_in_force: "TIF_GTC",
                status,
                event_ts,
            };
            insert_order_row(&db, "trading.orders", order)
                .await
                .unwrap();
        }

        let db = Arc::new(db);
        let scaling_cache = Arc::new(ScalingCache::new(db.clone()));
        scaling_cache.load_all().await.unwrap();
        let trading_repo: Arc<dyn TradingRepository> =
            Arc::new(PostgresTradingRepository::new(db.clone()));
        let app = test::init_service(
            App::new()
                // What the auth middleware leaves for the handlers.
                .wrap_fn(move |request, service| {
                    request
                        .extensions_mut()
                        .insert(AuthenticatedUser { user_id: user });
                    service.call(request)
                })
                .app_data(web::Data::new(Arc::new(UserBalanceStore::new(
                    scaling_cache.clone(),
                ))))
                .app_data(web::Data::new(Arc::new(OrderStore::new(scaling_cache))))
                .app_data(web::Data::new(trading_repo))
                .configure(lib_api::routes::user_route::configure),
        )
        .await;
        let get = |uri: &'static str| {
            let app = &app;
            async move {
                let response =
                    test::call_service(app, test::TestRequest::get().uri(uri).to_request()).await;
                assert_eq!(response.status(), 200, "GET {uri}");
                test::read_body_json::<serde_json::Value, _>(response).await
            }
        };
        let decimal = |value: &serde_json::Value| -> Decimal {
            serde_json::from_value(value.clone()).unwrap()
        };

        let balances = get("/balances").await;
        assert_eq!(balances["total_assets"], 2);
        let (usd, eth) = (&balances["balances"][0], &balances["balances"][1]);
        assert_eq!(usd["user_id"], user_id.to_string());
        assert_eq!(usd["asset_id"], 1);
        assert_eq!(decimal(&usd["available"]), Decimal::ONE);
        assert_eq!(decimal(&usd["locked"]), Decimal::new(5, 1));
        assert_eq!(usd["timestamp"], 8);
        assert_eq!(eth["asset_id"], 3);
        assert_eq!(decimal(&eth["available"]), Decimal::TEN);
        assert_eq!(decimal(&eth["total"]), Decimal::TEN);

        let open = get("/orders/open").await;
        assert_eq!(open["total"], 1);
        let order = &open["orders"][0];
        assert_eq!(order["order_id"], open_id.to_string());
        assert_eq!(order["user_id"], user_id.to_string());
        assert_eq!(order["market_id"], 65539);
        assert_eq!(decimal(&order["price"]), Decimal::from(2_500));
        // What remains: 1.5 ETH.
        assert_eq!(decimal(&order["size"]), Decimal::new(15, 1));
        assert_eq!(order["status"], "PartiallyFilled");
        assert_eq!(order["timestamp"], 9);

        let history = get("/orders/history").await;
        assert_eq!(history["total"], 2);
        let ids: Vec<_> = history["orders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|order| order["order_id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, [filled_id.to_string(), open_id.to_string()]);
        assert_eq!(decimal(&history["orders"][0]["size"]), Decimal::TWO);
        assert_eq!(decimal(&history["orders"][1]["filled"]), Decimal::new(5, 1));

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
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
        let balance_store = Arc::new(UserBalanceStore::new(unscaled()));

        let authenticated_user = create_authenticated_user_id(user);

        // Call handler
        let result = get_balances(
            authenticated_user,
            web::Data::new(balance_store),
            web::Data::new(trading_repo as Arc<dyn TradingRepository>),
        )
        .await;

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
    async fn test_get_balances_empty() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = database_test_user(101_002);
        let user_id = uuid_to_u64(user);

        cleanup_test_data(&db, user_id).await;

        let trading_repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let balance_store = Arc::new(UserBalanceStore::new(unscaled()));

        let authenticated_user = create_authenticated_user_id(user);

        let result = get_balances(
            authenticated_user,
            web::Data::new(balance_store),
            web::Data::new(trading_repo as Arc<dyn TradingRepository>),
        )
        .await;

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let balance_store = Arc::new(UserBalanceStore::new(unscaled()));

        let result = get_balances(
            authenticated_user,
            web::Data::new(balance_store),
            web::Data::new(mock_repo as Arc<dyn TradingRepository>),
        )
        .await;

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
        let order_store = Arc::new(OrderStore::new(unscaled()));

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
