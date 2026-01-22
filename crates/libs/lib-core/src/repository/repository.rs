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
            .filter(UserBalanceColumn::UserId.eq(user_id as i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
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
        let total = Decimal::from(available) + Decimal::from(locked);

        // Insert new balance event (this is an event log table, not mutable state)
        let balance = UserBalanceActiveModel {
            user_id: Set(user_id as i64),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(Decimal::from(available)),
            locked: Set(Decimal::from(locked)),
            total: Set(total),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn create_user_balance(&self, user_id: u64, available: u64, locked: u64) -> Result<()> {
        let total = Decimal::from(available) + Decimal::from(locked);

        let balance = UserBalanceActiveModel {
            user_id: Set(user_id as i64),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(Decimal::from(available)),
            locked: Set(Decimal::from(locked)),
            total: Set(total),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn get_open_orders(&self, user_id: u64) -> Result<Vec<OrderModel>> {
        // Filter for "open" orders: Placed, Pending, PartiallyFilled, Processing
        // Exclude: Filled, Cancelled, Rejected
        let open_status_condition = Condition::any()
            .add(OrderColumn::OrderStatus.eq("Placed"))
            .add(OrderColumn::OrderStatus.eq("Pending"))
            .add(OrderColumn::OrderStatus.eq("PartiallyFilled"))
            .add(OrderColumn::OrderStatus.eq("Processing"));

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
        // Filter for "open" orders: Placed, Pending, PartiallyFilled, Processing
        // Exclude: Filled, Cancelled, Rejected
        let open_status_condition = Condition::any()
            .add(OrderColumn::OrderStatus.eq("Placed"))
            .add(OrderColumn::OrderStatus.eq("Pending"))
            .add(OrderColumn::OrderStatus.eq("PartiallyFilled"))
            .add(OrderColumn::OrderStatus.eq("Processing"));

        let mut query = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .filter(open_status_condition);

        if let Some(market_id) = market_id {
            query = query.filter(OrderColumn::MarketId.eq(market_id as i32));
        }

        let orders = query
            .order_by_desc(OrderColumn::OrderTimestamp)
            .offset(offset)
            .limit(limit)
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        Ok(orders)
    }

    async fn count_open_orders(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        // Count only "open" orders: Placed, Pending, PartiallyFilled, Processing
        // Exclude: Filled, Cancelled, Rejected
        let open_status_condition = Condition::any()
            .add(OrderColumn::OrderStatus.eq("Placed"))
            .add(OrderColumn::OrderStatus.eq("Pending"))
            .add(OrderColumn::OrderStatus.eq("PartiallyFilled"))
            .add(OrderColumn::OrderStatus.eq("Processing"));

        let mut query = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .filter(open_status_condition);

        if let Some(market_id) = market_id {
            query = query.filter(OrderColumn::MarketId.eq(market_id as i32));
        }

        let count = query.count(&*self.db).await.map_err(TradingError::from)?;

        Ok(count)
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
                    .eq("Cancelled")
                    .or(OrderColumn::OrderStatus.eq("PartiallyFilled"))
                    .or(OrderColumn::OrderStatus.eq("Filled")),
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
        // Only return Cancelled, PartiallyFilled, and Filled orders for history
        let mut query = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .filter(
                OrderColumn::OrderStatus
                    .eq("Cancelled")
                    .or(OrderColumn::OrderStatus.eq("PartiallyFilled"))
                    .or(OrderColumn::OrderStatus.eq("Filled")),
            );

        if let Some(market_id) = market_id {
            query = query.filter(OrderColumn::MarketId.eq(market_id as i32));
        }

        let orders = query
            .order_by_desc(OrderColumn::OrderTimestamp)
            .offset(offset)
            .limit(limit)
            .all(&*self.db)
            .await
            .map_err(TradingError::from)?;

        Ok(orders)
    }

    async fn count_order_history(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        // Count only Cancelled, PartiallyFilled, and Filled orders for history
        let mut query = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id as i64))
            .filter(
                OrderColumn::OrderStatus
                    .eq("Cancelled")
                    .or(OrderColumn::OrderStatus.eq("PartiallyFilled"))
                    .or(OrderColumn::OrderStatus.eq("Filled")),
            );

        if let Some(market_id) = market_id {
            query = query.filter(OrderColumn::MarketId.eq(market_id as i32));
        }

        let count = query.count(&*self.db).await?;
        Ok(count)
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

        // Determine step size in seconds for the buckets
        let step_seconds: i64 = match resolution {
            PnlResolution::Day => 3600,    // 1 hour
            PnlResolution::Week => 21600,  // 6 hours
            PnlResolution::Month => 86400, // 1 day
            PnlResolution::Year => 604800, // 1 week
        };

        // 1. Fetch ALL balance updates for this user up to end_ts
        let balance_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64) as ts, toString(total)
             FROM {} FINAL
             WHERE user_id = ? AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("balances_last_per_min")
        );

        let balance_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(&balance_sql)
            .bind(user_id)
            .bind(end_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        // 2. Fetch ALL price updates up to end_ts
        let price_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64) as ts, toString(price)
             FROM {} FINAL
             WHERE quote_asset_id = ? AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("asset_prices_last_per_min")
        );

        let price_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(&price_sql)
            .bind(quote_asset_id)
            .bind(end_ts)
            .fetch_all()
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

        // 4. Generate time buckets and calculate equity at each
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

        // 2. Fetch Price History to value these flows
        // Include ALL prices (including genesis at timestamp 0) so cashflow valuation
        // matches equity valuation.
        let price_sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64), toString(price)
             FROM {} FINAL
             WHERE quote_asset_id = ? AND minute <= toDateTime(?)
             ORDER BY minute ASC",
            self.table("asset_prices_last_per_min")
        );

        let price_rows: Vec<(u16, i64, String)> = self
            .ch
            .query(&price_sql)
            .bind(quote_asset_id)
            .bind(end_ts)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        // 3. Build Price Map
        let mut prices: BTreeMap<u16, BTreeMap<i64, Decimal>> = BTreeMap::new();
        for (asset, ts, val) in price_rows {
            let v = Decimal::from_str(&val).unwrap_or(Decimal::ZERO);
            prices.entry(asset).or_default().insert(ts, v);
        }

        // 4. Calculate Cumulative Cost Basis
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
}

