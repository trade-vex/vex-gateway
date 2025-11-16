use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// Re-export models from separate modules
pub use crate::model::orders::Model as OrderModel;
pub use crate::model::user_balances::Model as UserBalanceModel;
pub use crate::model::user_favorite_markets::Model as UserFavoriteMarketModel;
use crate::{BalancePoint, PnlPoint, PnlResolution};

// Simple types as specified
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Side {
    Bid,
    Ask,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Bid => write!(f, "Bid"),
            Side::Ask => write!(f, "Ask"),
        }
    }
}

impl std::str::FromStr for Side {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Bid" => Ok(Side::Bid),
            "Ask" => Ok(Side::Ask),
            _ => Err(format!("Invalid side: {}", s)),
        }
    }
}

// API Response DTOs
#[derive(Debug, Serialize, Deserialize)]
pub struct OrderResponse {
    pub order_id: u64,
    pub user_id: u64, // Changed from Uuid to u64 to match DB schema
    pub market_id: u32,
    pub price: Decimal,
    pub size: Decimal,
    pub side: Side,
    pub timestamp: u64,
    pub time_in_force: String,
    pub status: String,
}

/// Enhanced order response for order history with status and fill information
#[derive(Debug, Serialize, Deserialize)]
pub struct OrderHistoryResponse {
    pub order_id: u64,
    pub user_id: u64,
    pub market_id: u32,
    pub price: Decimal,
    pub size: Decimal,
    pub side: Side,
    pub timestamp: u64,
    pub time_in_force: String,
    pub status: String,
    pub filled_size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserBalanceResponse {
    pub user_id: u64, // Changed from Uuid to u64 to match DB schema
    pub asset_id: u32,
    pub available: u64,
    pub locked: u64,
    pub total: u64,
    pub timestamp: u64, // Changed from DateTime to unix timestamp
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BalancesListResponse {
    pub balances: Vec<UserBalanceResponse>,
    pub total_assets: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrdersListResponse {
    pub orders: Vec<OrderResponse>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrdersHistoryListResponse {
    pub orders: Vec<OrderHistoryResponse>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PortfolioOverviewResponse {
    pub balance: Decimal,
    pub balance_24h_ago: Decimal,
    pub volume_24h: Decimal,
    pub pnl_7d: Decimal,
    pub max_drawdown_1m: Decimal,
    pub pnl_series: Vec<PnlPoint>,
    pub pnl_resolution: String,
}

/// Portfolio metrics response (without PnL series)
#[derive(Debug, Serialize, Deserialize)]
pub struct PortfolioMetricsResponse {
    pub balance: Decimal,
    pub balance_24h_ago: Decimal,
    pub volume_24h: Decimal,
    pub pnl_7d: Decimal,
    pub max_drawdown_1m: Decimal,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BalanceChartResponse {
    pub balance_series: Vec<BalancePoint>,
    pub resolution: String,
}

/// PnL chart response
#[derive(Debug, Serialize, Deserialize)]
pub struct PnlChartResponse {
    pub pnl_series: Vec<PnlPoint>,
    pub resolution: String,
}

/// List of favorite markets response
#[derive(Debug, Serialize, Deserialize)]
pub struct FavoriteMarketsListResponse {
    pub market_ids: Vec<u32>,
}

/// Request to add/toggle a favorite market
#[derive(Debug, Deserialize)]
pub struct ToggleFavoriteMarketRequest {
    pub market_id: u32,
}

// Conversion implementations
impl From<OrderModel> for OrderResponse {
    fn from(model: OrderModel) -> Self {
        Self {
            order_id: model.order_order_id as u64,
            user_id: model.order_user_id as u64,
            market_id: model.market_id as u32,
            price: model.order_price,
            size: model.order_size,
            side: model.order_side.parse().unwrap_or(Side::Bid),
            timestamp: model.order_timestamp as u64,
            time_in_force: model.time_in_force,
            status: model.status,
        }
    }
}

impl From<UserBalanceModel> for UserBalanceResponse {
    fn from(model: UserBalanceModel) -> Self {
        Self {
            user_id: model.user_id as u64,
            asset_id: model.asset_id as u32,
            available: model.available as u64,
            locked: model.locked as u64,
            total: model.total as u64,
            timestamp: model.timestamp as u64,
        }
    }
}

impl From<OrderModel> for OrderHistoryResponse {
    fn from(model: OrderModel) -> Self {
        Self {
            order_id: model.order_order_id as u64,
            user_id: model.order_user_id as u64,
            market_id: model.market_id as u32,
            price: model.order_price,
            size: model.order_size,
            side: model.order_side.parse().unwrap_or(Side::Bid),
            timestamp: model.order_timestamp as u64,
            time_in_force: model.time_in_force,
            status: model.status,
            filled_size: 0,
        }
    }
}

impl PortfolioOverviewResponse {
    pub fn with_metrics(
        balance: Decimal,
        balance_24h_ago: Decimal,
        volume_24h: Decimal,
        pnl_7d: Decimal,
        max_drawdown_1m: Decimal,
        pnl_series: Vec<PnlPoint>,
        resolution: PnlResolution,
    ) -> Self {
        Self {
            balance,
            balance_24h_ago,
            volume_24h,
            pnl_7d,
            max_drawdown_1m,
            pnl_series,
            pnl_resolution: resolution.as_str().to_string(),
        }
    }
}

impl PortfolioMetricsResponse {
    pub fn new(
        balance: Decimal,
        balance_24h_ago: Decimal,
        volume_24h: Decimal,
        pnl_7d: Decimal,
        max_drawdown_1m: Decimal,
    ) -> Self {
        Self {
            balance,
            balance_24h_ago,
            volume_24h,
            pnl_7d,
            max_drawdown_1m,
        }
    }
}

