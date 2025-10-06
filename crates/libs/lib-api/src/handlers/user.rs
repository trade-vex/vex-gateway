//! lib-api/src/handlers/users.rs

use actix_web::{HttpResponse, Result, web};
use lib_core::{
    BalancesListResponse, OrderResponse, OrderStore, OrdersListResponse, UserBalanceResponse,
    UserBalanceStore,
};
use lib_utils::auth_extractors::AuthenticatedUserId;
use std::sync::Arc;

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
