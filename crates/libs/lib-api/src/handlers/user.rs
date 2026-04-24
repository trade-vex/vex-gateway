//! User Account Data API Handlers

use actix_web::{HttpResponse, Result, web};
use lib_core::dto::trading::{BalanceChartResponse, PnlChartResponse, PortfolioMetricsResponse};
use lib_core::{
    BalancesListResponse, FavoriteMarketsListResponse, OrderHistoryResponse, OrderResponse,
    OrderStore, OrdersHistoryListResponse, OrdersListResponse, ToggleFavoriteMarketRequest,
    UserBalanceResponse, UserBalanceStore,
};
use lib_core::{PnlResolution, PortfolioRepository, TradingRepository};
use lib_utils::auth_extractors::AuthenticatedUserId;
use serde::{Deserialize, Deserializer, de};
use std::str::FromStr;
use std::sync::Arc;

/// Parse an optional query-string value, treating missing *and* empty
/// (`?market_id=`) inputs as `None`. serde_urlencoded surfaces an empty value
/// as `Some("")`, which the default `Option<T: FromStr>` impl then rejects
/// with a 400 — not what clients with "clear filter" UIs expect. Applies to
/// any `FromStr` target so we can reuse it for `page`, `limit`, `market_id`.
fn deserialize_empty_string_as_none<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => T::from_str(s.trim())
            .map(Some)
            .map_err(de::Error::custom),
    }
}

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
            let balance_responses: Vec<UserBalanceResponse> = balances
                .into_iter()
                .map(|balance| balance_store.to_response(balance))
                .collect();

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
                "message": "An internal error occurred. Please try again later."
            })))
        }
    }
}

/// Get user's open orders
///
/// Retrieves currently open (unfilled) orders for the authenticated user across all markets.
/// Returns detailed information about each order including price, size, side, and current status.
/// Supports pagination via query parameters.
///
/// # Endpoint
/// `GET /api/user/orders/open?page=1&limit=20`
///
/// # Query Parameters
/// - `page` (optional): Page number (1-indexed). Defaults to 1.
/// - `limit` (optional): Number of orders per page. Defaults to 20, max 100.
/// - `market_id` (optional): Filter orders by market ID. If not provided, returns orders from all markets.
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Response
/// Returns [`OrdersListResponse`] containing paginated open orders with pagination metadata.
///
/// # Example Request
/// ```bash
/// curl -X GET "http://localhost:8080/api/user/orders/open?page=1&limit=20&market_id=1" \
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
///     }
///   ],
///   "total": 2,
///   "page": 1,
///   "limit": 20,
///   "total_pages": 1,
///   "has_next": false,
///   "has_prev": false
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
/// - Placed - Order placed but not yet processed
/// - Pending - Order submitted but not yet processed
/// - PartiallyFilled - Order partially executed
/// - Processing - Order being matched
///
/// Excludes:
/// - Filled - Completely executed orders
/// - Cancelled - User-cancelled orders
/// - Rejected - Orders rejected by matching engine
#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    #[serde(default, deserialize_with = "deserialize_empty_string_as_none")]
    pub page: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_empty_string_as_none")]
    pub limit: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_empty_string_as_none")]
    pub market_id: Option<u32>,
}

pub async fn get_open_orders(
    user: AuthenticatedUserId,
    query: web::Query<OrdersQuery>,
    order_store: web::Data<Arc<OrderStore>>,
    _trading_repo: web::Data<Arc<dyn TradingRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    // Parse pagination parameters
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = ((page - 1) * limit) as u64;
    let limit_u64 = limit as u64;

    // Fetch limit+1 rows to derive has_next without a separate COUNT(*).
    // This keeps each request to one DB connection and avoids an index scan
    // proportional to the user's full open-order set (whales).
    match order_store
        .get_user_orders_paginated(user_id, offset, limit_u64 + 1, query.market_id)
        .await
    {
        Ok(mut orders) => {
            let has_next = orders.len() > limit;
            if has_next {
                orders.truncate(limit);
            }
            let order_responses: Vec<OrderResponse> = orders
                .into_iter()
                .map(|order| order_store.to_response(order))
                .collect();
            let has_prev = page > 1;

            let response = OrdersListResponse {
                orders: order_responses,
                total: None,
                page,
                limit,
                total_pages: None,
                has_next,
                has_prev,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get open orders for user_id {}: {}", user_id, e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve open orders",
                "message": "An internal error occurred. Please try again later."
            })))
        }
    }
}

