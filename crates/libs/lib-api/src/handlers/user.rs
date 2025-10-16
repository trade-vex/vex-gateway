//! User Account Data API Handlers

use actix_web::{HttpResponse, Result, web};
use lib_core::{
    BalancesListResponse, OrderResponse, OrderStore, OrdersListResponse, UserBalanceResponse,
    UserBalanceStore,
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
    let user_id = user.0;

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
    let user_id = user.0;

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
