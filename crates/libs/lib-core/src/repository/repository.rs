use crate::model::orders::{
    ActiveModel as OrderActiveModel, Column as OrderColumn, Entity as OrderEntity,
    Model as OrderModel,
};
use crate::model::user_balances::{
    ActiveModel as UserBalanceActiveModel, Column as UserBalanceColumn,
    Entity as UserBalanceEntity, Model as UserBalanceModel,
};
use crate::model::user_favorite_markets::{
    ActiveModel as UserFavoriteMarketActiveModel, Column as UserFavoriteMarketColumn,
    Entity as UserFavoriteMarketEntity, Model as UserFavoriteMarketModel,
};
use async_trait::async_trait;
use clickhouse::Client as ClickHouseClient;
use rust_decimal::Decimal;
use sea_orm::prelude::*;
use sea_orm::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

// Error type for trading operations
#[derive(Debug)]
pub enum TradingError {
    Database(String),
    NotFound,
}

impl std::fmt::Display for TradingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingError::Database(msg) => write!(f, "Database error: {}", msg),
            TradingError::NotFound => write!(f, "Not found"),
        }
    }
}

impl std::error::Error for TradingError {}

impl From<DbErr> for TradingError {
    fn from(err: DbErr) -> Self {
        TradingError::Database(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TradingError>;

#[async_trait]
pub trait TradingRepository: Send + Sync {
    // Balance operations
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>>;
    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>>;
    async fn update_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()>;
    async fn create_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()>;

    // Order operations
    async fn get_open_orders(&self, user_id: u64) -> Result<Vec<OrderModel>>;
    async fn get_open_orders_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>>;
    async fn count_open_orders(&self, user_id: u64, market_id: Option<u32>) -> Result<u64>;
    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>>;
    async fn create_order(&self, order: OrderModel) -> Result<u64>;
    async fn count_user_orders(&self, user_id: u64) -> Result<u64>;
    async fn get_order_history(&self, user_id: u64) -> Result<Vec<OrderModel>>;
    async fn get_order_history_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>>;
    async fn count_order_history(&self, user_id: u64, market_id: Option<u32>) -> Result<u64>;

    // Favorite markets operations
    async fn get_user_favorite_markets(&self, user_id: u64)
        -> Result<Vec<UserFavoriteMarketModel>>;
    async fn add_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()>;
    async fn remove_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()>;
    async fn is_market_favorited(&self, user_id: u64, market_id: u32) -> Result<bool>;
    /// Atomically toggle a market's favorite status.
    /// Returns Ok(true) if the market was added to favorites, Ok(false) if removed.
    async fn toggle_favorite_market(&self, user_id: u64, market_id: u32) -> Result<bool>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnlPoint {
    pub timestamp: i64,
    pub pnl: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancePoint {
    pub timestamp: i64,
    pub balance: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PnlResolution {
    #[default]
    Day,
    Week,
    Month,
    Year,
}

impl PnlResolution {
    fn window_seconds(self) -> i64 {
        match self {
            PnlResolution::Day => 24 * 60 * 60,
            PnlResolution::Week => 7 * 24 * 60 * 60,
            PnlResolution::Month => 30 * 24 * 60 * 60,
            PnlResolution::Year => 365 * 24 * 60 * 60,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PnlResolution::Day => "1d",
            PnlResolution::Week => "1w",
            PnlResolution::Month => "1m",
            PnlResolution::Year => "1y",
        }
    }

    pub fn from_query(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "1d" | "d1" | "day" => Some(PnlResolution::Day),
            "1w" | "w1" | "week" => Some(PnlResolution::Week),
            "1m" | "m1" | "month" => Some(PnlResolution::Month),
            "1y" | "y1" | "year" => Some(PnlResolution::Year),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PortfolioMetrics {
    /// Current equity in configured quote as Decimal(38,12)
    pub balance: Decimal,
    /// Equity 24h ago in configured quote as Decimal(38,12)
    pub balance_24h_ago: Decimal,
    /// Trade volume over the last 24h in configured quote as Decimal(38,12)
    pub volume_24h: Decimal,
    /// Accounting PnL over the last 7d in configured quote as Decimal(38,12)
    pub pnl_7d: Decimal,
    /// Max drawdown over the last 30d in configured quote as Decimal(38,12)
    pub max_drawdown_1m: Decimal,
    /// Time-series accounting PnL data for the requested window
    pub pnl_series: Vec<PnlPoint>,
    /// Resolution used to compute the PnL series
    pub pnl_resolution: PnlResolution,
}

#[async_trait]
pub trait PortfolioRepository: Send + Sync {
    async fn get_portfolio(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<PortfolioMetrics>;

    async fn get_portfolio_overview(
        &self,
        user_id: u64,
        now_ts_secs: i64,
    ) -> Result<PortfolioMetrics> {
        let mut metrics = self
            .get_portfolio(user_id, now_ts_secs, PnlResolution::Day)
            .await?;
        metrics.pnl_series.clear();
        metrics.pnl_resolution = PnlResolution::Day;
        Ok(metrics)
    }

    async fn get_pnl_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<PnlPoint>> {
        Ok(self
            .get_portfolio(user_id, now_ts_secs, resolution)
            .await?
            .pnl_series)
    }

    async fn get_balance_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>>;
}

// PostgreSQL implementation
pub struct PostgresTradingRepository {
    db: Arc<DatabaseConnection>,
}

impl PostgresTradingRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

// Scalar COUNT(*) result for the paginated count queries.
#[derive(Debug, FromQueryResult)]
struct CountRow {
    n: i64,
}

// SQL predicate for "open" orders against trading.orders_current. Matches the
// WHERE clause of the partial index idx_orders_current_user_open_ts so
// Postgres can serve both the page fetch AND the COUNT(*) as index-only scans.
const OPEN_PREDICATE_SQL: &str = r#"(
    order_status IN ('STATUS_PLACED','STATUS_PENDING','STATUS_PROCESSING')
    OR (
        order_status = 'STATUS_PARTIALLY_FILLED'
        AND order_time_in_force <> 'TIF_IOC'
    )
)"#;

// SQL predicate for "history" orders. Matches idx_orders_current_user_history_ts.
const HISTORY_PREDICATE_SQL: &str =
    "order_status IN ('STATUS_CANCELLED','STATUS_PARTIALLY_FILLED','STATUS_FILLED')";

#[async_trait]
impl TradingRepository for PostgresTradingRepository {
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>> {
        // trading.balances_events has UNIQUE(user_id, asset_id) and is fed by
        // the postgres-balances-sink in upsert mode (pk.fields=user_id,asset_id),
        // so it already holds one current-state row per (user, asset). A plain
        // filter + sort is enough; no DISTINCT ON.
        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id as i64))
            .order_by_asc(UserBalanceColumn::AssetId)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>> {
        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id as i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let total = available + locked;

        // Insert new balance event (this is an event log table, not mutable state)
        let balance = UserBalanceActiveModel {
            user_id: Set(user_id as i64),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(available as i64),
            locked: Set(locked as i64),
            total: Set(total as i64),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn create_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let total = available + locked;

        let balance = UserBalanceActiveModel {
            user_id: Set(user_id as i64),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(available as i64),
            locked: Set(locked as i64),
            total: Set(total as i64),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn get_open_orders(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        // Filter for "open" orders: Placed, Pending, PartiallyFilled (non-IOC), Processing
        // Exclude: Filled, Cancelled, Rejected, and PartiallyFilled IOC (no exception: IOC is immediate-or-cancel, so partially filled = done).
        let part_filled_not_ioc = Condition::all()
            .add(OrderColumn::OrderStatus.eq("STATUS_PARTIALLY_FILLED"))
            .add(OrderColumn::OrderTimeInForce.ne("TIF_IOC"));
        let open_status_condition = Condition::any()
            .add(OrderColumn::OrderStatus.eq("STATUS_PLACED"))
            .add(OrderColumn::OrderStatus.eq("STATUS_PENDING"))
            .add(part_filled_not_ioc)
            .add(OrderColumn::OrderStatus.eq("STATUS_PROCESSING"));

        let orders = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .filter(open_status_condition)
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        Ok(orders)
    }

    async fn get_open_orders_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        // Reads the current-state projection (trading.orders_current). WHERE
        // shape matches the partial index idx_orders_current_user_open_ts
        // (no market) or idx_orders_current_user_market_open_ts (with market)
        // so Postgres serves the page via a narrow B-tree range scan.
        //
        // Split by market_id presence so each shape is a stable prepared
        // statement tied to exactly one partial index — no NULL branch for
        // the planner to guess at.
        let stmt = match market_id {
            Some(mid) => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT
                        id, market_id, order_order_id, order_user_id,
                        order_price, order_size, order_filled_size,
                        order_side, order_time_in_force, order_status,
                        order_timestamp, created_at
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND market_id = $2
                      AND {predicate}
                    ORDER BY order_timestamp DESC
                    LIMIT $3 OFFSET $4
                    "#,
                    predicate = OPEN_PREDICATE_SQL,
                ),
                [
                    (user_id as i64).into(),
                    (mid as i32).into(),
                    (limit as i64).into(),
                    (offset as i64).into(),
                ],
            ),
            None => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT
                        id, market_id, order_order_id, order_user_id,
                        order_price, order_size, order_filled_size,
                        order_side, order_time_in_force, order_status,
                        order_timestamp, created_at
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND {predicate}
                    ORDER BY order_timestamp DESC
                    LIMIT $2 OFFSET $3
                    "#,
                    predicate = OPEN_PREDICATE_SQL,
                ),
                [
                    (user_id as i64).into(),
                    (limit as i64).into(),
                    (offset as i64).into(),
                ],
            ),
        };
        OrderEntity::find()
            .from_raw_sql(stmt)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn count_open_orders(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        // Plain COUNT(*). Index-only scan against
        // idx_orders_current_user_open_ts (or ..._market_open_ts when a market
        // filter is supplied). NOT called on the order-read hot path — the
        // handler derives has_next from a LIMIT+1 probe instead so each
        // request uses one connection, not two. Kept for out-of-band callers.
        let stmt = match market_id {
            Some(mid) => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT COUNT(*) AS n
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND market_id = $2
                      AND {predicate}
                    "#,
                    predicate = OPEN_PREDICATE_SQL,
                ),
                [(user_id as i64).into(), (mid as i32).into()],
            ),
            None => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT COUNT(*) AS n
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND {predicate}
                    "#,
                    predicate = OPEN_PREDICATE_SQL,
                ),
                [(user_id as i64).into()],
            ),
        };
        let row = CountRow::find_by_statement(stmt).one(&*self.db).await?;
        Ok(row.map(|r| r.n as u64).unwrap_or(0))
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>> {
        OrderEntity::find()
            .filter(
                OrderColumn::OrderOrderId
                    .eq(order_id as i64)
                    .and(OrderColumn::OrderUserId.eq(user_id as i64)),
            )
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn create_order(&self, order: OrderModel) -> Result<u64> {
        let active_model = OrderActiveModel {
            market_id: Set(order.market_id),
            order_order_id: Set(order.order_order_id),
            order_user_id: Set(order.order_user_id),
            order_price: Set(order.order_price),
            order_size: Set(order.order_size),
            order_filled_size: Set(order.order_filled_size),
            order_side: Set(order.order_side.to_string()),
            order_time_in_force: Set(order.order_time_in_force),
            order_status: Set(order.order_status),
            order_timestamp: Set(order.order_timestamp),
            ..Default::default()
        };

        let result = active_model.insert(&*self.db).await?;
        Ok(result.order_order_id as u64)
    }

    async fn count_user_orders(&self, user_id: u64) -> Result<u64> {
        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .count(&*self.db)
            .await?;
        Ok(count)
    }

    async fn get_order_history(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        // Only return Cancelled, PartiallyFilled, and Filled orders for history
        OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .filter(
                OrderColumn::OrderStatus
                    .eq("STATUS_CANCELLED")
                    .or(OrderColumn::OrderStatus.eq("STATUS_PARTIALLY_FILLED"))
                    .or(OrderColumn::OrderStatus.eq("STATUS_FILLED")),
            )
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_order_history_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        // Reads trading.orders_current. WHERE matches
        // idx_orders_current_user_history_ts (no market) or
        // idx_orders_current_user_market_history_ts (with market).
        // Split by market_id presence — one prepared statement per index.
        let stmt = match market_id {
            Some(mid) => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT
                        id, market_id, order_order_id, order_user_id,
                        order_price, order_size, order_filled_size,
                        order_side, order_time_in_force, order_status,
                        order_timestamp, created_at
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND market_id = $2
                      AND {predicate}
                    ORDER BY order_timestamp DESC
                    LIMIT $3 OFFSET $4
                    "#,
                    predicate = HISTORY_PREDICATE_SQL,
                ),
                [
                    (user_id as i64).into(),
                    (mid as i32).into(),
                    (limit as i64).into(),
                    (offset as i64).into(),
                ],
            ),
            None => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT
                        id, market_id, order_order_id, order_user_id,
                        order_price, order_size, order_filled_size,
                        order_side, order_time_in_force, order_status,
                        order_timestamp, created_at
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND {predicate}
                    ORDER BY order_timestamp DESC
                    LIMIT $2 OFFSET $3
                    "#,
                    predicate = HISTORY_PREDICATE_SQL,
                ),
                [
                    (user_id as i64).into(),
                    (limit as i64).into(),
                    (offset as i64).into(),
                ],
            ),
        };
        OrderEntity::find()
            .from_raw_sql(stmt)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn count_order_history(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        // Plain COUNT(*). NOT called on the hot path (handler uses LIMIT+1);
        // kept for out-of-band use.
        let stmt = match market_id {
            Some(mid) => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT COUNT(*) AS n
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND market_id = $2
                      AND {predicate}
                    "#,
                    predicate = HISTORY_PREDICATE_SQL,
                ),
                [(user_id as i64).into(), (mid as i32).into()],
            ),
            None => Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    r#"
                    SELECT COUNT(*) AS n
                    FROM trading.orders_current
                    WHERE order_user_id = $1
                      AND {predicate}
                    "#,
                    predicate = HISTORY_PREDICATE_SQL,
                ),
                [(user_id as i64).into()],
            ),
        };
        let row = CountRow::find_by_statement(stmt).one(&*self.db).await?;
        Ok(row.map(|r| r.n as u64).unwrap_or(0))
    }

    async fn get_user_favorite_markets(
        &self,
        user_id: u64,
    ) -> Result<Vec<UserFavoriteMarketModel>> {
        UserFavoriteMarketEntity::find()
            .filter(UserFavoriteMarketColumn::UserId.eq(user_id as i64))
            .order_by_desc(UserFavoriteMarketColumn::FavoritedAt)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn add_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()> {
        let market_id_i32 = market_id as i32;

        let favorite = UserFavoriteMarketActiveModel {
            user_id: Set(user_id as i64),
            market_id: Set(market_id_i32),
            favorited_at: Set(Some(chrono::Utc::now())),
            ..Default::default()
        };

        // Use insert with on_conflict to handle duplicate key constraint
        let result = UserFavoriteMarketEntity::insert(favorite)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    UserFavoriteMarketColumn::UserId,
                    UserFavoriteMarketColumn::MarketId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(&*self.db)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(DbErr::RecordNotInserted) => Ok(()), // Already exists, treat as success
            Err(e) => Err(e.into()),
        }
    }

    async fn remove_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()> {
        let market_id_i32 = market_id as i32;

        UserFavoriteMarketEntity::delete_many()
            .filter(
                UserFavoriteMarketColumn::UserId
                    .eq(user_id as i64)
                    .and(UserFavoriteMarketColumn::MarketId.eq(market_id_i32)),
            )
            .exec(&*self.db)
            .await?;

        Ok(())
    }

    async fn is_market_favorited(&self, user_id: u64, market_id: u32) -> Result<bool> {
        let market_id_i32 = market_id as i32;

        let count = UserFavoriteMarketEntity::find()
            .filter(
                UserFavoriteMarketColumn::UserId
                    .eq(user_id as i64)
                    .and(UserFavoriteMarketColumn::MarketId.eq(market_id_i32)),
            )
            .count(&*self.db)
            .await?;

        Ok(count > 0)
    }

    async fn toggle_favorite_market(&self, user_id: u64, market_id: u32) -> Result<bool> {
        use sea_orm::TransactionTrait;

        let market_id_i32 = market_id as i32;

        // Use a transaction to ensure atomicity
        let txn = self.db.begin().await?;

        // Check if already favorited within the transaction
        let existing = UserFavoriteMarketEntity::find()
            .filter(
                UserFavoriteMarketColumn::UserId
                    .eq(user_id as i64)
                    .and(UserFavoriteMarketColumn::MarketId.eq(market_id_i32)),
            )
            .one(&txn)
            .await?;

        let was_added = if existing.is_some() {
            // Remove the favorite
            UserFavoriteMarketEntity::delete_many()
                .filter(
                    UserFavoriteMarketColumn::UserId
                        .eq(user_id as i64)
                        .and(UserFavoriteMarketColumn::MarketId.eq(market_id_i32)),
                )
                .exec(&txn)
                .await?;
            false
        } else {
            // Add the favorite
            let favorite = UserFavoriteMarketActiveModel {
                user_id: Set(user_id as i64),
                market_id: Set(market_id_i32),
                favorited_at: Set(Some(chrono::Utc::now())),
                ..Default::default()
            };
            UserFavoriteMarketEntity::insert(favorite)
                .exec(&txn)
                .await?;
            true
        };

        txn.commit().await?;
        Ok(was_added)
    }
}