/// Get user's complete order history
///
/// Retrieves orders (filled, cancelled, partially filled) for the authenticated user across all markets.
/// Returns detailed information about each order including price, size, side, status, and fill information.
/// Supports pagination via query parameters.
///
/// # Endpoint
/// `GET /api/user/orders/history?page=1&limit=20`
///
/// # Query Parameters
/// - `page` (optional): Page number (1-indexed). Defaults to 1.
/// - `limit` (optional): Number of orders per page. Defaults to 20, max 100.
/// - `market_id` (optional): Filter orders by market ID. If not provided, returns orders from all markets.
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Response
/// Returns [`OrdersHistoryListResponse`] containing paginated orders with status and fill information.
///
/// # Example Request
/// ```bash
/// curl -X GET "http://localhost:8080/api/user/orders/history?page=1&limit=20&market_id=1" \
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
///       "market_id": 65538,
///       "price": 5000000,
///       "size": 100,
///       "filled": 100,
///       "side": "Bid",
///       "timestamp": 1697452800000000,
///       "time_in_force": "GTC",
///       "status": "Filled"
///     }
///   ],
///   "total": 3,
///   "page": 1,
///   "limit": 20,
///   "total_pages": 1,
///   "has_next": false,
///   "has_prev": false
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
/// Returns orders in terminal states:
/// - PartiallyFilled - Order partially executed
/// - Filled - Completely executed orders
/// - Cancelled - User-cancelled orders
pub async fn get_order_history(
    user: AuthenticatedUserId,
    query: web::Query<OrdersQuery>,
    order_store: web::Data<Arc<OrderStore>>,
    _trading_repo: web::Data<Arc<dyn TradingRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    // Parse pagination parameters
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = ((page - 1) * limit) as u64;
    let limit_u64 = limit as u64;

    // Fetch limit+1 rows to derive has_next without a separate COUNT(*).
    // Whales can have hundreds of thousands of history rows — a per-request
    // count scan (even index-only) is pool-pressure we can't afford.
    match order_store
        .get_user_order_history_paginated(user_id, offset, limit_u64 + 1, query.market_id)
        .await
    {
        Ok(mut orders) => {
            let has_next = orders.len() > limit;
            if has_next {
                orders.truncate(limit);
            }
            let order_responses: Vec<OrderHistoryResponse> = orders
                .into_iter()
                .map(|order| order_store.to_history_response(order))
                .collect();
            let has_prev = page > 1;

            let response = OrdersHistoryListResponse {
                orders: order_responses,
                total: None,
                page,
                limit,
                total_pages: None,
                has_next,
                has_prev,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get order history for user_id {}: {}", user_id, e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve order history",
                "message": "An internal error occurred. Please try again later."
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

    let metrics = portfolio_repo
        .get_portfolio_overview(user_id, now_secs)
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

    let pnl_series = portfolio_repo
        .get_pnl_chart(user_id, now_secs, resolution)
        .await
        .map_err(|e| actix_web::error::ErrorInternalServerError(e.to_string()))?;

    let response = PnlChartResponse {
        pnl_series,
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
            let market_ids: Vec<u32> = favorites
                .into_iter()
                .filter_map(|fav| {
                    u32::try_from(fav.market_id).ok().or_else(|| {
                        tracing::warn!(
                            market_id = fav.market_id,
                            "Invalid market_id (negative value), skipping"
                        );
                        None
                    })
                })
                .collect();

            let response = FavoriteMarketsListResponse { market_ids };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("failed to get favorite markets: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve favorite markets",
                "message": "An internal error occurred. Please try again later."
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

    // Use atomic toggle operation to prevent race conditions
    match trading_repo
        .toggle_favorite_market(user_id, market_id)
        .await
    {
        Ok(was_added) => {
            let action = if was_added { "added" } else { "removed" };
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "success": true,
                "action": action,
                "market_id": market_id
            })))
        }
        Err(e) => {
            tracing::error!("failed to toggle favorite market: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to toggle favorite market",
                "message": "An internal error occurred. Please try again later."
            })))
        }
    }
}
