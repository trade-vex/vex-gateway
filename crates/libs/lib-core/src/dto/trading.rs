use serde::{Deserialize, Serialize};

// Re-export models from separate modules
pub use crate::model::orders::Model as OrderModel;
pub use crate::model::user_balances::Model as UserBalanceModel;
pub use crate::model::user_favorite_markets::Model as UserFavoriteMarketModel;
use crate::{BalancePoint, PnlPoint, PnlResolution};
use rust_decimal::Decimal;

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
        // Case-insensitive matching with common aliases and protobuf format
        match s.to_lowercase().as_str() {
            "bid" | "buy" | "side_bid" => Ok(Side::Bid),
            "ask" | "sell" | "side_ask" => Ok(Side::Ask),
            _ => Err(format!(
                "Invalid side: '{}'. Expected: Bid, Ask, Buy, Sell, SIDE_BID, or SIDE_ASK",
                s
            )),
        }
    }
}

// API Response DTOs
/// Open order response: size = remaining (original - filled)
#[derive(Debug, Serialize, Deserialize)]
pub struct OrderResponse {
    #[serde(serialize_with = "serialize_i64_as_string", deserialize_with = "deserialize_i64_from_string")]
    pub order_id: i64,
    #[serde(serialize_with = "serialize_i64_as_string", deserialize_with = "deserialize_i64_from_string")]
    pub user_id: i64,
    pub market_id: i32,
    pub price: Decimal,
    /// Remaining size (order_size - order_filled_size)
    pub size: Decimal,
    pub side: Side,
    pub timestamp: i64,
    pub time_in_force: String,
    pub status: String,
}

fn serialize_i64_as_string<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn deserialize_i64_from_string<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let s = String::deserialize(deserializer)?;
    s.parse::<i64>().map_err(|e| D::Error::custom(format!("Invalid i64: {}", e)))
}

/// Enhanced order response for order history with status
/// size = original order size, filled = filled amount
#[derive(Debug, Serialize, Deserialize)]
pub struct OrderHistoryResponse {
    #[serde(serialize_with = "serialize_i64_as_string", deserialize_with = "deserialize_i64_from_string")]
    pub order_id: i64,
    #[serde(serialize_with = "serialize_i64_as_string", deserialize_with = "deserialize_i64_from_string")]
    pub user_id: i64,
    pub market_id: i32,
    pub price: Decimal,
    /// Original order size
    pub size: Decimal,
    /// Filled amount
    pub filled: Decimal,
    pub side: Side,
    pub timestamp: i64,
    pub time_in_force: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserBalanceResponse {
    #[serde(serialize_with = "serialize_i64_as_string", deserialize_with = "deserialize_i64_from_string")]
    pub user_id: i64,
    pub asset_id: u32,
    pub available: Decimal,
    pub locked: Decimal,
    pub total: Decimal,
    pub timestamp: u64,
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
    pub page: usize,
    pub limit: usize,
    pub total_pages: usize,
    pub has_next: bool,
    pub has_prev: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrdersHistoryListResponse {
    pub orders: Vec<OrderHistoryResponse>,
    pub total: usize,
    pub page: usize,
    pub limit: usize,
    pub total_pages: usize,
    pub has_next: bool,
    pub has_prev: bool,
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