/// ClickHouse-backed Portfolio Repository
///
/// Data model (ClickHouse):
/// - `trading.equity_per_minute(user_id, minute, equity_quote)`
///   ReplacingMergeTree with last-per-minute equity valued in configured quote asset.
/// - `trading.user_cashflow_per_minute(user_id, minute, cashflow_quote)`
///   SummingMergeTree aggregating deposits (+) and withdrawals (-) per minute.
/// - `trading.trade_events(ts, price, size_base, maker_user_id, taker_user_id, ...)`
///   Raw trade feed for computing user volume.
///
/// Metrics computed:
/// - balance (now): last ASOF-valued equity at or before `now`
/// - balance_24h_ago: last ASOF-valued equity at or before `now - 24h`
/// - pnl_7d: equity(now) - equity(now - 7d) - cashflow(7d)
/// - max_drawdown_1m: peak-to-trough decline over ASOF equity in the last 30 days
/// - volume_24h: sum(price * size_base) where user is maker or taker in last 24h
///
/// Notes:
/// - Time parameters are passed as Unix seconds and wrapped with ClickHouse `toDateTime(?)`
/// - Equity, cashflow, and volume are stored as Decimal(38,12); we read them directly as textual
///   decimals from ClickHouse and parse without applying any manual scaling.
pub struct ClickHousePortfolioRepository {
    ch: ClickHouseClient,
    schema: String,
}

