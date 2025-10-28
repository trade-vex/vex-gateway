use crate::model::orders::{
    ActiveModel as OrderActiveModel, Column as OrderColumn, Entity as OrderEntity,
    Model as OrderModel,
};
use crate::model::user_balances::{
    ActiveModel as UserBalanceActiveModel, Column as UserBalanceColumn,
    Entity as UserBalanceEntity, Model as UserBalanceModel,
};
use crate::model::trades::{Entity as TradeEntity, Column as TradeColumn};
use crate::model::deposits::{Entity as DepositEntity, Column as DepositColumn};
use crate::model::withdrawals::{Entity as WithdrawalEntity, Column as WithdrawalColumn};
use async_trait::async_trait;
use sea_orm::prelude::*;
use sea_orm::*;
use std::sync::Arc;
use uuid::Uuid;

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
    async fn get_user_balances(&self, user_id: Uuid) -> Result<Vec<UserBalanceModel>>;
    async fn get_user_balance(&self, user_id: Uuid) -> Result<Option<UserBalanceModel>>;
    async fn update_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()>;
    async fn create_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()>;

    // Order operations
    async fn get_open_orders(&self, user_id: Uuid) -> Result<Vec<OrderModel>>;
    async fn get_order_by_id(&self, order_id: u64, user_id: Uuid) -> Result<Option<OrderModel>>;
    async fn create_order(&self, order: OrderModel) -> Result<u64>;
    async fn count_user_orders(&self, user_id: Uuid) -> Result<u64>;
}

#[derive(Debug, Clone)]
pub struct PortfolioMetrics {
    pub balance: u64,
    pub balance_24h_ago: u64,
    pub volume_24h: u64,
    pub pnl_7d: i64,
    pub max_drawdown_1m: i64,
    pub total_deposits: u64,
    pub total_withdrawals: u64,
}

#[async_trait]
pub trait PortfolioRepository: Send + Sync {
    async fn get_portfolio(&self, user: Uuid, now_ts_secs: i64) -> Result<PortfolioMetrics>;
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
    async fn get_user_balances(&self, user_id: Uuid) -> Result<Vec<UserBalanceModel>> {
        // Query trading.balances_events by user_id
        // Note: Kafka writes user_id as i64, but we receive Uuid from API
        let user_id_i64 = user_id.as_u128() as i64;

        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: Uuid) -> Result<Option<UserBalanceModel>> {
        let user_id_i64 = user_id.as_u128() as i64;

        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()> {
        let user_id_i64 = user_id.as_u128() as i64;
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

    async fn create_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()> {
        let user_id_i64 = user_id.as_u128() as i64;
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

    async fn get_open_orders(&self, user_id: Uuid) -> Result<Vec<OrderModel>> {
        // Query trading.orders_events by order_user_id
        let user_id_i64 = user_id.as_u128() as i64;

        OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id_i64))
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: Uuid) -> Result<Option<OrderModel>> {
        let user_id_i64 = user_id.as_u128() as i64;

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

    async fn count_user_orders(&self, user_id: Uuid) -> Result<u64> {
        let user_id_i64 = user_id.as_u128() as i64;

        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id_i64))
            .count(&*self.db)
            .await?;
        Ok(count as u64)
    }
}

