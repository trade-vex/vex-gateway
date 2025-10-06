//! lib-api/src/handlers/orders.rs

use actix_web::{HttpResponse, web};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::sync::mpsc;
use vex_networking::client::{VexGateway, OrderCommandHandler};
use vex_config::GatewayNetworkingConfig;
use common::model::enums::{Side, OrderType};
use common::cmd::{OrderCommand, OrderCommandType};
use lib_utils::AuthenticatedUserId;


#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    pub price: u64,
    pub volume: u64,
    pub symbol_id: u32,
    pub side: Side,
    pub order_type: OrderType,
}

#[derive(Debug, Deserialize)]
pub struct CancelOrderRequest {
    pub order_id: u64,
    pub symbol_id: u32,
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
) -> HttpResponse {
    // Use folded hash to preserve all UUID bits in u64
    let raw = auth_user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    
    let order_data = order_data.into_inner();

    // Inline order processing to avoid extra stack frames
    let order_id = 0;
    let command = OrderCommand {
        command: OrderCommandType::PlaceLimitOrder,
        order_id,
        symbol_id: order_data.symbol_id,
        user_id,
        price: order_data.price,
        reserve_bid_price: order_data.price,
        size: order_data.volume,
        side: order_data.side.clone(),
        order_type: order_data.order_type.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        matcher_event: None,
    };

    let client_config = GatewayNetworkingConfig::test_defaults();
    let mut client = match VexGateway::new(client_config) {
        Ok(client) => client,
        Err(e) => {
            return HttpResponse::InternalServerError().json(OrderResponse {
                order_id,
                status: "error".to_string(),
                message: format!("Failed to initialize gateway: {}", e),
            });
        }
    };

    let (sx, rx) = mpsc::channel();
    let handler = OrderCommandHandler::new(client.gateway_id().to_string(), sx);

    match client.start(handler) {
        Ok(()) => match client.send_order_command(&command) {
            Ok(()) => match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(processed_order) => HttpResponse::Ok().json(OrderResponse {
                    order_id,
                    status: "success".to_string(),
                    message: format!("Order processed successfully: {:?}", processed_order),
                }),
                Err(_) => HttpResponse::RequestTimeout().json(OrderResponse {
                    order_id,
                    status: "timeout".to_string(),
                    message: "Order processing timed out".to_string(),
                }),
            },
            Err(e) => HttpResponse::InternalServerError().json(OrderResponse {
                order_id,
                status: "error".to_string(),
                message: format!("Failed to send order command: {}", e),
            }),
        },
        Err(e) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id,
            status: "error".to_string(),
            message: format!("Failed to start gateway client: {}", e),
        }),
    }
}

pub async fn cancel_order(
    cancel_data: web::Json<CancelOrderRequest>,
    auth_user: AuthenticatedUserId,
) -> HttpResponse {
    // Use folded hash to preserve all UUID bits in u64
    let raw = auth_user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    
    let cancel_data = cancel_data.into_inner();

    // Inline cancel processing to avoid extra stack frames
    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id: cancel_data.order_id,
        symbol_id: cancel_data.symbol_id,
        user_id,
        price: 0,
        reserve_bid_price: 0,
        size: 0,
        side: Side::Bid,
        order_type: OrderType::Gtc,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        matcher_event: None,
    };

    let client_config = GatewayNetworkingConfig::test_defaults();
    let mut client = match VexGateway::new(client_config) {
        Ok(client) => client,
        Err(e) => {
            return HttpResponse::InternalServerError().json(OrderResponse {
                order_id: cancel_data.order_id,
                status: "error".to_string(),
                message: format!("Failed to initialize gateway: {}", e),
            });
        }
    };

    let (sx, rx) = mpsc::channel();
    let handler = OrderCommandHandler::new(client.gateway_id().to_string(), sx);

    match client.start(handler) {
        Ok(()) => match client.send_order_command(&command) {
            Ok(()) => match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(processed_order) => HttpResponse::Ok().json(OrderResponse {
                    order_id: cancel_data.order_id,
                    status: "success".to_string(),
                    message: format!("Order cancelled successfully: {:?}", processed_order),
                }),
                Err(_) => HttpResponse::RequestTimeout().json(OrderResponse {
                    order_id: cancel_data.order_id,
                    status: "timeout".to_string(),
                    message: "Order cancellation timed out".to_string(),
                }),
            },
            Err(e) => HttpResponse::InternalServerError().json(OrderResponse {
                order_id: cancel_data.order_id,
                status: "error".to_string(),
                message: format!("Failed to send cancel order command: {}", e),
            }),
        },
        Err(e) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id: cancel_data.order_id,
            status: "error".to_string(),
            message: format!("Failed to start gateway client: {}", e),
        }),
    }
}