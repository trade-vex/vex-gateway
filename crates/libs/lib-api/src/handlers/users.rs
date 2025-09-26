//! lib-api/src/handlers/users.rs

use actix_web::{web, HttpResponse, Result};
use lib_utils::auth_extractors::AuthenticatedUserId;
use crate::models::trading::*;
use crate::repository::trading_repository::TradingRepository;
use std::sync::Arc;

pub async fn get_balances(
    user: AuthenticatedUserId,
    repo: web::Data<Arc<dyn TradingRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    match repo.get_user_balances(user_id).await {
        Ok(balances) => {
            let total_assets = balances.len();
            let balance_responses: Vec<UserBalanceResponse> = balances
                .into_iter()
                .map(|balance| balance.into())
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
                "message": e.to_string()
            })))
        }
    }
}

pub async fn get_open_orders(
    user: AuthenticatedUserId,
    repo: web::Data<Arc<dyn TradingRepository>>,
) -> Result<HttpResponse> {
    let raw = user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    match repo.get_open_orders(user_id).await {
        Ok(orders) => {
            let total = orders.len();
            let order_responses: Vec<OrderResponse> = orders
                .into_iter()
                .map(|order| order.into())
                .collect();

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