#[async_trait]
impl PortfolioRepository for ClickHousePortfolioRepository {
    async fn get_portfolio(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<PortfolioMetrics> {
        let cutoff_24h = now_ts_secs - 24 * 60 * 60;
        let cutoff_7d = now_ts_secs - 7 * 24 * 60 * 60;
        let cutoff_1m = now_ts_secs - 30 * 24 * 60 * 60;

        // 1. Fetch Equity Series
        // We fetch enough for drawdown calculation (30d)
        let full_series = self
            .fetch_total_equity_series(user_id, cutoff_1m, now_ts_secs, PnlResolution::Day)
            .await?;

        // Helper to get value from series
        let get_val = |ts: i64| -> Decimal {
            full_series
                .iter()
                .rev()
                .find(|p| p.timestamp <= ts)
                .map(|p| p.balance)
                .unwrap_or(Decimal::ZERO)
        };

        let balance_now = get_val(now_ts_secs);
        let balance_24h_ago = get_val(cutoff_24h);
        let balance_7d_ago = get_val(cutoff_7d);

        // 2. Calculate Cashflow (Dynamic)
        // Fetch ALL cashflows from beginning to get correct cumulative totals
        // PnL = (Equity_Now - Equity_Start) - Net_Cashflow
        let cashflow_map = self
            .fetch_cashflow_series(user_id, 0, now_ts_secs, resolution)
            .await?;

        // Get cumulative cashflow at 7d ago and now
        let cf_at_7d_ago = cashflow_map
            .range(..=cutoff_7d)
            .next_back()
            .map(|(_, v)| *v)
            .unwrap_or(Decimal::ZERO);
        let cf_at_now = cashflow_map
            .range(..=now_ts_secs)
            .next_back()
            .map(|(_, v)| *v)
            .unwrap_or(Decimal::ZERO);
        // Net cashflow in the 7d window
        let net_cashflow_7d = cf_at_now - cf_at_7d_ago;

        let pnl_7d = (balance_now - balance_7d_ago) - net_cashflow_7d;

        // 3. Calculate Drawdown
        let mut peak = Decimal::MIN;
        let mut max_drawdown = Decimal::ZERO;
        for point in &full_series {
            if point.timestamp < cutoff_1m {
                continue;
            }
            if point.balance > peak {
                peak = point.balance;
            }
            let dd = peak - point.balance;
            if dd > max_drawdown {
                max_drawdown = dd;
            }
        }

        // 4. Volume (Still simple SQL is fine)
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

        // 5. Build PnL Series for Chart
        // We need the specific resolution requested by the user (e.g. 1w or 1m)
        let window_start = now_ts_secs - resolution.window_seconds();
        let chart_series_equity = self
            .fetch_total_equity_series(user_id, window_start, now_ts_secs, resolution)
            .await?;

        // Fetch ALL cashflows from the beginning (timestamp 0) to now
        // This ensures we capture the full cumulative history to calculate correct baseline
        let chart_cashflows = self
            .fetch_cashflow_series(user_id, 0, now_ts_secs, resolution)
            .await?;

        // Baseline for chart is the start of the window
        let baseline_equity = chart_series_equity
            .first()
            .map(|x| x.balance)
            .unwrap_or(Decimal::ZERO);

        // Baseline cashflow is the cumulative cashflow AT or BEFORE window_start
        // This includes all historical deposits/withdrawals before the chart window
        let baseline_cf = chart_cashflows
            .range(..=window_start)
            .next_back()
            .map(|(_, v)| *v)
            .unwrap_or(Decimal::ZERO);

        let mut pnl_series = Vec::new();
        for point in chart_series_equity {
            let current_cf = chart_cashflows
                .range(..=point.timestamp)
                .next_back()
                .map(|(_, v)| *v)
                .unwrap_or(Decimal::ZERO);
            // PnL = (CurrentEquity - BaselineEquity) - (CurrentCashflow - BaselineCashflow)
            // This gives: change in equity minus new money added/removed
            let pnl = (point.balance - baseline_equity) - (current_cf - baseline_cf);
            pnl_series.push(PnlPoint {
                timestamp: point.timestamp,
                pnl,
            });
        }

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
        let window_start = now_ts_secs - resolution.window_seconds();
        self.fetch_total_equity_series(user_id, window_start, now_ts_secs, resolution)
            .await
    }
}
