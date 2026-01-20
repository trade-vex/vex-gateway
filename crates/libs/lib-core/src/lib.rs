//! Core domain logic and data structures for the VEX Gateway.
//!
//! This crate provides the foundational types and services used throughout
//! the gateway, including:
//!
//! - **DTOs**: Data transfer objects for API requests/responses
//! - **Cache**: Redis-backed caching for orders and user balances
//! - **Repository**: Database access layer for trading data
//! - **Correlation**: Request/response correlation for async order processing
//! - **Publisher**: Lock-free channel-based message publishing to VEX Core

pub mod cache;
pub mod correlation;
pub mod dto;
pub mod model;
pub mod publisher_handle;
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

pub use cache::user_store::{UserBalanceCacheConfig, UserBalanceStore};

pub use cache::order_store::{OrderCacheConfig, OrderStore};

pub use correlation::{
    generate_client_order_id, wait_for_response, CorrelationService, ResponseError,
};

// Type alias for the Publisher from vex-networking
// Shared across the application to avoid circular dependencies
pub type Publisher = vex_networking::client::Publisher;

// Re-export the channel-based PublisherHandle for lock-free order publishing
pub use publisher_handle::{PublisherError, PublisherHandle};
