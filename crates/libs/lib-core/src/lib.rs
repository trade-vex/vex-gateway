pub mod cache;
pub mod correlation;
pub mod dto;
pub mod engine_link;
pub mod model;
pub mod order_idempotency;
pub mod repository;

pub use dto::trading::{
    BalanceChartResponse, BalancesListResponse, FavoriteMarketsListResponse, OrderHistoryResponse,
    OrderResponse, OrdersHistoryListResponse, OrdersListResponse, Side,
    ToggleFavoriteMarketRequest, UserBalanceResponse,
};

pub use repository::repository::{
    BalancePoint, PnlPoint, PnlResolution, PortfolioMetrics, PortfolioRepository,
    PostgresTradingRepository, Result as TradingResult, TradingError, TradingRepository,
};

pub use cache::user_store::UserBalanceStore;

pub use cache::order_store::OrderStore;

pub use correlation::{
    generate_client_order_id, wait_for_response, CorrelationService, ResponseError,
};

pub use engine_link::{CoreConnector, EngineLink, EngineLinkSupervisor, LinkPublisher};

pub use order_idempotency::{
    CompletionWait, IdempotentOrderOutcome, OrderClaim, OrderIdempotencyError,
    OrderIdempotencyStore, ReleaseReason,
};

// Type alias for the Publisher from vex-networking
// Shared across the application to avoid circular dependencies
pub type Publisher = vex_networking::client::Publisher;
