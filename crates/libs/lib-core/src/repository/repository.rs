use crate::model::orders::Model as OrderModel;
use crate::model::user_balances::Model as UserBalanceModel;
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
}

impl std::fmt::Display for TradingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingError::Database(msg) => write!(f, "Database error: {}", msg),
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

/// The gateway's reads of the read model's `trading` schema, which only Kafka Connect writes (each
/// one query), and its favourite markets.
#[async_trait]
pub trait TradingRepository: Send + Sync {
    /// The user's balance of every asset, from `trading.balances`, by asset.
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>>;

    // Orders, from `trading.orders`; a page holds the most recently changed first.
    async fn get_open_orders_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>>;
    async fn count_open_orders(&self, user_id: u64, market_id: Option<u32>) -> Result<u64>;
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

    /// The time between a chart's points, in seconds.
    fn step(self) -> usize {
        match self {
            PnlResolution::Day => 60 * 60,
            PnlResolution::Week => 6 * 60 * 60,
            PnlResolution::Month => 24 * 60 * 60,
            PnlResolution::Year => 7 * 24 * 60 * 60,
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

    /// `SELECT <select>` over the user's rows of `trading.orders` (one per order) that match
    /// `status_filter` and, if given, `market_id`; with `page`, `(offset, limit)`, that page of
    /// them, the most recently changed first. The index on `(order_user_id, event_ts)` serves it.
    fn orders_statement(
        select: &str,
        user_id: u64,
        status_filter: &str,
        market_id: Option<u32>,
        page: Option<(u64, u64)>,
    ) -> Statement {
        let mut values: Vec<Value> = vec![(user_id as i64).into()];
        let mut sql = format!(
            "SELECT {select} FROM trading.orders WHERE order_user_id = $1 AND ({status_filter})"
        );
        if let Some(market_id) = market_id {
            values.push(i64::from(market_id).into());
            sql.push_str(&format!(" AND market_id = ${}", values.len()));
        }
        if let Some((offset, limit)) = page {
            values.push((offset as i64).into());
            values.push((limit as i64).into());
            // order_order_id breaks ties: one command can change several of a user's orders.
            sql.push_str(&format!(
                " ORDER BY event_ts DESC, order_order_id DESC OFFSET ${} LIMIT ${}",
                values.len() - 1,
                values.len()
            ));
        }
        Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
    }

    async fn orders_page(
        &self,
        user_id: u64,
        status_filter: &str,
        market_id: Option<u32>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<OrderModel>> {
        OrderModel::find_by_statement(Self::orders_statement(
            ORDER_COLUMNS,
            user_id,
            status_filter,
            market_id,
            Some((offset, limit)),
        ))
        .all(&*self.db)
        .await
        .map_err(Into::into)
    }

    async fn count_orders(
        &self,
        user_id: u64,
        status_filter: &str,
        market_id: Option<u32>,
    ) -> Result<u64> {
        let row = self
            .db
            .query_one(Self::orders_statement(
                "COUNT(*) AS count",
                user_id,
                status_filter,
                market_id,
                None,
            ))
            .await?
            .ok_or_else(|| TradingError::Database("Order count returned no row".to_string()))?;
        let count: i64 = row.try_get("", "count")?;
        Ok(count as u64)
    }
}

/// The columns of [`OrderModel`].
const ORDER_COLUMNS: &str = "order_order_id, order_user_id, order_price, order_size, \
    order_filled_size, order_side, order_time_in_force, order_status, event_ts, market_id";

// Status and time-in-force values are vex-core's protobuf enum value names (trading.proto
// `Status`, `TimeInForce`), which the Kafka Connect sink writes verbatim.
const OPEN_ORDER_FILTER: &str = r#"
    order_status IN ('STATUS_PLACED', 'STATUS_PROCESSING')
    OR (
        order_status = 'STATUS_PARTIALLY_FILLED'
        AND order_time_in_force <> 'TIF_IOC'
    )
"#;

const HISTORY_ORDER_FILTER: &str =
    "order_status IN ('STATUS_CANCELLED', 'STATUS_PARTIALLY_FILLED', 'STATUS_FILLED')";

#[async_trait]
impl TradingRepository for PostgresTradingRepository {
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>> {
        UserBalanceModel::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT user_id, asset_id, available, locked, total, event_ts \
             FROM trading.balances WHERE user_id = $1 ORDER BY asset_id",
            [(user_id as i64).into()],
        ))
        .all(&*self.db)
        .await
        .map_err(Into::into)
    }

    async fn get_open_orders_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        self.orders_page(user_id, OPEN_ORDER_FILTER, market_id, offset, limit)
            .await
    }

    async fn count_open_orders(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        self.count_orders(user_id, OPEN_ORDER_FILTER, market_id)
            .await
    }

