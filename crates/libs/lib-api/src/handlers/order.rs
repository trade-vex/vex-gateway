//! lib-api/src/handlers/orders.rs

use actix_web::{HttpResponse, web};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::Publisher;
use lib_utils::AuthenticatedUserId;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    pub price: u64,
    pub volume: u64,
    pub market_id: u32,
    pub side: Side,
    pub time_in_force: TimeInForce,
}

#[derive(Debug, Deserialize)]
pub struct CancelOrderRequest {
    pub order_id: u64,
    pub market_id: u32,
}

#[derive(Debug, Serialize)]
pub struct OrderResponse {
    pub order_id: u64,
    pub status: String,
    pub message: String,
}

pub async fn place_order(
    order_data: web::Json<OrderRequest>,
    auth_user: AuthenticatedUserId,
    publisher: web::Data<Arc<Mutex<Publisher>>>,
) -> HttpResponse {
    // Use folded hash to preserve all UUID bits in u64
    let raw = auth_user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    let order_data = order_data.into_inner();

    // Inline order processing to avoid extra stack frames
    let order_id = 0;
    let command = OrderCommand {
        command: OrderCommandType::PlaceOrder,
        order_id,
        market_id: order_data.market_id,
        user_id,
        price: order_data.price,
        size: order_data.volume,
        side: order_data.side.clone(),
        time_in_force: order_data.time_in_force.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        status: Status::Processing,
        events: None,
        balance: [UserBalance::default(); 2],
        l2_data: None,
    };

    // Send order command using the publisher from app state
    let mut publisher = match publisher.lock() {
        Ok(guard) => guard,
        Err(e) => {
            return HttpResponse::InternalServerError().json(OrderResponse {
                order_id,
                status: "error".to_string(),
                message: format!("Failed to acquire publisher lock: {}", e),
            });
        }
    };

    match publisher.send_order_command(&command) {
        Ok(()) => HttpResponse::Ok().json(OrderResponse {
            order_id,
            status: "success".to_string(),
            message: "Order sent successfully".to_string(),
        }),
        Err(e) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id,
            status: "error".to_string(),
            message: format!("Failed to send order command: {}", e),
        }),
    }
}

pub async fn cancel_order(
    cancel_data: web::Json<CancelOrderRequest>,
    auth_user: AuthenticatedUserId,
    publisher: web::Data<Arc<Mutex<Publisher>>>,
) -> HttpResponse {
    // Use folded hash to preserve all UUID bits in u64
    let raw = auth_user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);

    let cancel_data = cancel_data.into_inner();

    // Inline cancel processing to avoid extra stack frames
    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id: cancel_data.order_id,
        market_id: cancel_data.market_id,
        user_id,
        price: 0,
        size: 0,
        side: Side::Bid, // Default side for cancel
        time_in_force: TimeInForce::Gtc,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        status: Status::Processing,
        events: None,
        balance: [UserBalance::default(); 2],
        l2_data: None,
    };

    // Send cancel order command using the publisher from app state
    let mut publisher = match publisher.lock() {
        Ok(guard) => guard,
        Err(e) => {
            return HttpResponse::InternalServerError().json(OrderResponse {
                order_id: cancel_data.order_id,
                status: "error".to_string(),
                message: format!("Failed to acquire publisher lock: {}", e),
            });
        }
    };

    match publisher.send_order_command(&command) {
        Ok(()) => HttpResponse::Ok().json(OrderResponse {
            order_id: cancel_data.order_id,
            status: "success".to_string(),
            message: "Cancel order sent successfully".to_string(),
        }),
        Err(e) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id: cancel_data.order_id,
            status: "error".to_string(),
            message: format!("Failed to send cancel order command: {}", e),
        }),
    }
}
