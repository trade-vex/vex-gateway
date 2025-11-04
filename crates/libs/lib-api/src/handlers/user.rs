//! User Account Data API Handlers

use actix_web::{HttpResponse, Result, web};
use lib_core::PortfolioRepository;
use lib_core::dto::trading::PortfolioOverviewResponse;
use lib_core::{
    BalancesListResponse, OrderHistoryResponse, OrderResponse, OrderStore,
    OrdersHistoryListResponse, OrdersListResponse, UserBalanceResponse, UserBalanceStore,
};
use lib_utils::auth_extractors::AuthenticatedUserId;
use std::sync::Arc;

/// Get user balances for all assets
///
/// Retrieves the current balance information for all assets held by the authenticated user.
/// Balances include available, locked, and total amounts for each asset.
///
/// # Endpoint
/// `GET /api/user/balances`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Response
/// Returns [`BalancesListResponse`] containing all user balances.
///
/// # Example Request
/// ```bash
/// curl -X GET http://localhost:8080/api/user/balances \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "balances": [
///     {
///       "user_id": 123456,
///       "asset_id": 1,
///       "available": 1000000,
///       "locked": 50000,
///       "total": 1050000,
///       "timestamp": 1697452800000000
///     },
///     {
///       "user_id": 123456,
///       "asset_id": 2,
///       "available": 5000000,
///       "locked": 100000,
///       "total": 5100000,
///       "timestamp": 1697452800000000
///     }
///   ],
///   "total_assets": 2
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "error": "failed to retrieve balances",
///   "message": "cache connection timeout"
/// }
/// ```
///
/// # Data Source
/// - Primary: Redis cache (sub-millisecond latency)
/// - Fallback: PostgreSQL database (if cache miss)
/// - Updates: Real-time via Kafka event streaming from VEX Core
pub async fn get_balances(
    user: AuthenticatedUserId,
    balance_store: web::Data<Arc<UserBalanceStore>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    match balance_store.get_user_balances(user_id).await {
        Ok(balances) => {
            let total_assets = balances.len();
            let balance_responses: Vec<UserBalanceResponse> =
                balances.into_iter().map(|balance| balance.into()).collect();

            let response = BalancesListResponse {
                balances: balance_responses,
                total_assets,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get user balances: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve balances",
                "message": e.to_string()
            })))
        }
    }
}

/// Get user's open orders
///
/// Retrieves all currently open (unfilled) orders for the authenticated user across all markets.
/// Returns detailed information about each order including price, size, side, and current status.
///
/// # Endpoint
/// `GET /api/user/orders/open`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Response
/// Returns [`OrdersListResponse`] containing all open orders.
///
/// # Example Request
/// ```bash
/// curl -X GET http://localhost:8080/api/user/orders/open \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "orders": [
///     {
///       "order_id": 123456789,
///       "user_id": 123456,
///       "market_id": 1,
///       "price": 5000000,
///       "size": 100,
///       "side": "Bid",
///       "timestamp": 1697452800000000
///     },
///     {
///       "order_id": 987654321,
///       "user_id": 123456,
///       "market_id": 2,
///       "price": 10000000,
///       "size": 50,
///       "side": "Ask",
///       "timestamp": 1697452900000000
///     }
///   ],
///   "total": 2
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "error": "failed to retrieve open orders",
///   "message": "database connection lost"
/// }
/// ```
///
/// # Data Source
/// - Primary: Redis cache (sub-millisecond latency)
/// - Fallback: PostgreSQL database (if cache miss)
/// - Updates: Real-time via Kafka event streaming from VEX Core
///
/// # Order States
/// Only returns orders in "open" states:
/// - Pending - Order submitted but not yet processed
/// - PartiallyFilled - Order partially executed
/// - Processing - Order being matched
///
/// Excludes:
/// - Filled - Completely executed orders
/// - Cancelled - User-cancelled orders
/// - Rejected - Orders rejected by matching engine
pub async fn get_open_orders(
    user: AuthenticatedUserId,
    order_store: web::Data<Arc<OrderStore>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    match order_store.get_user_orders(user_id).await {
        Ok(orders) => {
            let total = orders.len();
            let order_responses: Vec<OrderResponse> =
                orders.into_iter().map(|order| order.into()).collect();

            let response = OrdersListResponse {
                orders: order_responses,
                total,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get open orders: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve open orders",
                "message": e.to_string()
            })))
        }
    }
}

