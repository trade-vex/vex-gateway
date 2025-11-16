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
use serde::{Deserialize, Serialize};
use sea_orm::prelude::*;
use sea_orm::*;
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
    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>>;
    async fn create_order(&self, order: OrderModel) -> Result<u64>;
    async fn count_user_orders(&self, user_id: u64) -> Result<u64>;
    
    // Favorite markets operations
    async fn get_user_favorite_markets(&self, user_id: u64) -> Result<Vec<UserFavoriteMarketModel>>;
    async fn add_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()>;
    async fn remove_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()>;
    async fn is_market_favorited(&self, user_id: u64, market_id: u32) -> Result<bool>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PnlResolution {
    Day,
    Week,
    Month,
    Year,
}

impl Default for PnlResolution {
    fn default() -> Self {
        PnlResolution::Day
    }
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

    fn bucket_expression(self) -> &'static str {
        match self {
            PnlResolution::Day => "toStartOfHour(minute)",
            PnlResolution::Week => "toStartOfDay(minute)",
            PnlResolution::Month => "toStartOfDay(minute)",
            PnlResolution::Year => "toStartOfMonth(minute)",
        }
    }

    fn bucket_floor(self, ts_secs: i64) -> i64 {
        use chrono::{Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};

        let Some(dt) = chrono::DateTime::<Utc>::from_timestamp(ts_secs, 0) else {
            return ts_secs;
        };
        let date = dt.date_naive();

        let to_ts = |date: NaiveDate| -> i64 {
            date.and_hms_opt(0, 0, 0)
                .map(|naive_dt| Utc.from_utc_datetime(&naive_dt).timestamp())
                .unwrap_or(ts_secs)
        };

        match self {
            PnlResolution::Day => dt
                .with_minute(0)
                .and_then(|d| d.with_second(0))
                .and_then(|d| d.with_nanosecond(0))
                .unwrap_or(dt)
                .timestamp(),
            PnlResolution::Week => {
                let weekday = date.weekday().num_days_from_monday() as i64;
                let start_date = date - Duration::days(weekday);
                to_ts(start_date)
            }
            PnlResolution::Month => {
                let floored =
                    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date);
                to_ts(floored)
            }
            PnlResolution::Year => {
                let floored = NaiveDate::from_ymd_opt(date.year(), 1, 1).unwrap_or(date);
                to_ts(floored)
            }
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

#[async_trait]
impl TradingRepository for PostgresTradingRepository {
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>> {
        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>> {
        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let total = available + locked;

        // Insert new balance event (this is an event log table, not mutable state)
        let balance = UserBalanceActiveModel {
            user_id: Set(user_id),
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
            user_id: Set(user_id),
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
        let all_orders = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id))
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        // Filter to only include "open" orders: Pending, PartiallyFilled, Processing
        // Exclude: Filled, Cancelled, Rejected, Placed
        let orders: Vec<_> = all_orders
            .into_iter()
            .filter(|order| {
                let status_lower = order.status.to_lowercase();
                status_lower == "pending" 
                    || status_lower == "partiallyfilled" 
                    || status_lower == "processing"
            })
            .collect();

        Ok(orders)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>> {
        OrderEntity::find()
            .filter(
                OrderColumn::OrderOrderId
                    .eq(order_id)
                    .and(OrderColumn::OrderUserId.eq(user_id)),
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
            time_in_force: Set(order.time_in_force),
            status: Set(order.status),
            ..Default::default()
        };

        let result = active_model.insert(&*self.db).await?;
        Ok(result.order_order_id)
    }

    async fn count_user_orders(&self, user_id: u64) -> Result<u64> {
        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id))
            .count(&*self.db)
            .await?;
        Ok(count as u64)
    }
    
    async fn get_user_favorite_markets(&self, user_id: u64) -> Result<Vec<UserFavoriteMarketModel>> {
        UserFavoriteMarketEntity::find()
            .filter(UserFavoriteMarketColumn::UserId.eq(user_id))
            .order_by_desc(UserFavoriteMarketColumn::FavoritedAt)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }
    
    async fn add_favorite_market(&self, user_id: u64, market_id: u32) -> Result<()> {
        let market_id_i32 = market_id as i32;

        let favorite = UserFavoriteMarketActiveModel {
            user_id: Set(user_id),
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
                    .eq(user_id)
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
                    .eq(user_id)
                    .and(UserFavoriteMarketColumn::MarketId.eq(market_id_i32)),
            )
            .count(&*self.db)
            .await?;

        Ok(count > 0)
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

    async fn fetch_equity_at(&self, user_id: u64, ts_secs: i64) -> Result<Decimal> {
        Ok(
            self
                .ch
                .query(
                    format!(
                        "SELECT toString(toDecimal128(coalesce(equity_quote, 0), 12)) AS v
                 FROM {}
                 WHERE user_id = ?
                   AND minute <= toDateTime(?)
                 ORDER BY minute DESC
                 LIMIT 1",
                        self.table("equity_per_minute")
                    )
                    .as_str(),
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
    /// - max_drawdown_1m: running maximum drawdown over equity snapshots between now-30d and now
    async fn get_portfolio(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<PortfolioMetrics> {
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
        let cashflow_7d_query = format!(
            "SELECT toString(toDecimal128(coalesce(sum(cashflow_quote), 0), 12)) AS cf
                 FROM {}
                 WHERE user_id = ?
                   AND minute > toDateTime(?)
                   AND minute <= toDateTime(?)",
            self.table("user_cashflow_per_minute")
        );
        let cashflow_7d_str: String = self
            .ch
            .query(cashflow_7d_query.as_str())
            .bind(user_id)
            .bind(cutoff_7d)
            .bind(now_ts_secs)
            .fetch_one::<String>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;
        let cashflow_7d = parse_decimal(cashflow_7d_str)?;

        // volume_24h
        let volume_query = format!(
            "SELECT
                     toString(toDecimal128(coalesce(sum(CAST(price * size_base AS Decimal(38, 12))), 0), 12)) AS vol
                 FROM {}
                 WHERE ts >= toDateTime(?)
                   AND ts <= toDateTime(?)
                   AND (maker_user_id = ? OR taker_user_id = ?)",
            self.table("trade_events")
        );
        let volume_24h_str: String = self
            .ch
            .query(volume_query.as_str())
            .bind(cutoff_24h)
            .bind(now_ts_secs)
            .bind(user_id)
            .bind(user_id)
            .fetch_one::<String>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;
        let volume_24h = parse_decimal(volume_24h_str)?;

        // max_drawdown_1m
        let max_drawdown_query = format!(
            "WITH s AS (
                   SELECT minute, toDecimal128(coalesce(equity_quote, 0), 12) AS v
                   FROM {equity}
                   WHERE user_id = ?
                     AND minute >= toDateTime(?)
                     AND minute <= toDateTime(?)
                   ORDER BY minute
                 ),
                 r AS (
                   SELECT
                       minute,
                       v,
                       max(v) OVER (ORDER BY minute ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS peak
                   FROM s
                 )
                 SELECT toString(toDecimal128(coalesce(max(peak - v), 0), 12)) AS max_drawdown FROM r",
            equity = self.table("equity_per_minute")
        );
        let max_drawdown = self
            .ch
            .query(max_drawdown_query.as_str())
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

        let pnl_series = self
            .fetch_pnl_series(user_id, now_ts_secs, resolution)
            .await?;

        Ok(PortfolioMetrics {
            balance: balance_now,
            balance_24h_ago,
            volume_24h,
            pnl_7d,
            max_drawdown_1m: max_drawdown,
            pnl_series,
            pnl_resolution: resolution,
        })
    }
    
    async fn get_balance_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        self.fetch_balance_series(user_id, now_ts_secs, resolution)
            .await
    }
}

impl ClickHousePortfolioRepository {
    async fn fetch_pnl_series(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<PnlPoint>> {
        let window_start = now_ts_secs - resolution.window_seconds();
        let bucket_expr = resolution.bucket_expression();

        let equity_sql = format!(
            "SELECT CAST(toUnixTimestamp(bucket) AS Int64) AS ts, toString(anyLast(equity_quote)) AS eq \
             FROM ( \
                SELECT {bucket} AS bucket, minute, equity_quote \
                FROM {equity_table} \
                WHERE user_id = ? \
                  AND minute >= toDateTime(?) \
                  AND minute <= toDateTime(?) \
             ) \
             GROUP BY bucket \
             ORDER BY bucket",
            bucket = bucket_expr,
            equity_table = self.table("equity_per_minute")
        );

        let equity_rows: Vec<(i64, String)> = self
            .ch
            .query(equity_sql.as_str())
            .bind(user_id)
            .bind(window_start)
            .bind(now_ts_secs)
            .fetch_all::<(i64, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let cashflow_sql = format!(
            "SELECT CAST(toUnixTimestamp(bucket) AS Int64) AS ts, toString(sum(cashflow_quote)) AS cf \
             FROM ( \
                SELECT {bucket} AS bucket, cashflow_quote \
                FROM {cashflow_table} \
                WHERE user_id = ? \
                  AND minute > toDateTime(?) \
                  AND minute <= toDateTime(?) \
             ) \
             GROUP BY bucket \
             ORDER BY bucket",
            bucket = bucket_expr,
            cashflow_table = self.table("user_cashflow_per_minute")
        );

        let cashflow_rows: Vec<(i64, String)> = self
            .ch
            .query(cashflow_sql.as_str())
            .bind(user_id)
            .bind(window_start)
            .bind(now_ts_secs)
            .fetch_all::<(i64, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let mut equity_map = BTreeMap::new();
        for (ts, eq_str) in equity_rows {
            let value = Decimal::from_str(&eq_str).map_err(|err| {
                TradingError::Database(format!("invalid decimal from ClickHouse: {}", err))
            })?;
            equity_map.insert(ts, value);
        }

        let mut cashflow_map = BTreeMap::new();
        for (ts, cf_str) in cashflow_rows {
            let value = Decimal::from_str(&cf_str).map_err(|err| {
                TradingError::Database(format!("invalid decimal from ClickHouse: {}", err))
            })?;
            cashflow_map.insert(ts, value);
        }

        let mut cumulative_cashflow = Decimal::ZERO;
        let mut cashflow_cumulative_map = BTreeMap::new();
        for (ts, value) in &cashflow_map {
            cumulative_cashflow += *value;
            cashflow_cumulative_map.insert(*ts, cumulative_cashflow);
        }

        let baseline_equity = self.fetch_equity_at(user_id, window_start).await?;

        let mut bucket_keys: BTreeSet<i64> = BTreeSet::new();
        bucket_keys.extend(equity_map.keys().copied());
        bucket_keys.extend(cashflow_map.keys().copied());
        bucket_keys.insert(resolution.bucket_floor(window_start));
        bucket_keys.insert(resolution.bucket_floor(now_ts_secs));
        bucket_keys.insert(window_start);
        bucket_keys.insert(now_ts_secs);

        let mut last_equity = baseline_equity;
        let mut series = Vec::with_capacity(bucket_keys.len());

        for bucket_ts in bucket_keys {
            if let Some(value) = equity_map.get(&bucket_ts) {
                last_equity = *value;
            }

            let cf_up_to_bucket = cashflow_cumulative_map
                .range(..=bucket_ts)
                .next_back()
                .map(|(_, v)| *v)
                .unwrap_or(Decimal::ZERO);

            let pnl = last_equity - baseline_equity - cf_up_to_bucket;
            series.push(PnlPoint {
                timestamp: bucket_ts,
                pnl,
            });
        }

        series.sort_by_key(|point| point.timestamp);
        Ok(series)
    }
    
    /// Fetch balance time series for a user over a time window
    ///
    /// # Balance Computation Logic
    ///
    /// ## What is "Balance" in this context?
    /// Balance represents the user's **total portfolio equity** valued in the configured
    /// quote asset (e.g., USDT). This is the sum of ALL assets (BTC, ETH, USDT, etc.)
    /// converted to a common quote currency.
    ///
    /// ## How Multi-Asset Balances Are Computed
    ///
    /// The balance series leverages ClickHouse materialized views (see clickhouse.sql):
    ///
    /// ### Step 1: Per-Asset Balances (trading.balances_last_per_min)
    /// ```sql
    /// -- For each (user_id, asset_id, minute), stores the last known balance in native units
    /// -- Example: User 123 has 1.5 BTC (native units: 150000000 satoshis) at 2024-01-01 10:00
    /// ```
    ///
    /// ### Step 2: Asset Price Conversion (trading.asset_prices_last_per_min)
    /// ```sql
    /// -- For each (asset_id, quote_asset_id, minute), stores the conversion price
    /// -- Example: BTC->USDT price = 45000.00 at 2024-01-01 10:00
    /// --          ETH->USDT price = 2500.00 at 2024-01-01 10:00
    /// ```
    ///
    /// ### Step 3: Equity Aggregation (trading.equity_per_minute)
    /// The materialized view `mv_equity_per_minute` performs the following for each minute:
    /// ```sql
    /// SELECT
    ///     minute,
    ///     user_id,
    ///     sum(balance_in_native_units * price_to_quote) AS equity_quote
    /// FROM balances_last_per_min b
    /// LEFT JOIN asset_prices_last_per_min ap ON ...
    /// GROUP BY minute, user_id
    /// ```
    ///
    /// **Concrete Example:**
    /// ```
    /// User 123 at 2024-01-01 10:00:
    /// - BTC balance: 1.5 BTC (150000000 satoshis) * $45,000/BTC = $67,500
    /// - ETH balance: 10 ETH (10000000000000000000 wei) * $2,500/ETH = $25,000
    /// - USDT balance: 7500 USDT (7500000000 micro-USDT) * $1/USDT = $7,500
    /// -------------------------------------------------------------------
    /// Total Equity in USDT = $100,000
    /// ```
    ///
    /// ## What This Function Does
    ///
    /// This function queries the **pre-aggregated** `equity_per_minute` table and:
    /// 1. Groups equity values into time buckets based on resolution (hour, day, month, year)
    /// 2. Takes the last equity value within each bucket (using `anyLast`)
    /// 3. Forward-fills missing buckets with the last known equity value
    /// 4. Returns a time series of total portfolio balance over the window
    ///
    /// ## Query Breakdown
    ///
    /// ### Inner Query: Fetch Per-Minute Equity
    /// ```sql
    /// SELECT {bucket_expr} AS bucket, minute, equity_quote
    /// FROM trading.equity_per_minute
    /// WHERE user_id = ?
    ///   AND minute >= toDateTime(window_start)
    ///   AND minute <= toDateTime(now)
    /// ```
    /// - Fetches all per-minute equity snapshots for the user in the time window
    /// - Each row represents total portfolio value (all assets) at that minute
    ///
    /// ### Outer Query: Aggregate into Time Buckets
    /// ```sql
    /// SELECT
    ///     CAST(toUnixTimestamp(bucket) AS Int64) AS ts,
    ///     toString(anyLast(equity_quote)) AS eq
    /// FROM (inner_query)
    /// GROUP BY bucket
    /// ORDER BY bucket
    /// ```
    /// - `bucket_expr` maps minutes to buckets:
    ///   - 1D resolution: `toStartOfHour(minute)` → hourly buckets
    ///   - 1W/1M resolution: `toStartOfDay(minute)` → daily buckets
    ///   - 1Y resolution: `toStartOfMonth(minute)` → monthly buckets
    /// - `anyLast(equity_quote)`: picks the last equity value within each bucket
    /// - Returns timestamp + equity pairs for charting
    ///
    /// ## Forward-Fill Logic
    ///
    /// After fetching data from ClickHouse, we fill gaps in the series:
    /// ```rust
    /// let mut last_equity = baseline_equity;  // equity at window start
    /// for bucket_ts in all_bucket_timestamps {
    ///     if equity_map.contains(bucket_ts) {
    ///         last_equity = equity_map[bucket_ts];  // update from DB
    ///     }
    ///     series.push(BalancePoint { timestamp: bucket_ts, balance: last_equity });
    /// }
    /// ```
    /// This ensures:
    /// - Every expected bucket has a balance value (no gaps in chart)
    /// - Periods with no trades/events show constant balance (last known value)
    ///
    /// ## Correctness Guarantees
    ///
    /// ✓ **Multi-Asset Handling**: All assets are converted to quote via materialized views
    /// ✓ **Price Accuracy**: Uses ASOF joins to get best available price for each asset/minute
    /// ✓ **Temporal Consistency**: Per-minute granularity ensures accurate historical balances
    /// ✓ **Decimal Precision**: Decimal(38,12) for equity prevents rounding errors
    ///
    /// # Parameters
    /// - `user_id`: User whose balance chart to fetch
    /// - `now_ts_secs`: Current time as Unix timestamp (seconds)
    /// - `resolution`: Time resolution (Day=hourly buckets, Week/Month=daily, Year=monthly)
    ///
    /// # Returns
    /// Sorted vector of (timestamp, balance) points representing total portfolio equity over time
    async fn fetch_balance_series(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        let window_start = now_ts_secs - resolution.window_seconds();
        let bucket_expr = resolution.bucket_expression();

        let equity_sql = format!(
            "SELECT CAST(toUnixTimestamp(bucket) AS Int64) AS ts, toString(anyLast(equity_quote)) AS eq \
             FROM ( \
                SELECT {bucket} AS bucket, minute, equity_quote \
                FROM {equity_table} \
                WHERE user_id = ? \
                  AND minute >= toDateTime(?) \
                  AND minute <= toDateTime(?) \
             ) \
             GROUP BY bucket \
             ORDER BY bucket",
            bucket = bucket_expr,
            equity_table = self.table("equity_per_minute")
        );

        let equity_rows: Vec<(i64, String)> = self
            .ch
            .query(equity_sql.as_str())
            .bind(user_id)
            .bind(window_start)
            .bind(now_ts_secs)
            .fetch_all::<(i64, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        // Parse equity values from ClickHouse strings into Decimal
        // BTreeMap provides sorted keys for efficient iteration
        let mut equity_map = BTreeMap::new();
        for (ts, eq_str) in equity_rows {
            let value = Decimal::from_str(&eq_str).map_err(|err| {
                TradingError::Database(format!("invalid decimal from ClickHouse: {}", err))
            })?;
            equity_map.insert(ts, value);
        }

        // Fetch baseline equity at window start for forward-fill initialization
        // This is the user's total portfolio value at the beginning of the chart window
        let baseline_equity = self.fetch_equity_at(user_id, window_start).await?;

        // Build complete set of bucket timestamps to ensure no gaps in the chart
        // This includes:
        // 1. All buckets returned from ClickHouse (equity_map keys)
        // 2. Floored window boundaries (start/end of chart aligned to bucket boundaries)
        // 3. Exact window_start and now_ts_secs for precise endpoints
        let mut bucket_keys: BTreeSet<i64> = BTreeSet::new();
        bucket_keys.extend(equity_map.keys().copied());
        bucket_keys.insert(resolution.bucket_floor(window_start));
        bucket_keys.insert(resolution.bucket_floor(now_ts_secs));
        bucket_keys.insert(window_start);
        bucket_keys.insert(now_ts_secs);

        // Forward-fill algorithm:
        // - Start with baseline_equity at window start
        // - For each bucket timestamp (sorted):
        //   - If we have data from DB for this bucket, use it and update last_equity
        //   - Otherwise, use last_equity (carry forward)
        // Result: Continuous balance series with no gaps
        let mut last_equity = baseline_equity;
        let mut series = Vec::with_capacity(bucket_keys.len());

        for bucket_ts in bucket_keys {
            if let Some(value) = equity_map.get(&bucket_ts) {
                last_equity = *value;
            }

            series.push(BalancePoint {
                timestamp: bucket_ts,
                balance: last_equity,
            });
        }

        series.sort_by_key(|point| point.timestamp);
        Ok(series)
    }
}
