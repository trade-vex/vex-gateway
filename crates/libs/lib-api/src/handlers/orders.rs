//! lib-api/src/handlers/orders.rs

use actix_web::{HttpRequest, HttpResponse, web, HttpMessage};
use serde::{Deserialize, Serialize};
use serde_json;
use std::sync::Arc;
use std::time::Duration;
use std::sync::mpsc;
use vex_networking::client::{VexGateway, OrderCommandHandler};
use futures_util::future::{ready, Ready};
use vex_config::GatewayNetworkingConfig;
use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::api_key::security::ApiKeyConfig;
use common::model::enums::{Side, OrderType};
use common::cmd::{OrderCommand, OrderCommandType};

#[derive(Debug, Clone)]
pub enum AuthContext {
    Jwt(AuthenticatedUser),
    ApiKey(Arc<ApiKeyConfig>),
}

impl actix_web::FromRequest for AuthContext {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &actix_web::HttpRequest, _: &mut actix_web::dev::Payload) -> Self::Future {
        if let Some(jwt_user) = req.extensions().get::<AuthenticatedUser>() {
            return ready(Ok(AuthContext::Jwt(jwt_user.clone())));
        }
        
        if let Some(api_config) = req.extensions().get::<Arc<ApiKeyConfig>>() {
            return ready(Ok(AuthContext::ApiKey(api_config.clone())));
        }
        
        ready(Err(actix_web::error::ErrorUnauthorized("Authentication required")))
    }
}

#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    pub price: u64,
    pub volume: u64,
    pub symbol_id: u32,
    pub user_id: Option<u64>,
    pub side: Side,
    pub order_type: OrderType,
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
    req: HttpRequest,
    order_data: web::Json<OrderRequest>,
) -> HttpResponse {
    let user_id = if let Some(jwt_user) = req.extensions().get::<AuthenticatedUser>() {
        jwt_user.user_id.as_u128() as u64
    } else if let Some(_api_config) = req.extensions().get::<Arc<ApiKeyConfig>>() {
        order_data.user_id.unwrap_or_else(|| 0)
    } else {
        return HttpResponse::Unauthorized().json(serde_json::json!({
            "error": "Authentication required"
        }));
    };

    let mut order_data = order_data.into_inner();
    order_data.user_id = Some(user_id);

    process_order(order_data).await
}

pub async fn order_with_auth_context(
    auth_context: AuthContext,
    order_data: web::Json<OrderRequest>,
) -> HttpResponse {
    let user_id = match auth_context {
        AuthContext::Jwt(user) => user.user_id.as_u128() as u64,
        AuthContext::ApiKey(_api_config) => {
            match order_data.user_id {
                Some(id) => id,
                None => {
                    return HttpResponse::BadRequest().json(serde_json::json!({
                        "error": "user_id is required for API key requests"
                    }));
                }
            }
        }
    };

    let mut order_data = order_data.into_inner();
    order_data.user_id = Some(user_id);

    process_order(order_data).await
}