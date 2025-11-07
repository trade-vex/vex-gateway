use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// Re-export models from separate modules
pub use crate::model::orders::Model as OrderModel;
pub use crate::model::user_balances::Model as UserBalanceModel;

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
    pub price: u64,
    pub size: u64,
    pub side: Side,
    pub timestamp: u64,
}

/// Enhanced order response for order history with status and fill information
#[derive(Debug, Serialize, Deserialize)]
pub struct OrderHistoryResponse {
    pub order_id: u64,
    pub user_id: u64,
    pub market_id: u32,
    pub price: u64,
    pub size: u64,
    pub side: Side,
    pub timestamp: u64,
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
}

// Conversion implementations
impl From<OrderModel> for OrderResponse {
    fn from(model: OrderModel) -> Self {
        Self {
            order_id: model.order_order_id as u64,
            user_id: model.order_user_id as u64,
            market_id: model.market_id as u32,
            price: model.order_price as u64,
            size: model.order_size as u64,
            side: model.order_side.parse().unwrap_or(Side::Bid),
            timestamp: model.order_timestamp as u64,
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
            price: model.order_price as u64,
            size: model.order_size as u64,
            side: model.order_side.parse().unwrap_or(Side::Bid),
            timestamp: model.order_timestamp as u64,
            status: "Open".to_string(),
            filled_size: 0,
        }
    }
}
