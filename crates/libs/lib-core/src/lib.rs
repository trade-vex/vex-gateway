pub mod cache;
pub mod correlation;
pub mod dto;
pub mod model;
pub mod repository;

pub use dto::trading::{
    BalancesListResponse, OrderHistoryResponse, OrderResponse, OrdersHistoryListResponse,
    OrdersListResponse, Side, UserBalanceResponse,
};

pub use repository::repository::{
    PnlPoint, PnlResolution, PortfolioMetrics, PortfolioRepository, PostgresTradingRepository,
    Result as TradingResult, TradingError, TradingRepository,
};

pub use cache::user_store::{UserBalanceCacheConfig, UserBalanceStore};

pub use cache::order_store::{OrderCacheConfig, OrderStore};

pub use correlation::{
    generate_client_order_id, wait_for_response, CorrelationService, ResponseError,
};

// Type alias for the Publisher from vex-networking
// Shared across the application to avoid circular dependencies
pub type Publisher = vex_networking::client::Publisher;