impl ClickHousePortfolioRepository {
    pub fn new(ch: ClickHouseClient) -> Self {
        Self::new_with_schema(ch, "trading")
    }

    pub fn new_with_schema<S: Into<String>>(ch: ClickHouseClient, schema: S) -> Self {
        Self {
            ch,
            schema: schema.into(),
        }
    }

    fn table(&self, name: &str) -> String {
        format!("{}.{}", self.schema, name)
    }

    /// Quote asset ID (USD = 1). This is a constant since the system
    /// always values portfolios in USD.
    const QUOTE_ASSET_ID: u16 = 1;

    fn step_seconds(resolution: PnlResolution) -> i64 {
        match resolution {
            PnlResolution::Day => 3600,    // 1 hour
            PnlResolution::Week => 21600,  // 6 hours
            PnlResolution::Month => 86400, // 1 day
            PnlResolution::Year => 604800, // 1 week
        }
    }

    async fn fetch_price_history_for_assets(
        &self,
        asset_ids: &BTreeSet<u16>,
        start_ts: i64,
        end_ts: i64,
    ) -> Result<BTreeMap<u16, BTreeMap<i64, Decimal>>> {
        if asset_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let asset_filter = asset_ids
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");

        let seed_price_sql = format!(
            "SELECT asset_id, toString(argMax(price, minute))
             FROM {} FINAL
             WHERE quote_asset_id = ? AND asset_id IN ({}) AND minute <= toDateTime(?)
             GROUP BY asset_id",
            self.table("asset_prices_last_per_min"),
            asset_filter
        );

        let seed_price_rows: Vec<(u16, String)> = self
            .ch
            .query(&seed_price_sql)
            .bind(Self::QUOTE_ASSET_ID)
            .bind(start_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let delta_price_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64), toString(price)
             FROM {} FINAL
             WHERE quote_asset_id = ? AND asset_id IN ({})
               AND minute > toDateTime(?) AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("asset_prices_last_per_min"),
            asset_filter
        );

