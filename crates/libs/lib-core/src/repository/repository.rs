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
use std::collections::BTreeMap;
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
    async fn get_open_orders_paginated(&self, user_id: u64, offset: u64, limit: u64) -> Result<Vec<OrderModel>>;
    async fn count_open_orders(&self, user_id: u64) -> Result<u64>;
    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>>;
    async fn create_order(&self, order: OrderModel) -> Result<u64>;
    async fn count_user_orders(&self, user_id: u64) -> Result<u64>;
    async fn get_order_history(&self, user_id: u64) -> Result<Vec<OrderModel>>;
    async fn get_order_history_paginated(&self, user_id: u64, offset: u64, limit: u64) -> Result<Vec<OrderModel>>;
    async fn count_order_history(&self, user_id: u64) -> Result<u64>;
    
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
            .filter(UserBalanceColumn::UserId.eq(Decimal::from(user_id)))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>> {
        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(Decimal::from(user_id)))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let total = available + locked;

        // Insert new balance event (this is an event log table, not mutable state)
        let balance = UserBalanceActiveModel {
            user_id: Set(Decimal::from(user_id)),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(Decimal::from(available)),
            locked: Set(Decimal::from(locked)),
            total: Set(Decimal::from(total)),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn create_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let total = available + locked;

        let balance = UserBalanceActiveModel {
            user_id: Set(Decimal::from(user_id)),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(Decimal::from(available)),
            locked: Set(Decimal::from(locked)),
            total: Set(Decimal::from(total)),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn get_open_orders(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        let all_orders = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        // Filter to only include "open" orders: Placed, Pending, PartiallyFilled, Processing
        // Exclude: Filled, Cancelled, Rejected
        let orders: Vec<_> = all_orders
            .into_iter()
            .filter(|order| {
                let status_lower = order.status.to_lowercase();
                status_lower == "placed"
                    || status_lower == "pending" 
                    || status_lower == "partiallyfilled" 
                    || status_lower == "processing"
            })
            .collect();

        Ok(orders)
    }

    async fn get_open_orders_paginated(&self, user_id: u64, offset: u64, limit: u64) -> Result<Vec<OrderModel>> {
        let all_orders = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .order_by_desc(OrderColumn::OrderTimestamp)
            .offset(offset)
            .limit(limit)
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        // Filter to only include "open" orders: Placed, Pending, PartiallyFilled, Processing
        // Exclude: Filled, Cancelled, Rejected
        let orders: Vec<_> = all_orders
            .into_iter()
            .filter(|order| {
                let status_lower = order.status.to_lowercase();
                status_lower == "placed"
                    || status_lower == "pending" 
                    || status_lower == "partiallyfilled" 
                    || status_lower == "processing"
            })
            .collect();

        Ok(orders)
    }

    async fn count_open_orders(&self, user_id: u64) -> Result<u64> {
        let all_orders = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        // Count only "open" orders: Placed, Pending, PartiallyFilled, Processing
        let count = all_orders
            .into_iter()
            .filter(|order| {
                let status_lower = order.status.to_lowercase();
                status_lower == "placed"
                    || status_lower == "pending" 
                    || status_lower == "partiallyfilled" 
                    || status_lower == "processing"
            })
            .count();

        Ok(count as u64)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: u64) -> Result<Option<OrderModel>> {
        OrderEntity::find()
            .filter(
                OrderColumn::OrderOrderId
                    .eq(Decimal::from(order_id))
                    .and(OrderColumn::OrderUserId.eq(Decimal::from(user_id))),
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
        Ok(result.order_order_id.to_string().parse::<u64>().unwrap_or(0))
    }

    async fn count_user_orders(&self, user_id: u64) -> Result<u64> {
        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .count(&*self.db)
            .await?;
        Ok(count as u64)
    }
    
    async fn get_order_history(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        // Only return Cancelled, PartiallyFilled, and Filled orders for history
        OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .filter(
                OrderColumn::Status.eq("Cancelled")
                    .or(OrderColumn::Status.eq("PartiallyFilled"))
                    .or(OrderColumn::Status.eq("Filled"))
            )
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_order_history_paginated(&self, user_id: u64, offset: u64, limit: u64) -> Result<Vec<OrderModel>> {
        // Only return Cancelled, PartiallyFilled, and Filled orders for history
        OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .filter(
                OrderColumn::Status.eq("Cancelled")
                    .or(OrderColumn::Status.eq("PartiallyFilled"))
                    .or(OrderColumn::Status.eq("Filled"))
            )
            .order_by_desc(OrderColumn::OrderTimestamp)
            .offset(offset)
            .limit(limit)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn count_order_history(&self, user_id: u64) -> Result<u64> {
        // Count only Cancelled, PartiallyFilled, and Filled orders for history
        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(Decimal::from(user_id)))
            .filter(
                OrderColumn::Status.eq("Cancelled")
                    .or(OrderColumn::Status.eq("PartiallyFilled"))
                    .or(OrderColumn::Status.eq("Filled"))
            )
            .count(&*self.db)
            .await?;
        Ok(count as u64)
    }
    
    async fn get_user_favorite_markets(&self, user_id: u64) -> Result<Vec<UserFavoriteMarketModel>> {
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

    /// Fetches total portfolio equity at a point in time by summing across all assets.
    ///
    /// With the per-asset equity schema, we need to:
    /// 1. Get the latest equity value for each asset at or before ts_secs
    /// 2. Sum all asset equity values to get total portfolio equity
    ///
    /// Note: Currently unused but kept for ad-hoc queries and debugging.
    #[allow(dead_code)]
    async fn fetch_equity_at(&self, user_id: u64, ts_secs: i64) -> Result<Decimal> {
        // For each asset, get the latest equity value at or before the target timestamp,
        // then sum across all assets
        let sql = format!(
            "SELECT toString(sum(eq)) AS v
             FROM (
                 SELECT asset_id, argMax(equity_quote, minute) AS eq
                 FROM {} FINAL
                 WHERE user_id = ?
                   AND minute <= toDateTime(?)
                 GROUP BY asset_id
             )",
            self.table("equity_per_minute")
        );

        self.ch
            .query(sql.as_str())
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
            .transpose()
            .map(|opt| opt.unwrap_or(Decimal::ZERO))
    }

    /// Fetches per-asset equity components and performs forward-fill sum in Rust.
    ///
    /// This generates a mathematically correct total equity curve by:
    /// 1. Fetching all asset equity values bucketed by time
    /// 2. Forward-filling each asset's value across time
    /// 3. Summing all asset values at each bucket timestamp
    ///
    /// This approach fixes balance charts by ensuring BTC and ETH updates
    /// don't overwrite each other - the sum always reflects all assets.
    async fn fetch_total_equity_series(
        &self,
        user_id: u64,
        start_ts: i64,
        end_ts: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        let bucket_expr = resolution.bucket_expression();

        // Fetch raw components: timestamp, asset_id, equity value
        // argMax gets the last value within each bucket for each asset
        let sql = format!(
            "SELECT
                CAST(toUnixTimestamp(bucket) AS Int64) AS ts,
                asset_id,
                toString(argMax(equity_quote, minute)) AS equity_val
             FROM (
                SELECT {bucket} AS bucket, minute, asset_id, equity_quote
                FROM {table} FINAL
                WHERE user_id = ?
                  AND minute >= toDateTime(?)
                  AND minute <= toDateTime(?)
             )
             GROUP BY bucket, asset_id
             ORDER BY bucket ASC",
            bucket = bucket_expr,
            table = self.table("equity_per_minute")
        );

        let rows: Vec<(i64, u16, String)> = self
            .ch
            .query(sql.as_str())
            .bind(user_id)
            .bind(start_ts)
            .bind(end_ts)
            .fetch_all::<(i64, u16, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        // Organize data: Map<Timestamp, Vec<(AssetId, Value)>>
        let mut updates_by_time: BTreeMap<i64, Vec<(u16, Decimal)>> = BTreeMap::new();

        // Ensure we have data points for chart boundaries
        updates_by_time.entry(resolution.bucket_floor(start_ts)).or_default();
        updates_by_time.entry(resolution.bucket_floor(end_ts)).or_default();

        for (ts, asset_id, val_str) in rows {
            let val = Decimal::from_str(&val_str).unwrap_or(Decimal::ZERO);
            updates_by_time.entry(ts).or_default().push((asset_id, val));
        }

        // Fetch baseline state (latest value of every asset before start_ts)
        // This ensures the chart doesn't incorrectly start at 0
        let baseline_sql = format!(
            "SELECT asset_id, toString(argMax(equity_quote, minute)) AS eq
             FROM {} FINAL
             WHERE user_id = ?
               AND minute < toDateTime(?)
             GROUP BY asset_id",
            self.table("equity_per_minute")
        );

        let baseline_rows: Vec<(u16, String)> = self
            .ch
            .query(baseline_sql.as_str())
            .bind(user_id)
            .bind(start_ts)
            .fetch_all::<(u16, String)>()
            .await
            .unwrap_or_default();

        let mut current_portfolio_state: BTreeMap<u16, Decimal> = BTreeMap::new();
        for (asset, val_str) in baseline_rows {
            if let Ok(val) = Decimal::from_str(&val_str) {
                current_portfolio_state.insert(asset, val);
            }
        }

        // Forward-fill and sum: iterate through time buckets
        let mut series = Vec::with_capacity(updates_by_time.len());
        for (ts, updates) in updates_by_time {
            // Apply updates for this timestamp
            for (asset_id, value) in updates {
                current_portfolio_state.insert(asset_id, value);
            }

            // Sum the state of all assets at this moment
            let total_equity: Decimal = current_portfolio_state.values().copied().sum();
            series.push(BalancePoint {
                timestamp: ts,
                balance: total_equity,
            });
        }

        Ok(series)
    }

    /// Fetches cumulative cashflow series bucketed by resolution.
    ///
    /// Returns a map from bucket timestamp to cumulative cashflow up to that bucket.
    async fn fetch_cashflow_series(
        &self,
        user_id: u64,
        start_ts: i64,
        end_ts: i64,
        resolution: PnlResolution,
    ) -> Result<BTreeMap<i64, Decimal>> {
        let bucket_expr = resolution.bucket_expression();

        let sql = format!(
            "SELECT
                CAST(toUnixTimestamp({bucket}) AS Int64) AS ts,
                toString(sum(cashflow_quote)) AS cf
             FROM {table}
             WHERE user_id = ?
               AND minute > toDateTime(?)
               AND minute <= toDateTime(?)
             GROUP BY ts
             ORDER BY ts",
            bucket = bucket_expr,
            table = self.table("user_cashflow_per_minute")
        );

        let rows: Vec<(i64, String)> = self
            .ch
            .query(sql.as_str())
            .bind(user_id)
            .bind(start_ts)
            .bind(end_ts)
            .fetch_all::<(i64, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let mut map = BTreeMap::new();
        let mut cumulative = Decimal::ZERO;

        for (ts, cf_str) in rows {
            let val = Decimal::from_str(&cf_str).unwrap_or(Decimal::ZERO);
            cumulative += val;
            map.insert(ts, cumulative);
        }

        Ok(map)
    }

    /// Helper to get cumulative cashflow up to a specific bucket timestamp.
    fn get_cumulative_cashflow_at(
        cashflow_map: &BTreeMap<i64, Decimal>,
        target_ts: i64,
    ) -> Decimal {
        cashflow_map
            .range(..=target_ts)
            .next_back()
            .map(|(_, v)| *v)
            .unwrap_or(Decimal::ZERO)
    }
}

#[async_trait]
impl PortfolioRepository for ClickHousePortfolioRepository {
    /// ClickHouse implementation using per-asset equity with Rust-side aggregation.
    ///
    /// The new schema stores equity per asset (not aggregated), so we:
    /// 1. Fetch the total equity series with forward-fill sum for drawdown calculation
    /// 2. Extract scalar values (balance_now, balance_24h_ago, etc.) from the series
    /// 3. Compute max drawdown by iterating through the series
    /// 4. Build PnL series by adjusting equity for cashflow
    ///
    /// This approach ensures mathematically correct portfolio values even when
    /// different assets update at different times.
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

        // Fetch total equity series for the last 31 days (covers drawdown window)
        // Using Day resolution for scalar extraction (hourly buckets)
        let full_equity_series = self
            .fetch_total_equity_series(user_id, cutoff_1m, now_ts_secs, PnlResolution::Day)
            .await?;

        // Helper to extract value from series at a target timestamp
        let get_equity_at = |target_ts: i64| -> Decimal {
            full_equity_series
                .iter()
                .filter(|p| p.timestamp <= target_ts)
                .last()
                .map(|p| p.balance)
                .unwrap_or(Decimal::ZERO)
        };

        // Extract scalar values from the series
        let balance_now = get_equity_at(now_ts_secs);
        let balance_24h_ago = get_equity_at(cutoff_24h);
        let balance_7d_ago = get_equity_at(cutoff_7d);

        // Cashflow over last 7d (deposits - withdrawals)
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

        // Compute max_drawdown from the equity series (peak to trough)
        let mut peak = Decimal::ZERO;
        let mut max_drawdown = Decimal::ZERO;
        for point in full_equity_series.iter().filter(|p| p.timestamp >= cutoff_1m) {
            if point.balance > peak {
                peak = point.balance;
            }
            let drawdown = peak - point.balance;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        // Accounting PnL over 7d: Equity(now) - Equity(7d_ago) - Cashflow(7d)
        let pnl_7d = balance_now - balance_7d_ago - cashflow_7d;

        // Build PnL series for the requested resolution
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
    /// Fetch PnL time series for a user over a time window
    ///
    /// PnL = Equity(t) - Equity(baseline) - Cashflow(baseline..t)
    ///
    /// Uses the per-asset equity series (forward-fill summed in Rust) combined
    /// with cumulative cashflow to produce accurate accounting PnL.
    async fn fetch_pnl_series(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<PnlPoint>> {
        let window_start = now_ts_secs - resolution.window_seconds();

        // Get total equity series using per-asset forward-fill sum
        let equity_series = self
            .fetch_total_equity_series(user_id, window_start, now_ts_secs, resolution)
            .await?;

        // Get cumulative cashflow series
        let cashflow_map = self
            .fetch_cashflow_series(user_id, window_start, now_ts_secs, resolution)
            .await?;

        // Baseline equity at window start
        let baseline_equity = equity_series
            .first()
            .map(|p| p.balance)
            .unwrap_or(Decimal::ZERO);

        // Build PnL series: PnL = Equity(t) - Equity(baseline) - Cashflow(baseline..t)
        let mut series = Vec::with_capacity(equity_series.len());
        for point in equity_series {
            let cumulative_cashflow =
                Self::get_cumulative_cashflow_at(&cashflow_map, point.timestamp);
            let pnl = point.balance - baseline_equity - cumulative_cashflow;
            series.push(PnlPoint {
                timestamp: point.timestamp,
                pnl,
            });
        }

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
    /// The new schema stores equity PER ASSET in ClickHouse. This function:
    /// 1. Fetches all per-asset equity values bucketed by resolution
    /// 2. Performs forward-fill for each asset in Rust
    /// 3. Sums across all assets at each time bucket
    ///
    /// This approach ensures BTC and ETH updates don't overwrite each other,
    /// producing mathematically correct total portfolio equity.
    ///
    /// ## Correctness Guarantees
    ///
    /// ✓ **Multi-Asset Handling**: Each asset tracked separately, summed in Rust
    /// ✓ **Forward-Fill**: Missing buckets carry forward last known value per asset
    /// ✓ **Temporal Consistency**: Per-asset forward-fill prevents asset overwrites
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
        
        // Delegate to fetch_total_equity_series which handles per-asset
        // forward-fill and aggregation in Rust
        self.fetch_total_equity_series(user_id, window_start, now_ts_secs, resolution)
            .await
    }
}
