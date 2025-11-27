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

    /// Fetches the configured quote asset ID from ClickHouse.
    async fn fetch_quote_asset_id(&self) -> Result<u16> {
        let sql = "SELECT quote_asset_id FROM trading.equity_quote_config LIMIT 1";
        self.ch
            .query(sql)
            .fetch_optional::<u16>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?
            .ok_or_else(|| TradingError::Database("equity_quote_config not seeded".to_string()))
    }

    /// Fetches total portfolio equity series using DYNAMIC VALUATION.
    ///
    /// # Why Dynamic Valuation?
    ///
    /// The `equity_per_minute` materialized view has a fundamental flaw: it only
    /// triggers when balance events occur, NOT when prices change. This means:
    /// - User deposits Asset 2 → Balance recorded, but price is 0 (no trades yet)
    /// - User trades Asset 2 → Price updates to 1.0, but balance didn't change
    /// - Result: MV still has stale equity = balance * 0 = 0
    ///
    /// # Solution: Query-Time Valuation
    ///
    /// This method fetches raw balances and raw prices separately, then calculates
    /// equity dynamically in Rust. This ensures that price changes are immediately
    /// reflected in portfolio valuations, even for assets whose balance didn't change.
    ///
    /// # Algorithm
    ///
    /// 1. Fetch ALL balance history for user (asset_id, timestamp, total)
    /// 2. Fetch ALL price history (asset_id, timestamp, price)
    /// 3. Collect all timestamps where either balance or price changed
    /// 4. At each timestamp, forward-fill both balances and prices
    /// 5. Calculate total_equity = Σ(balance[asset] × price[asset])
    ///
    /// # Performance
    ///
    /// This approach fetches more data from ClickHouse but avoids the stale MV issue.
    /// For single-user dashboards, this is fast (milliseconds). The Rust iteration
    /// over thousands of points takes microseconds.
    async fn fetch_total_equity_series(
        &self,
        user_id: u64,
        start_ts: i64,
        end_ts: i64,
        _resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        // Get quote asset ID (e.g., USD = 1)
        let quote_asset_id = self.fetch_quote_asset_id().await?;

        // 1. Fetch ALL balance updates for this user up to end_ts
        // We need full history to reconstruct state at any point
        let balance_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64), toString(total)
             FROM {} FINAL
             WHERE user_id = ? AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("balances_last_per_min")
        );

        let balance_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(balance_sql.as_str())
            .bind(user_id)
            .bind(end_ts)
            .fetch_all::<(u16, i64, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        // 2. Fetch ALL price updates up to end_ts
        // Prices are global (not per-user), filtered by quote_asset_id
        let price_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64), toString(price)
             FROM {} FINAL
             WHERE quote_asset_id = ? AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("asset_prices_last_per_min")
        );

        let price_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(price_sql.as_str())
            .bind(quote_asset_id)
            .bind(end_ts)
            .fetch_all::<(u16, i64, String)>()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        // 3. Build history maps: asset_id -> (timestamp -> value)
        let mut balances: BTreeMap<u16, BTreeMap<i64, Decimal>> = BTreeMap::new();
        for (asset, ts, val) in balance_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            balances.entry(asset).or_default().insert(ts, v);
        }

        let mut prices: BTreeMap<u16, BTreeMap<i64, Decimal>> = BTreeMap::new();
        for (asset, ts, val) in price_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            prices.entry(asset).or_default().insert(ts, v);
        }

        // 4. Collect all timestamps where state changes occur
        let mut timestamps: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        timestamps.insert(start_ts);
        timestamps.insert(end_ts);
        for inner in balances.values() {
            timestamps.extend(inner.keys());
        }
        for inner in prices.values() {
            timestamps.extend(inner.keys());
        }

        // 5. Forward-fill and calculate equity at each timestamp
        let mut current_balances: BTreeMap<u16, Decimal> = BTreeMap::new();
        let mut current_prices: BTreeMap<u16, Decimal> = BTreeMap::new();

        // Quote asset always has price = 1.0 (it's the reference)
        current_prices.insert(quote_asset_id, Decimal::ONE);

        let mut series = Vec::new();
        for ts in timestamps {
            if ts > end_ts {
                break;
            }

            // Update balance state: forward-fill each asset's balance
            for (asset, history) in &balances {
                if let Some((_, val)) = history.range(..=ts).next_back() {
                    current_balances.insert(*asset, *val);
                }
            }

            // Update price state: forward-fill each asset's price
            for (asset, history) in &prices {
                if let Some((_, val)) = history.range(..=ts).next_back() {
                    current_prices.insert(*asset, *val);
                }
            }

            // Only include points within our requested window
            if ts >= start_ts {
                // Calculate total equity: Σ(balance × price) for all assets
                let mut total_equity = Decimal::ZERO;
                for (asset, balance) in &current_balances {
                    let price = if *asset == quote_asset_id {
                        Decimal::ONE // Quote asset is always 1:1
                    } else {
                        *current_prices.get(asset).unwrap_or(&Decimal::ZERO)
                    };
                    total_equity += *balance * price;
                }

                series.push(BalancePoint {
                    timestamp: ts,
                    balance: total_equity,
                });
            }
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