        let delta_price_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(&delta_price_sql)
            .bind(Self::QUOTE_ASSET_ID)
            .bind(start_ts)
            .bind(end_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let mut prices: BTreeMap<u16, BTreeMap<i64, Decimal>> = BTreeMap::new();

        for (asset, val) in seed_price_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            prices.entry(asset).or_default().insert(start_ts, v);
        }

        for (asset, ts, val) in delta_price_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            prices.entry(asset).or_default().insert(ts, v);
        }

        Ok(prices)
    }

    /// Fetch Equity Series using Rust-side forward-fill aggregation.
    ///
    /// This approach fetches raw balance and price data, then calculates equity
    /// in Rust with proper forward-fill semantics. More reliable than complex
    /// SQL ASOF JOINs which have edge cases with empty data.
    async fn fetch_total_equity_series(
        &self,
        user_id: u64,
        start_ts: i64,
        end_ts: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        let quote_asset_id = Self::QUOTE_ASSET_ID;
        let step_seconds = Self::step_seconds(resolution);

        // Seed the window with the last known balance per asset at or before
        // start_ts, then fetch only updates inside the requested window.
        let seed_balance_sql = format!(
            "SELECT asset_id, toString(argMax(total, minute))
             FROM {} FINAL
             WHERE user_id = ? AND minute <= toDateTime(?)
             GROUP BY asset_id",
            self.table("balances_last_per_min")
        );

        let seed_balance_rows: Vec<(u16, String)> = self
            .ch
            .query(&seed_balance_sql)
            .bind(user_id)
            .bind(start_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let delta_balance_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64), toString(total)
             FROM {} FINAL
             WHERE user_id = ? AND minute > toDateTime(?) AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("balances_last_per_min")
        );

        let delta_balance_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(&delta_balance_sql)
            .bind(user_id)
            .bind(start_ts)
            .bind(end_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let mut balances: BTreeMap<u16, BTreeMap<i64, Decimal>> = BTreeMap::new();

        for (asset, val) in seed_balance_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            balances.entry(asset).or_default().insert(start_ts, v);
        }

        for (asset, ts, val) in delta_balance_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            balances.entry(asset).or_default().insert(ts, v);
        }

        let tracked_assets = balances
            .keys()
            .copied()
            .filter(|asset_id| *asset_id != quote_asset_id)
            .collect::<BTreeSet<_>>();

        let prices = self
            .fetch_price_history_for_assets(&tracked_assets, start_ts, end_ts)
            .await?;

        // Generate time buckets and calculate equity at each.
        let mut series = Vec::new();
        let mut current_ts = start_ts;

        while current_ts <= end_ts {
            // Forward-fill balances and prices at this timestamp
            let mut total_equity = Decimal::ZERO;

            for (asset_id, balance_history) in &balances {
                // Get balance at or before current_ts
                let balance = balance_history
                    .range(..=current_ts)
                    .next_back()
                    .map(|(_, v)| *v)
                    .unwrap_or(Decimal::ZERO);

                // Get price at or before current_ts
                let price = if *asset_id == quote_asset_id {
                    Decimal::ONE // Quote asset always worth 1.0
                } else {
                    prices
                        .get(asset_id)
                        .and_then(|h| h.range(..=current_ts).next_back())
                        .map(|(_, v)| *v)
                        .unwrap_or(Decimal::ZERO)
                };

                total_equity += balance * price;
            }

            series.push(BalancePoint {
                timestamp: current_ts,
                balance: total_equity,
            });

            current_ts += step_seconds;
        }

        Ok(series)
    }

    /// Dynamic Cashflow Calculation
    ///
    /// Fetches raw Deposit/Withdrawal events and values them using:
    /// 1. The price AT THE TIME of the deposit/withdrawal (historical cost basis)
    /// 2. If no real price exists (only genesis $0), falls back to LATEST price
    ///
    /// This ensures deposits made before any trading activity are valued
    /// consistently with equity (both use the same latest price).
    async fn fetch_cashflow_series(
        &self,
        user_id: u64,
        start_ts: i64,
        end_ts: i64,
        _resolution: PnlResolution,
    ) -> Result<BTreeMap<i64, Decimal>> {
        let quote_asset_id = Self::QUOTE_ASSET_ID;

        // 1. Fetch Raw Flows (Deposits +, Withdrawals -)
        let flow_sql = format!(
            "SELECT
                CAST(toUnixTimestamp(ts) as Int64) as t,
                asset_id, 
                toString(amount) as amt
             FROM {} 
             WHERE user_id = ? AND ts > toDateTime(?) AND ts <= toDateTime(?)
             UNION ALL
             SELECT 
                CAST(toUnixTimestamp(ts) as Int64) as t,
                asset_id, 
                toString(amount * -1) as amt
             FROM {} 
             WHERE user_id = ? AND ts > toDateTime(?) AND ts <= toDateTime(?)
             ORDER BY t ASC",
            self.table("deposit_events"),
            self.table("withdrawal_events")
        );

        let flow_rows: Vec<(i64, u16, String)> = self
            .ch
            .query(&flow_sql)
            .bind(user_id)
            .bind(start_ts)
            .bind(end_ts)
            .bind(user_id)
            .bind(start_ts)
            .bind(end_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let tracked_assets = flow_rows
            .iter()
            .map(|(_, asset_id, _)| *asset_id)
            .filter(|asset_id| *asset_id != quote_asset_id)
            .collect::<BTreeSet<_>>();

        let prices = self
            .fetch_price_history_for_assets(&tracked_assets, start_ts, end_ts)
            .await?;

        // 3. Calculate cumulative net cashflow inside the requested window.
        let mut cumulative_map = BTreeMap::new();
        let mut running_total = Decimal::ZERO;

        for (ts, asset_id, amt_str) in flow_rows {
            let amount = Decimal::from_str(&amt_str).unwrap_or(Decimal::ZERO);

            // Value the flow using historical cost basis with genesis fallback
            let price = if asset_id == quote_asset_id {
                Decimal::ONE // USD always worth 1.0
            } else {
                // Try to find price AT or BEFORE the deposit time (historical cost)
                let historical_price = prices
                    .get(&asset_id)
                    .and_then(|h| h.range(..=ts).next_back())
                    .map(|(_, p)| *p);

                match historical_price {
                    Some(p) if p != Decimal::ZERO => p, // Real price exists, use it
                    _ => {
                        // No real price at deposit time (genesis case)
                        // Fall back to LATEST available price to match equity valuation
                        prices
                            .get(&asset_id)
                            .and_then(|h| h.iter().next_back())
                            .map(|(_, p)| *p)
                            .unwrap_or(Decimal::ZERO)
                    }
                }
            };

            let flow_value = amount * price;
            running_total += flow_value;

            cumulative_map.insert(ts, running_total);
        }

        Ok(cumulative_map)
    }

    fn series_value_at_or_before(series: &[BalancePoint], ts: i64) -> Decimal {
        series
            .iter()
            .rev()
            .find(|point| point.timestamp <= ts)
            .map(|point| point.balance)
            .unwrap_or(Decimal::ZERO)
    }

    fn compute_max_drawdown(series: &[BalancePoint], cutoff_ts: i64) -> Decimal {
        let mut peak = Decimal::MIN;
        let mut max_drawdown = Decimal::ZERO;

        for point in series {
            if point.timestamp < cutoff_ts {
                continue;
            }
            if point.balance > peak {
                peak = point.balance;
            }
            let drawdown = peak - point.balance;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        max_drawdown
    }

    fn build_pnl_series(
        equity_series: Vec<BalancePoint>,
        cashflow_map: &BTreeMap<i64, Decimal>,
    ) -> Vec<PnlPoint> {
        let baseline_equity = equity_series
            .first()
            .map(|point| point.balance)
            .unwrap_or(Decimal::ZERO);

        equity_series
            .into_iter()
            .map(|point| {
                let current_cf = cashflow_map
                    .range(..=point.timestamp)
                    .next_back()
                    .map(|(_, value)| *value)
                    .unwrap_or(Decimal::ZERO);

                PnlPoint {
                    timestamp: point.timestamp,
                    pnl: (point.balance - baseline_equity) - current_cf,
                }
            })
            .collect()
    }
}

#[async_trait]
impl PortfolioRepository for ClickHousePortfolioRepository {
    async fn get_portfolio_overview(
        &self,
        user_id: u64,
        now_ts_secs: i64,
    ) -> Result<PortfolioMetrics> {
        let cutoff_24h = now_ts_secs - 24 * 60 * 60;
        let cutoff_7d = now_ts_secs - 7 * 24 * 60 * 60;
        let cutoff_1m = now_ts_secs - 30 * 24 * 60 * 60;

        let equity_series = self
            .fetch_total_equity_series(user_id, cutoff_1m, now_ts_secs, PnlResolution::Day)
            .await?;

        let balance = Self::series_value_at_or_before(&equity_series, now_ts_secs);
        let balance_24h_ago = Self::series_value_at_or_before(&equity_series, cutoff_24h);
        let balance_7d_ago = Self::series_value_at_or_before(&equity_series, cutoff_7d);

        let cashflow_map = self
            .fetch_cashflow_series(user_id, cutoff_7d, now_ts_secs, PnlResolution::Day)
            .await?;
        let net_cashflow_7d = cashflow_map
            .range(..=now_ts_secs)
            .next_back()
            .map(|(_, value)| *value)
            .unwrap_or(Decimal::ZERO);
        let pnl_7d = (balance - balance_7d_ago) - net_cashflow_7d;

        let max_drawdown_1m = Self::compute_max_drawdown(&equity_series, cutoff_1m);

        let volume_query = format!(
            "SELECT toString(toDecimal128(coalesce(sum(CAST(price * size_base AS Decimal(38, 12))), 0), 12)) 
             FROM {} WHERE ts >= toDateTime(?) AND ts <= toDateTime(?) 
               AND (maker_user_id = ? OR taker_user_id = ?)",
            self.table("trade_events")
        );
        let vol_str: String = self
            .ch
            .query(&volume_query)
            .bind(cutoff_24h)
            .bind(now_ts_secs)
            .bind(user_id)
            .bind(user_id)
            .fetch_one()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;
        let volume_24h = Decimal::from_str(&vol_str).unwrap_or(Decimal::ZERO);

        Ok(PortfolioMetrics {
            balance,
            balance_24h_ago,
            volume_24h,
            pnl_7d,
            max_drawdown_1m,
            pnl_series: Vec::new(),
            pnl_resolution: PnlResolution::Day,
        })
    }

    async fn get_pnl_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<PnlPoint>> {
        let window_start = now_ts_secs - resolution.window_seconds();
        let equity_series = self
            .fetch_total_equity_series(user_id, window_start, now_ts_secs, resolution)
            .await?;
        let cashflow_map = self
            .fetch_cashflow_series(user_id, window_start, now_ts_secs, resolution)
            .await?;

        Ok(Self::build_pnl_series(equity_series, &cashflow_map))
    }

    async fn get_portfolio(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<PortfolioMetrics> {
        let mut metrics = self.get_portfolio_overview(user_id, now_ts_secs).await?;
        metrics.pnl_series = self.get_pnl_chart(user_id, now_ts_secs, resolution).await?;
        metrics.pnl_resolution = resolution;

        Ok(metrics)
    }

    async fn get_balance_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        let window_start = now_ts_secs - resolution.window_seconds();
        self.fetch_total_equity_series(user_id, window_start, now_ts_secs, resolution)
            .await
    }
}
