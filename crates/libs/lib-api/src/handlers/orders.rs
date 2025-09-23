//! lib-api/src/handlers/orders.rs

use actix_web::{HttpResponse, web};
use serde::{Deserialize, Serialize};
use serde_json;
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
    pub user_id: Option<u64>,
    pub side: Side,
    pub order_type: OrderType,
}

#[derive(Debug, Deserialize)]
pub struct CancelOrderRequest {
    pub order_id: u64,
    pub symbol_id: u32,
    pub user_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct OrderResponse {
    pub order_id: u64,
    pub status: String,
    pub message: String,
}

pub async fn order(req: web::Json<OrderRequest>) -> HttpResponse {
    // Extract order data from the request
    let order_data = req.into_inner();
    
    // Validate that user_id is provided for legacy endpoint
    if order_data.user_id.is_none() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "user_id is required for this endpoint"
        }));
    }
    
    process_order(order_data).await
}

/// Common order processing logic
async fn process_order(order_data: OrderRequest) -> HttpResponse {
    // Extract order data from the request
    let user_id = order_data.user_id.expect("user_id should be set at this point");
    
    // Generate a unique order ID 
    let order_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let command = OrderCommand {
        command: OrderCommandType::PlaceLimitOrder,
        order_id,
        symbol_id: order_data.symbol_id,
        user_id: user_id,
        price: order_data.price,
        reserve_bid_price: order_data.price, // Using same price as reserve bid price
        size: order_data.volume,
        side: order_data.side.clone(),
        order_type: order_data.order_type.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        matcher_event: None,
    };

    // Initialize gateway client
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

    // Create channel for receiving processed order
    let (sx, rx) = mpsc::channel();
    let handler = OrderCommandHandler::new(client.gateway_id().to_string(), sx);

    // Start the client
    match client.start(handler) {
        Ok(()) => {
            // Send the order command
            match client.send_order_command(&command) {
                Ok(()) => {
                    // Wait for the processed order response
                    match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(processed_order) => {
                            HttpResponse::Ok().json(OrderResponse {
                                order_id,
                                status: "success".to_string(),
                                message: format!("Order processed successfully: {:?}", processed_order),
                            })
                        }
                        Err(_) => {
                            HttpResponse::RequestTimeout().json(OrderResponse {
                                order_id,
                                status: "timeout".to_string(),
                                message: "Order processing timed out".to_string(),
                            })
                        }
                    }
                }
                Err(e) => {
                    HttpResponse::InternalServerError().json(OrderResponse {
                        order_id,
                        status: "error".to_string(),
                        message: format!("Failed to send order command: {}", e),
                    })
                }
            }
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(OrderResponse {
                order_id,
                status: "error".to_string(),
                message: format!("Failed to start gateway client: {}", e),
            })
        }
    }
}


pub async fn order_with_auth(
    order_data: web::Json<OrderRequest>,
    auth_user: AuthenticatedUserId,
) -> HttpResponse {
    let user_id = auth_user.0.as_u128() as u64;
    
    let mut order_data = order_data.into_inner();
    order_data.user_id = Some(user_id);

    process_order(order_data).await
}

pub async fn cancel_order(req: web::Json<CancelOrderRequest>) -> HttpResponse {
    // Extract cancel order data from the request
    let cancel_data = req.into_inner();
    
    // Validate that user_id is provided for legacy endpoint
    if cancel_data.user_id.is_none() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "user_id is required for this endpoint"
        }));
    }
    
    process_cancel_order(cancel_data).await
}

/// Common cancel order processing logic
async fn process_cancel_order(cancel_data: CancelOrderRequest) -> HttpResponse {
    // Extract cancel order data from the request
    let user_id = cancel_data.user_id.expect("user_id should be set at this point");
    
    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id: cancel_data.order_id,
        symbol_id: cancel_data.symbol_id,
        user_id: user_id,
        price: 0, // Not needed for cancel order
        reserve_bid_price: 0, // Not needed for cancel order
        size: 0, // Not needed for cancel order
        side: Side::Bid, // Not needed for cancel order, but required by struct
        order_type: OrderType::Gtc, // Not needed for cancel order, but required by struct
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        matcher_event: None,
    };

    // Initialize gateway client
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

    // Create channel for receiving processed order
    let (sx, rx) = mpsc::channel();
    let handler = OrderCommandHandler::new(client.gateway_id().to_string(), sx);

    // Start the client
    match client.start(handler) {
        Ok(()) => {
            // Send the cancel order command
            match client.send_order_command(&command) {
                Ok(()) => {
                    // Wait for the processed order response
                    match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(processed_order) => {
                            HttpResponse::Ok().json(OrderResponse {
                                order_id: cancel_data.order_id,
                                status: "success".to_string(),
                                message: format!("Order cancelled successfully: {:?}", processed_order),
                            })
                        }
                        Err(_) => {
                            HttpResponse::RequestTimeout().json(OrderResponse {
                                order_id: cancel_data.order_id,
                                status: "timeout".to_string(),
                                message: "Order cancellation timed out".to_string(),
                            })
                        }
                    }
                }
                Err(e) => {
                    HttpResponse::InternalServerError().json(OrderResponse {
                        order_id: cancel_data.order_id,
                        status: "error".to_string(),
                        message: format!("Failed to send cancel order command: {}", e),
                    })
                }
            }
        }
        Err(e) => {
            HttpResponse::InternalServerError().json(OrderResponse {
                order_id: cancel_data.order_id,
                status: "error".to_string(),
                message: format!("Failed to start gateway client: {}", e),
            })
        }
    }
}

pub async fn cancel_order_with_auth(
    cancel_data: web::Json<CancelOrderRequest>,
    auth_user: AuthenticatedUserId,
) -> HttpResponse {
    let user_id = auth_user.0.as_u128() as u64;
    
    let mut cancel_data = cancel_data.into_inner();
    cancel_data.user_id = Some(user_id);

    process_cancel_order(cancel_data).await
}