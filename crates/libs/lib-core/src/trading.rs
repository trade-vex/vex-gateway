use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use uuid::Uuid;

// Re-export models from separate modules
pub use crate::orders::Model as OrderModel;
pub use crate::user_balances::Model as UserBalanceModel;

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
    pub user_id: Uuid,
    pub price: u64,
    pub size: u64,
    pub side: Side,
    pub timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserBalanceResponse {
    pub user_id: Uuid,
    pub available: u64,
    pub locked: u64,
    pub total: u64,
    pub last_updated: DateTime<Utc>,
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

// Conversion implementations
impl From<OrderModel> for OrderResponse {
    fn from(model: OrderModel) -> Self {
        Self {
            order_id: model.order_id as u64,
            user_id: model.user_id,
            price: model.price as u64,
            size: model.size as u64,
            side: model.side.parse().unwrap_or(Side::Bid),
            timestamp: model.timestamp as u64,
        }
    }
}

impl From<UserBalanceModel> for UserBalanceResponse {
    fn from(model: UserBalanceModel) -> Self {
        Self {
            user_id: model.user_id,
            available: model.available as u64,
            locked: model.locked as u64,
            total: (model.available + model.locked) as u64,
            last_updated: model.last_updated,
        }
    }
}