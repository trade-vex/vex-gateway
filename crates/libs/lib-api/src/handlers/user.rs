//! User Account Data API Handlers

use actix_web::{HttpResponse, Result, web};
use lib_core::{PnlResolution, PortfolioRepository, TradingRepository};
use lib_core::dto::trading::{BalanceChartResponse, PnlChartResponse, PortfolioMetricsResponse};
use lib_core::{
    BalancesListResponse, FavoriteMarketsListResponse,
    OrderHistoryResponse, OrderResponse, OrderStore, OrdersHistoryListResponse,
    OrdersListResponse, ToggleFavoriteMarketRequest, UserBalanceResponse, UserBalanceStore,
};
use lib_utils::auth_extractors::AuthenticatedUserId;
use serde::Deserialize;
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

/// Get portfolio metrics
///
/// Returns portfolio metrics including balance, volume, PnL, and drawdown.
/// Does not include PnL time series (use /api/pnl-chart for that).
///
/// # Endpoint
/// `GET /api/portfolio`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Data Sources (ClickHouse)
/// - `trading.equity_per_minute(user_id, minute, equity_quote)`
///   ReplacingMergeTree with per-minute equity valued in configured quote asset.
/// - `trading.trade_events(ts, market_id, price, size_base, maker_user_id, taker_user_id, ...)`
///   Raw trades for user volume aggregation.
///
/// # Metrics
/// - `balance` (now): last ASOF-valued `equity_quote` at or before current time
/// - `balance_24h_ago`: last ASOF-valued `equity_quote` at or before now - 24h
/// - `pnl_7d`: `equity(now) - equity(now - 7d) - cashflow(7d)` with cashflow priced via ASOF joins
/// - `max_drawdown_1m`: max peak-to-trough drawdown of ASOF-valued equity over last 30d using `runningMax`
/// - `volume_24h`: sum(`price * size_base`) priced per trade for `ts in (now-24h, now]`
///
/// The handler is backend-agnostic and uses the injected `PortfolioRepository` which is
/// implemented against ClickHouse in production.
///
/// # Example Request
/// ```bash
/// curl -X GET "http://localhost:8081/api/portfolio" \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "balance": "10500.50",
///   "balance_24h_ago": "10000.00",
///   "volume_24h": "2500.75",
///   "pnl_7d": "500.50",
///   "max_drawdown_1m": "250.25"
/// }
/// ```
pub async fn get_portfolio_overview(
    user: AuthenticatedUserId,
    portfolio_repo: web::Data<Arc<dyn PortfolioRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let now_secs = chrono::Utc::now().timestamp();

    // Use default resolution (Day) for metrics - resolution only affects pnl_series which we don't need here
    let resolution = PnlResolution::Day;

    let metrics = portfolio_repo
        .get_portfolio(user_id, now_secs, resolution)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

    let response = PortfolioMetricsResponse::new(
        metrics.balance,
        metrics.balance_24h_ago,
        metrics.volume_24h,
        metrics.pnl_7d,
        metrics.max_drawdown_1m,
    );

    Ok(HttpResponse::Ok().json(response))
}

/// Get balance chart
///
/// Returns account balance over time with different resolutions.
/// Uses ClickHouse equity_per_minute table to compute balance series.
///
/// # Endpoint
/// `GET /api/user/balance-chart?resolution={resolution}`
///
/// # Query Parameters
/// - `resolution` (optional): Time resolution for the chart (1d, 1w, 1m, 1y). Defaults to 1d.
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Data Sources (ClickHouse)
/// - `trading.equity_per_minute(user_id, minute, equity_quote)`
///   Per-minute equity valued in configured quote asset.
///
/// # Response
/// Returns [`BalanceChartResponse`] containing balance time series.
///
/// # Example Request
/// ```bash
/// curl -X GET "http://localhost:8080/api/user/balance-chart?resolution=1w" \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "balance_series": [
///     {
///       "timestamp": 1697452800,
///       "balance": "10500.50"
///     },
///     {
///       "timestamp": 1697539200,
///       "balance": "10750.25"
///     }
///   ],
///   "resolution": "1w"
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct BalanceChartQuery {
    pub resolution: Option<String>,
}

pub async fn get_balance_chart(
    user: AuthenticatedUserId,
    query: web::Query<BalanceChartQuery>,
    portfolio_repo: web::Data<Arc<dyn PortfolioRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let now_secs = chrono::Utc::now().timestamp();

    let resolution = query
        .resolution
        .as_deref()
        .and_then(PnlResolution::from_query)
        .unwrap_or_default();

    let balance_series = portfolio_repo
        .get_balance_chart(user_id, now_secs, resolution)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

    let response = BalanceChartResponse {
        balance_series,
        resolution: resolution.as_str().to_string(),
    };

    Ok(HttpResponse::Ok().json(response))
}