    async fn get_order_history_paginated(
        &self,
        user_id: u64,
        offset: u64,
        limit: u64,
        market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        self.orders_page(user_id, HISTORY_ORDER_FILTER, market_id, offset, limit)
            .await
    }

    async fn count_order_history(&self, user_id: u64, market_id: Option<u32>) -> Result<u64> {
        self.count_orders(user_id, HISTORY_ORDER_FILTER, market_id)
            .await
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
/// It reads only the read model's views (`readmodel/migrations/clickhouse`) in the database
/// `clickhouse.database` names. The views read their tables through `FINAL`, so that copies of an
/// event count once, and scale vex-core's integers to decimals as `ScalingCache` does:
/// - `balances_per_minute(user_id, asset_id, minute, total)`: each asset's last balance per minute;
/// - `asset_prices_per_minute(asset_id, quote_asset_id, minute, price)`: each market's last trade
///   price per minute, as its base asset's price in its quote asset;
/// - `deposits` and `withdrawals(user_id, asset_id, ts, amount)`;
/// - `trade_volumes(ts, maker_user_id, taker_user_id, volume)`: each fill's price times size.
///
/// A request reads its window only: the balances and prices in it, with each asset's last balance
/// and price before it, and the deposits and withdrawals in it. Equity at a time is the sum of
/// each asset's last balance at or before it at its last price at or before it: an asset that
/// has no price yet is worth zero. A deposit or withdrawal counts from its minute, as the
/// per-minute balance it changed does, at its asset's price then.
///
/// Metrics computed:
/// - balance (now): equity at `now`
/// - balance_24h_ago: equity at `now - 24h`
/// - pnl_7d: equity(now) - equity(now - 7d) - cashflow(7d)
/// - max_drawdown_1m: peak-to-trough decline over equity net of deposits and withdrawals, hourly
///   in the last 30 days
/// - volume_24h: sum of `trade_volumes.volume` where user is maker or taker in last 24h
///
/// Notes:
/// - Time parameters are passed as Unix seconds and wrapped with ClickHouse `toDateTime(?)`
/// - Amounts, prices and volumes are `Decimal(38, 18)`, read as text and parsed.
pub struct ClickHousePortfolioRepository {
    ch: ClickHouseClient,
    schema: String,
}

/// Values by Unix second.
type Series = BTreeMap<i64, Decimal>;

/// A user's balance of each asset, and each asset's price in the quote asset, by minute.
struct Holdings {
    balances: BTreeMap<u16, Series>,
    prices: BTreeMap<u16, Series>,
}

/// The last value at or before `ts`, else zero.
fn asof(series: &Series, ts: i64) -> Decimal {
    series
        .range(..=ts)
        .next_back()
        .map_or(Decimal::ZERO, |(_, value)| *value)
}

/// `asset`'s price in the quote asset at `ts`: zero before its first trade.
fn price_at(prices: &BTreeMap<u16, Series>, asset: u16, ts: i64) -> Decimal {
    if asset == ClickHousePortfolioRepository::QUOTE_ASSET_ID {
        return Decimal::ONE;
    }
    prices
        .get(&asset)
        .map_or(Decimal::ZERO, |history| asof(history, ts))
}

fn equity_at(holdings: &Holdings, ts: i64) -> Decimal {
    holdings
        .balances
        .iter()
        .map(|(asset, balances)| asof(balances, ts) * price_at(&holdings.prices, *asset, ts))
        .sum()
}

/// A chart's points: every `step` seconds from `start`, then `end`, since a window need not be a
/// multiple of its step (365 d / 7 d).
fn buckets(start: i64, end: i64, step: usize) -> impl Iterator<Item = i64> {
    (start..end).step_by(step).chain([end])
}

impl ClickHousePortfolioRepository {
    /// A repository reading the tables of the database `schema`, which every query names.
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

    /// Each asset's `column` in the rows of `view` whose `key` is `value`, by minute: the rows in
    /// (start, end], and the last one at or before `start`, which holds at `start`.
    async fn series(
        &self,
        view: &str,
        column: &str,
        key: &str,
        value: u64,
        start: i64,
        end: i64,
    ) -> Result<BTreeMap<u16, Series>> {
        let sql = format!(
            "SELECT asset_id, CAST(toUnixTimestamp(minute) AS Int64), toString({column})
             FROM {view}
             WHERE {key} = ? AND minute > toDateTime(?) AND minute <= toDateTime(?)
             UNION ALL
             SELECT asset_id, CAST(toUnixTimestamp(max(minute)) AS Int64),
                    toString(argMax({column}, minute))
             FROM {view}
             WHERE {key} = ? AND minute <= toDateTime(?)
             GROUP BY asset_id",
            view = self.table(view)
        );
        let rows: Vec<(u16, i64, String)> = self
            .ch
            .query(&sql)
            .bind(value)
            .bind(start)
            .bind(end)
            .bind(value)
            .bind(start)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;
        let mut series: BTreeMap<u16, Series> = BTreeMap::new();
        for (asset, ts, text) in rows {
            let value = Decimal::from_str(&text).unwrap_or(Decimal::ZERO);
            series.entry(asset).or_default().insert(ts, value);
        }
        Ok(series)
    }

    async fn holdings(&self, user_id: u64, start: i64, end: i64) -> Result<Holdings> {
        Ok(Holdings {
            balances: self
                .series(
                    "balances_per_minute",
                    "total",
                    "user_id",
                    user_id,
                    start,
                    end,
                )
                .await?,
            prices: self
                .series(
                    "asset_prices_per_minute",
                    "price",
                    "quote_asset_id",
                    Self::QUOTE_ASSET_ID.into(),
                    start,
                    end,
                )
                .await?,
        })
    }

    /// The value of the user's deposits less withdrawals in (start, end], cumulated by minute:
    /// each counts from its minute, at its asset's price in that minute.
    async fn cashflows(
        &self,
        user_id: u64,
        start: i64,
        end: i64,
        prices: &BTreeMap<u16, Series>,
    ) -> Result<Series> {
        // In time order: the running total below depends on it. ORDER BY sorts the whole UNION
        // ALL only from outside it.
        let flow_sql = format!(
            "SELECT t, asset_id, amt FROM (
                SELECT CAST(toUnixTimestamp(toStartOfMinute(ts)) AS Int64) AS t, asset_id,
                       toString(amount) AS amt
                FROM {}
                WHERE user_id = ? AND ts > toDateTime(?) AND ts <= toDateTime(?)
                UNION ALL
                SELECT CAST(toUnixTimestamp(toStartOfMinute(ts)) AS Int64) AS t, asset_id,
                       toString(-amount) AS amt
                FROM {}
                WHERE user_id = ? AND ts > toDateTime(?) AND ts <= toDateTime(?)
             )
             ORDER BY t ASC",
            self.table("deposits"),
            self.table("withdrawals")
        );
        let flow_rows: Vec<(i64, u16, String)> = self
            .ch
            .query(&flow_sql)
            .bind(user_id)
            .bind(start)
            .bind(end)
            .bind(user_id)
            .bind(start)
            .bind(end)
            .fetch_all()
            .await
            .map_err(|e| TradingError::Database(e.to_string()))?;

        let mut running_total = Decimal::ZERO;
        let mut cumulative = Series::new();
        for (ts, asset_id, amount) in flow_rows {
            let amount = Decimal::from_str(&amount).unwrap_or(Decimal::ZERO);
            running_total += amount * price_at(prices, asset_id, ts);
            cumulative.insert(ts, running_total);
        }
        Ok(cumulative)
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
        let window_start = now_ts_secs - resolution.window_seconds();

        // Every metric below is a difference of `net` values, so flows before `start` cancel out.
        let start = cutoff_1m.min(window_start);
        let holdings = self.holdings(user_id, start, now_ts_secs).await?;
        let cashflows = self
            .cashflows(user_id, start, now_ts_secs, &holdings.prices)
            .await?;
        // Equity less the money moved in since `start`: it changes only with prices and trades.
        let net = |ts: i64| equity_at(&holdings, ts) - asof(&cashflows, ts);

        let pnl_7d = net(now_ts_secs) - net(cutoff_7d);

        // Over `net`, so that a withdrawal is not a loss.
        let mut peak = Decimal::MIN;
        let mut max_drawdown = Decimal::ZERO;
        for ts in buckets(cutoff_1m, now_ts_secs, PnlResolution::Day.step()) {
            let value = net(ts);
            peak = peak.max(value);
            max_drawdown = max_drawdown.max(peak - value);
        }

        let volume_query = format!(
            "SELECT toString(sum(volume))
             FROM {} WHERE ts >= toDateTime(?) AND ts <= toDateTime(?)
               AND (maker_user_id = ? OR taker_user_id = ?)",
            self.table("trade_volumes")
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

        let baseline = net(window_start);
        let pnl_series = buckets(window_start, now_ts_secs, resolution.step())
            .map(|timestamp| PnlPoint {
                timestamp,
                pnl: net(timestamp) - baseline,
            })
            .collect();

        Ok(PortfolioMetrics {
            balance: equity_at(&holdings, now_ts_secs),
            balance_24h_ago: equity_at(&holdings, cutoff_24h),
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
        let holdings = self.holdings(user_id, window_start, now_ts_secs).await?;
        Ok(buckets(window_start, now_ts_secs, resolution.step())
            .map(|timestamp| BalancePoint {
                timestamp,
                balance: equity_at(&holdings, timestamp),
            })
            .collect())
    }
}
