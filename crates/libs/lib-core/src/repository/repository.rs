use crate::model::orders::{
    ActiveModel as OrderActiveModel, Column as OrderColumn, Entity as OrderEntity,
    Model as OrderModel,
};
use crate::model::user_balances::{
    ActiveModel as UserBalanceActiveModel, Column as UserBalanceColumn,
    Entity as UserBalanceEntity, Model as UserBalanceModel,
};
use async_trait::async_trait;
use clickhouse::Client as ClickHouseClient;
use rust_decimal::Decimal;
use sea_orm::prelude::*;
use sea_orm::*;
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
    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>>;
    async fn create_order(&self, order: OrderModel) -> Result<u64>;
    async fn count_user_orders(&self, user_id: u64) -> Result<u64>;
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
}

#[async_trait]
pub trait PortfolioRepository: Send + Sync {
    async fn get_portfolio(&self, user_id: u64, now_ts_secs: i64) -> Result<PortfolioMetrics>;
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

#[async_trait]
impl TradingRepository for PostgresTradingRepository {
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>> {
        let user_id_i64 = user_id as i64;

        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>> {
        let user_id_i64 = user_id as i64;

        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let user_id_i64 = user_id as i64;
        let total = available + locked;

        // Insert new balance event (this is an event log table, not mutable state)
        let balance = UserBalanceActiveModel {
            user_id: Set(user_id_i64),
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
        let user_id_i64 = user_id as i64;
        let total = available + locked;

        let balance = UserBalanceActiveModel {
            user_id: Set(user_id_i64),
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
        let user_id_i64 = user_id as i64;

        OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id_i64))
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>> {
        let user_id_i64 = user_id as i64;

        OrderEntity::find()
            .filter(
                OrderColumn::OrderOrderId
                    .eq(order_id as i64)
                    .and(OrderColumn::OrderUserId.eq(user_id_i64)),
            )
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn create_order(&self, order: OrderModel) -> Result<u64> {
        let active_model = OrderActiveModel {
            order_order_id: Set(order.order_order_id),
            order_user_id: Set(order.order_user_id),
            order_price: Set(order.order_price),
            order_size: Set(order.order_size),
            order_side: Set(order.order_side.to_string()),
            order_timestamp: Set(order.order_timestamp),
            market_id: Set(order.market_id),
            ..Default::default()
        };

        let result = active_model.insert(&*self.db).await?;
        Ok(result.order_order_id as u64)
    }

    async fn count_user_orders(&self, user_id: u64) -> Result<u64> {
        let user_id_i64 = user_id as i64;

        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id_i64))
            .count(&*self.db)
            .await?;
        Ok(count as u64)
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
}

impl ClickHousePortfolioRepository {
    pub fn new(ch: ClickHouseClient) -> Self {
        Self { ch }
    }

    async fn fetch_equity_at(&self, user_id: u64, ts_secs: i64) -> Result<Decimal> {
        Ok(
            self
                .ch
                .query(
                    "SELECT toString(toDecimal128(coalesce(equity_quote, 0), 12)) AS v\n                 FROM trading.equity_per_minute\n                 WHERE user_id = ?\n                   AND minute <= toDateTime(?)\n                 ORDER BY minute DESC\n                 LIMIT 1",
                )
                .bind(user_id)
                .bind(ts_secs)
                .fetch_optional::<String>()
                .await
                .map_err(|e| TradingError::Database(e.to_string()))?
                .map(|value| {
                    Decimal::from_str(&value).map_err(|err| {
                        TradingError::Database(format!("invalid decimal from ClickHouse: {}", err))
                    })
                })
                .transpose()?
                .unwrap_or(Decimal::ZERO),
        )
    }
}

#[async_trait]
impl PortfolioRepository for ClickHousePortfolioRepository {
    /// ClickHouse implementation using the schema-provided tables and queries.
    ///
    /// SQL (conceptual):
    /// - balance_now
    ///   SELECT toDecimal128(coalesce(equity_quote, 0), 12) FROM trading.equity_per_minute
    ///   WHERE user_id = {user} AND minute <= toDateTime({now})
    ///   ORDER BY minute DESC LIMIT 1;
    ///
    /// - balance_24h_ago
    ///   same with minute <= toDateTime({now - 24h})
    ///
    /// - balance_7d_ago
    ///   same with minute <= toDateTime({now - 7d})
    ///
    /// - cashflow_7d
    ///   SELECT toDecimal128(coalesce(sum(cashflow_quote), 0), 12)
    ///   FROM trading.user_cashflow_per_minute
    ///   WHERE user_id = {user} AND minute > toDateTime({now - 7d}) AND minute <= toDateTime({now})
    ///
    /// - volume_24h
    ///   SELECT toDecimal128(coalesce(sum(CAST(price * size_base AS Decimal(38, 12))), 0), 12)
    ///   FROM trading.trade_events
    ///   WHERE ts >= toDateTime({now - 24h}) AND ts <= toDateTime({now})
    ///     AND (maker_user_id = {user} OR taker_user_id = {user});
    ///
    /// - max_drawdown_1m
    ///   WITH s AS (
    ///     SELECT minute, toDecimal128(coalesce(equity_quote, 0), 12) AS v
    ///     FROM trading.equity_per_minute
    ///     WHERE user_id = {user}
    ///       AND minute >= toDateTime({now - 30d})
    ///       AND minute <= toDateTime({now})
    ///     ORDER BY minute
    ///   ),
    ///   r AS (
    ///     SELECT minute, v,\n                          runningMax(v) OVER (ORDER BY minute ROWS UNBOUNDED PRECEDING) AS peak\n                   FROM s\n                 )\n                 SELECT max(peak - v) AS max_drawdown FROM r",
    ///   )
    ///   SELECT max(peak - v) AS max_drawdown FROM r;
    async fn get_portfolio(&self, user_id: u64, now_ts_secs: i64) -> Result<PortfolioMetrics> {
        let cutoff_24h = now_ts_secs - 24 * 60 * 60;
        let cutoff_7d = now_ts_secs - 7 * 24 * 60 * 60;
        let cutoff_1m = now_ts_secs - 30 * 24 * 60 * 60;

        let parse_decimal = |value: String| {
            Decimal::from_str(&value).map_err(|e| {
                TradingError::Database(format!("invalid decimal from ClickHouse: {}", e))
            })
        };

        // balance_now
        let balance_now = self.fetch_equity_at(user_id, now_ts_secs).await?;

        // balance_24h_ago
        let balance_24h_ago = self.fetch_equity_at(user_id, cutoff_24h).await?;

        // balance_7d_ago
        let balance_7d_ago = self.fetch_equity_at(user_id, cutoff_7d).await?;

        // cashflow over last 7d (deposits - withdrawals), already in the same quote
        let cashflow_7d_str: String = self
            .ch
            .query(
                "SELECT toString(toDecimal128(coalesce(sum(cashflow_quote), 0), 12)) AS cf\n                 FROM trading.user_cashflow_per_minute\n                 WHERE user_id = ?\n                   AND minute > toDateTime(?)\n                   AND minute <= toDateTime(?)",
            )
            .bind(user_id)
            .bind(cutoff_7d)
            .bind(now_ts_secs)
            .fetch_one::<String>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;
        let cashflow_7d = parse_decimal(cashflow_7d_str)?;

        // volume_24h
        let volume_24h_str: String = self
            .ch
            .query(
                "SELECT\n                     toString(toDecimal128(coalesce(sum(CAST(price * size_base AS Decimal(38, 12))), 0), 12)) AS vol\n                 FROM trading.trade_events\n                 WHERE ts >= toDateTime(?)\n                   AND ts <= toDateTime(?)\n                   AND (maker_user_id = ? OR taker_user_id = ?)",
            )
            .bind(cutoff_24h)
            .bind(now_ts_secs)
            .bind(user_id)
            .bind(user_id)
            .fetch_one::<String>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;
        let volume_24h = parse_decimal(volume_24h_str)?;

        // max_drawdown_1m
        let max_drawdown = self
            .ch
            .query(
                "WITH s AS (\n                   SELECT minute, toDecimal128(coalesce(equity_quote, 0), 12) AS v\n                   FROM trading.equity_per_minute\n                   WHERE user_id = ?\n                     AND minute >= toDateTime(?)\n                     AND minute <= toDateTime(?)\n                   ORDER BY minute\n                 ),\n                 r AS (\n                   SELECT minute, v,\n                          runningMax(v) OVER (ORDER BY minute ROWS UNBOUNDED PRECEDING) AS peak\n                   FROM s\n                 )\n                 SELECT toString(toDecimal128(coalesce(max(peak - v), 0), 12)) AS max_drawdown FROM r",
            )
            .bind(user_id)
            .bind(cutoff_1m)
            .bind(now_ts_secs)
            .fetch_optional::<String>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?
            .map(parse_decimal)
            .transpose()?
            .unwrap_or(Decimal::ZERO);

        // accounting PnL over 7d
        // PnL = Equity(now) - Equity(7d_ago) - Cashflow(7d)
        let pnl_7d = balance_now - balance_7d_ago - cashflow_7d;

        Ok(PortfolioMetrics {
            balance: balance_now,
            balance_24h_ago,
            volume_24h,
            pnl_7d,
            max_drawdown_1m: max_drawdown,
        })
    }
}