/// Get PnL chart
///
/// Returns profit & loss time series with different resolutions.
/// Uses ClickHouse equity_per_minute and cashflow tables to compute PnL series.
///
/// # Endpoint
/// `GET /api/pnl-chart?resolution={resolution}`
///
/// # Query Parameters
/// - `resolution` (optional): Time resolution for the chart (1d, 1w, 1m, 1y). Defaults to 1d.
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Data Sources (ClickHouse)
/// - `trading.equity_per_minute(user_id, minute, equity_quote)`
///   Per-minute equity valued in configured quote asset.
/// - `trading.user_cashflow_per_minute(user_id, minute, cashflow_quote)`
///   Per-minute cashflow for accounting PnL calculation.
///
/// # Response
/// Returns [`PnlChartResponse`] containing PnL time series.
///
/// # Example Request
/// ```bash
/// curl -X GET "http://localhost:8081/api/pnl-chart?resolution=1w" \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "pnl_series": [
///     {
///       "timestamp": 1697452800,
///       "pnl": "100.00"
///     },
///     {
///       "timestamp": 1697539200,
///       "pnl": "200.00"
///     },
///     {
///       "timestamp": 1697625600,
///       "pnl": "300.00"
///     }
///   ],
///   "resolution": "1w"
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct PnlChartQuery {
    pub resolution: Option<String>,
}

pub async fn get_pnl_chart(
    user: AuthenticatedUserId,
    query: web::Query<PnlChartQuery>,
    portfolio_repo: web::Data<Arc<dyn PortfolioRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let now_secs = chrono::Utc::now().timestamp();

    let resolution = query
        .resolution
        .as_deref()
        .and_then(PnlResolution::from_query)
        .unwrap_or_default();

    // Get portfolio metrics to extract pnl_series
    let metrics = portfolio_repo
        .get_portfolio(user_id, now_secs, resolution)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

    let response = PnlChartResponse {
        pnl_series: metrics.pnl_series,
        resolution: resolution.as_str().to_string(),
    };

    Ok(HttpResponse::Ok().json(response))
}

/// Get user's favorite markets
///
/// Retrieves all markets that the authenticated user has marked as favorites.
/// Returns a list of favorite markets with metadata about when they were favorited.
///
/// # Endpoint
/// `GET /api/user/favorites`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Response
/// Returns [`FavoriteMarketsListResponse`] containing all favorited markets.
///
/// # Example Request
/// ```bash
/// curl -X GET http://localhost:8080/api/user/favorites \
///   -H "Authorization: Bearer <jwt_token>"
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "market_ids": [1, 5, 10]
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "error": "failed to retrieve favorite markets",
///   "message": "database connection lost"
/// }
/// ```
///
/// # Data Source
/// - Primary: PostgreSQL database
pub async fn get_favorite_markets(
    user: AuthenticatedUserId,
    trading_repo: web::Data<Arc<dyn TradingRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    match trading_repo.get_user_favorite_markets(user_id).await {
        Ok(favorites) => {
            let market_ids: Vec<u32> =
                favorites.into_iter().map(|fav| fav.market_id as u32).collect();

            let response = FavoriteMarketsListResponse { market_ids };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get favorite markets: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve favorite markets",
                "message": e.to_string()
            })))
        }
    }
}

/// Toggle a market as favorite
///
/// Adds or removes a market from the user's favorites list.
/// If the market is already favorited, it will be removed. If not, it will be added.
///
/// # Endpoint
/// `POST /api/user/favorites`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Request Body
/// ```json
/// {
///   "market_id": 1
/// }
/// ```
///
/// # Response
/// Returns a success message indicating whether the market was added or removed.
///
/// # Example Request
/// ```bash
/// curl -X POST http://localhost:8080/api/user/favorites \
///   -H "Authorization: Bearer <jwt_token>" \
///   -H "Content-Type: application/json" \
///   -d '{"market_id": 1}'
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "success": true,
///   "action": "added",
///   "market_id": 1
/// }
/// ```
/// or
/// ```json
/// {
///   "success": true,
///   "action": "removed",
///   "market_id": 1
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "error": "failed to toggle favorite market",
///   "message": "database error"
/// }
/// ```
///
/// # Data Source
/// - Primary: PostgreSQL database
pub async fn toggle_favorite_market(
    user: AuthenticatedUserId,
    request: web::Json<ToggleFavoriteMarketRequest>,
    trading_repo: web::Data<Arc<dyn TradingRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let market_id = request.market_id;

    // Check if market is already favorited
    match trading_repo.is_market_favorited(user_id, market_id).await {
        Ok(is_favorited) => {
            if is_favorited {
                // Remove from favorites
                match trading_repo.remove_favorite_market(user_id, market_id).await {
                    Ok(_) => Ok(HttpResponse::Ok().json(serde_json::json!({
                        "success": true,
                        "action": "removed",
                        "market_id": market_id
                    }))),
                    Err(e) => {
                        tracing::error!("failed to remove favorite market: {}", e);
                        Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                            "error": "failed to remove favorite market",
                            "message": e.to_string()
                        })))
                    }
                }
            } else {
                // Add to favorites
                match trading_repo.add_favorite_market(user_id, market_id).await {
                    Ok(_) => Ok(HttpResponse::Ok().json(serde_json::json!({
                        "success": true,
                        "action": "added",
                        "market_id": market_id
                    }))),
                    Err(e) => {
                        tracing::error!("failed to add favorite market: {}", e);
                        Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                            "error": "failed to add favorite market",
                            "message": e.to_string()
                        })))
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("failed to check if market is favorited: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to toggle favorite market",
                "message": e.to_string()
            })))
        }
    }
}
