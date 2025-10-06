pub mod cache;
pub mod dto;
pub mod model;
pub mod repository;

pub use dto::trading::{
    BalancesListResponse,
    OrdersListResponse,
    UserBalanceResponse,
    OrderResponse,
    Side,
};

pub use repository::repository::{
    TradingRepository,
    TradingError,
    Result as TradingResult,
    PostgresTradingRepository,
};

pub use cache::user_store::{
    UserBalanceStore,
    UserBalanceCacheConfig,
};

pub use cache::order_store::{
    OrderStore,
    OrderCacheConfig,
};

// Type alias for the Publisher from vex-networking
// Shared across the application to avoid circular dependencies
pub type Publisher = vex_networking::client::Publisher;