/// Get user's complete order history
///
/// Retrieves all orders (open, filled, cancelled, rejected) for the authenticated user across all markets.
/// Returns detailed information about each order including price, size, side, status, and fill information.
///
/// # Endpoint
/// `GET /api/user/orders/history`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Response
/// Returns [`OrdersHistoryListResponse`] containing all orders with status and fill information.
///
/// # Example Request
/// ```bash
/// curl -X GET http://localhost:8080/api/user/orders/history \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "orders": [
///     {
///       "order_id": 123456789,
///       "user_id": 123456,
///       "market_id": 1,
///       "price": 5000000,
///       "size": 100,
///       "side": "Bid",
///       "timestamp": 1697452800000000,
///       "status": "Filled",
///       "filled_size": 100
///     },
///     {
///       "order_id": 987654321,
///       "user_id": 123456,
///       "market_id": 2,
///       "price": 10000000,
///       "size": 50,
///       "side": "Ask",
///       "timestamp": 1697452900000000,
///       "status": "PartiallyFilled",
///       "filled_size": 25
///     },
///     {
///       "order_id": 456789123,
///       "user_id": 123456,
///       "market_id": 1,
///       "price": 4500000,
///       "size": 75,
///       "side": "Bid",
///       "timestamp": 1697453000000000,
///       "status": "Cancelled",
///       "filled_size": 0
///     }
///   ],
///   "total": 3
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "error": "failed to retrieve order history",
///   "message": "database connection lost"
/// }
/// ```
///
/// # Data Source
/// - Primary: Redis cache (sub-millisecond latency)
/// - Fallback: PostgreSQL database (if cache miss)
/// - Updates: Real-time via Kafka event streaming from VEX Core
///
/// # Order States
/// Returns orders in ALL states:
/// - Pending - Order submitted but not yet processed
/// - PartiallyFilled - Order partially executed
/// - Processing - Order being matched
/// - Filled - Completely executed orders
/// - Cancelled - User-cancelled orders
/// - Rejected - Orders rejected by matching engine
///
/// # Fill Information
/// - `filled_size`: Amount of the order that has been executed
/// - `status`: Current status of the order (Pending, PartiallyFilled, Filled, Cancelled, Rejected)
pub async fn get_order_history(
    user: AuthenticatedUserId,
    order_store: web::Data<Arc<OrderStore>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    match order_store.get_user_order_history(user_id).await {
        Ok(orders) => {
            let total = orders.len();
            let order_responses: Vec<OrderHistoryResponse> =
                orders.into_iter().map(|order| order.into()).collect();

            let response = OrdersHistoryListResponse {
                orders: order_responses,
                total,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get order history: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve order history",
                "message": e.to_string()
            })))
        }
    }
}

/// Get portfolio overview
///
/// Computes portfolio metrics using ClickHouse materialized views and raw events.
///
/// # Data Sources (ClickHouse)
/// - `trading.equity_per_minute(user_id, minute, equity_quote)`
///   ReplacingMergeTree with per-minute equity valued in configured quote asset.
/// - `trading.trade_events(ts, market_id, price, size_base, maker_user_id, taker_user_id, ...)`
///   Raw trades for user volume aggregation.
///
/// # Metrics
/// - `balance` (now): last `equity_quote` at or before current time
/// - `balance_24h_ago`: last `equity_quote` at or before now - 24h
/// - `pnl_7d`: `equity(now) - equity(now - 7d)`
/// - `max_drawdown_1m`: max peak-to-trough drawdown of equity over last 30d using `runningMax`
/// - `volume_24h`: sum(`price * size_base`) where user is maker or taker and `ts >= now - 24h`
///
/// The handler is backend-agnostic and uses the injected `PortfolioRepository` which is
/// implemented against ClickHouse in production.
pub async fn get_portfolio_overview(
    user: AuthenticatedUserId,
    portfolio_repo: web::Data<Arc<dyn PortfolioRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let now_secs = chrono::Utc::now().timestamp();

    let metrics = portfolio_repo
        .get_portfolio(user_id, now_secs)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

    let response = PortfolioOverviewResponse {
        balance: metrics.balance,
        balance_24h_ago: metrics.balance_24h_ago,
        volume_24h: metrics.volume_24h,
        pnl_7d: metrics.pnl_7d,
        max_drawdown_1m: metrics.max_drawdown_1m,
    };

    Ok(HttpResponse::Ok().json(response))
}