/// Portfolio Repository implementation
#[async_trait]
impl PortfolioRepository for PostgresTradingRepository {
    /// Calculates: current balance, 24h balance, 24h volume, 7d PnL, 1m max drawdown,
    /// total deposits, total withdrawals.
    ///
    /// Data sources: `balances_events`, `trades_events`, `deposits_events`, `withdrawals_events`.
    async fn get_portfolio(&self, user: Uuid, now_ts_secs: i64) -> Result<PortfolioMetrics> {
        let user_id_i64 = user.as_u128() as i64; 
        let cutoff_24h = now_ts_secs - 24 * 60 * 60;
        let cutoff_7d = now_ts_secs - 7 * 24 * 60 * 60;
        let cutoff_1m = now_ts_secs - 30 * 24 * 60 * 60;

        // Current balance: sum the latest `total` per asset.
        // Query: SELECT * FROM trading.balances_events WHERE user_id = ? ORDER BY timestamp DESC
        // Fetch all user balance events ordered by time desc; for each asset_id,
        // keep the first seen event (latest), then sum totals across assets.
        let latest_rows = UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await?;
        use std::collections::HashMap;
        let mut latest_per_asset: HashMap<i32, i64> = HashMap::new();
        for r in &latest_rows {
            latest_per_asset.entry(r.asset_id).or_insert(r.total);
        }
        let balance_now: i64 = latest_per_asset.values().sum();

        // Balance 24h ago: for each asset, take the latest event with ts <= cutoff_24h; sum totals.
        // Query: Uses same data as current balance (no additional query)
        // Filter balance_events by timestamp <= cutoff_24h, group by asset_id, take latest per asset, sum
        let mut latest_24h_per_asset: HashMap<i32, i64> = HashMap::new();
        for r in &latest_rows {
            if r.timestamp <= cutoff_24h {
                latest_24h_per_asset.entry(r.asset_id).or_insert(r.total);
            }
        }
        let balance_24h_ago: i64 = latest_24h_per_asset.values().sum();

        // Balance 7d ago: same as above with ts <= cutoff_7d.
        // Query: Uses same data as current balance (no additional query)
        // Filter balance_events by timestamp <= cutoff_7d, group by asset_id, take latest per asset, sum
        let mut latest_7d_per_asset: HashMap<i32, i64> = HashMap::new();
        for r in &latest_rows {
            if r.timestamp <= cutoff_7d {
                latest_7d_per_asset.entry(r.asset_id).or_insert(r.total);
            }
        }
        let balance_7d_ago: i64 = latest_7d_per_asset.values().sum();

        // 24h volume: sum price * size for trades where user is maker or taker and ts >= cutoff_24h.
        // Query: SELECT * FROM trading.trades_events WHERE (maker_user_id = ? OR taker_user_id = ?) AND timestamp >= ?
        // Filter trades by user participation (maker OR taker) and timestamp >= cutoff_24h, sum price*size
        let volume_24h: i64 = TradeEntity::find()
            .filter(
                Condition::any()
                    .add(TradeColumn::MakerUserId.eq(user_id_i64))
                    .add(TradeColumn::TakerUserId.eq(user_id_i64)),
            )
            .filter(TradeColumn::Timestamp.gte(cutoff_24h))
            .all(&*self.db)
            .await?
            .into_iter()
            .map(|t| t.price.saturating_mul(t.size))
            .sum();

        // Lifetime deposits/withdrawals: sum amounts across all user events.
        // Query: SELECT amount FROM trading.deposits_events WHERE user_id = ?
        //        SELECT amount FROM trading.withdrawals_events WHERE user_id = ?
        // Simple SUM aggregation on deposits and withdrawals tables
        let total_deposits: i64 = DepositEntity::find()
            .filter(DepositColumn::UserId.eq(user_id_i64))
            .all(&*self.db)
            .await?
            .into_iter()
            .map(|d| d.amount)
            .sum();
        let total_withdrawals: i64 = WithdrawalEntity::find()
            .filter(WithdrawalColumn::UserId.eq(user_id_i64))
            .all(&*self.db)
            .await?
            .into_iter()
            .map(|w| w.amount)
            .sum();

        // 7d deposits/withdrawals: sum amounts where ts > cutoff_7d.
        // Query: SELECT amount FROM trading.deposits_events WHERE user_id = ? AND timestamp > ?
        //        SELECT amount FROM trading.withdrawals_events WHERE user_id = ? AND timestamp > ?
        // Filter by user_id and timestamp > cutoff_7d, sum amounts
        let deposits_7d: i64 = DepositEntity::find()
            .filter(DepositColumn::UserId.eq(user_id_i64))
            .filter(DepositColumn::Timestamp.gt(cutoff_7d))
            .all(&*self.db)
            .await?
            .into_iter()
            .map(|d| d.amount)
            .sum();
        let withdrawals_7d: i64 = WithdrawalEntity::find()
            .filter(WithdrawalColumn::UserId.eq(user_id_i64))
            .filter(WithdrawalColumn::Timestamp.gt(cutoff_7d))
            .all(&*self.db)
            .await?
            .into_iter()
            .map(|w| w.amount)
            .sum();

        // Max drawdown (1m): peak-to-trough decline over last 30 days.
        // Query: SELECT timestamp, total FROM trading.balances_events WHERE user_id = ? AND timestamp >= ? ORDER BY timestamp ASC
        // Within [cutoff_1m, now], aggregate total per timestamp (summing across assets),
        // iterate in time order, track running peak, compute (peak - value) at each point, take max.
        let series_rows = UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .filter(UserBalanceColumn::Timestamp.gte(cutoff_1m))
            .order_by_asc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await?;
        let mut by_ts: HashMap<i64, i64> = HashMap::new();
        for r in series_rows {
            let entry = by_ts.entry(r.timestamp).or_insert(0);
            *entry += r.total;
        }
        let mut timestamps: Vec<i64> = by_ts.keys().copied().collect();
        timestamps.sort_unstable();
        let mut peak: i64 = 0;
        let mut max_drawdown: i64 = 0;
        for ts in timestamps {
            let v = *by_ts.get(&ts).unwrap_or(&0);
            if v > peak { peak = v; }
            let dd = peak - v;
            if dd > max_drawdown { max_drawdown = dd; }
        }

        // 7d PnL: (current_balance - balance_7d_ago) - (deposits_7d - withdrawals_7d).
        // Query: Uses previously calculated values (no additional query)
        // (total_portfolio_change) - (external_cash_flows) = pure_trading_pnl
        let pnl_7d = (balance_now - balance_7d_ago) - (deposits_7d - withdrawals_7d);

        Ok(PortfolioMetrics {
            balance: balance_now.max(0) as u64,
            balance_24h_ago: balance_24h_ago.max(0) as u64,
            volume_24h: volume_24h.max(0) as u64,
            pnl_7d,
            max_drawdown_1m: max_drawdown,
            total_deposits: total_deposits.max(0) as u64,
            total_withdrawals: total_withdrawals.max(0) as u64,
        })
    }
}
