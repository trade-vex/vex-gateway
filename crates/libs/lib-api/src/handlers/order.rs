//! Order Management API Handlers

use actix_web::{HttpResponse, web};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    CorrelationService, Publisher, ResponseError, generate_client_order_id, wait_for_response,
};
use lib_utils::AuthenticatedUserId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

// Request/Response DTOs

/// Request payload for placing a new order
///
/// # Fields
/// * `price` - Order price in smallest currency unit (e.g., cents, satoshis)
/// * `volume` - Order size/quantity in base asset units
/// * `market_id` - Unique identifier for the trading market/pair
/// * `side` - Order side (Bid for buy, Ask for sell)
/// * `time_in_force` - Order duration policy (GTC, IOC, FOK, etc.)
///
/// # Example
/// ```json
/// {
///   "price": 5000000,
///   "volume": 100,
///   "market_id": 1,
///   "side": "Bid",
///   "time_in_force": "Gtc"
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    /// Order price in smallest currency unit
    pub price: u64,

    /// Order volume/size in base asset units
    pub volume: u64,

    /// Market identifier (trading pair)
    pub market_id: u32,

    /// Order side: Bid (buy) or Ask (sell)
    pub side: Side,

    /// Time in force policy for the order
    pub time_in_force: TimeInForce,
}

/// Request payload for cancelling an existing order
///
/// # Fields
/// * `order_id` - Unique identifier of the order to cancel
/// * `market_id` - Market identifier where the order exists
///
/// # Example
/// ```json
/// {
///   "order_id": 123456789,
///   "market_id": 1
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct CancelOrderRequest {
    /// Unique identifier of the order to cancel
    pub order_id: u64,

    /// Market identifier for validation
    pub market_id: u32,
}

/// Response payload for order operations
///
/// Returns the status and details of an order placement or cancellation request.
///
/// # Fields
/// * `order_id` - Unique order identifier assigned by the matching engine (0 if failed)
/// * `client_order_id` - Client-side correlation ID for request tracking
/// * `status` - Order status (Processing, Filled, Cancelled, Rejected, etc.)
/// * `message` - Human-readable status message
///
/// # Example Success Response
/// ```json
/// {
///   "order_id": 123456789,
///   "client_order_id": 987654321,
///   "status": "Processing",
///   "message": "Order processed successfully"
/// }
/// ```
///
/// # Example Error Response
/// ```json
/// {
///   "order_id": 0,
///   "client_order_id": 987654321,
///   "status": "error",
///   "message": "Insufficient balance"
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct OrderResponse {
    /// Order ID assigned by the matching engine
    pub order_id: u64,

    /// Client order ID for correlation tracking
    pub client_order_id: u64,

    /// Order status string
    pub status: String,

    /// Human-readable message describing the result
    pub message: String,
}

// API Handlers

/// Place a new order in the market
///
/// Submits a new order to the VEX Core matching engine for processing.
/// The order is validated, sent asynchronously, and tracked using a correlation ID.
///
/// # Endpoint
/// `POST /api/order/order`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Request Body
/// See [`OrderRequest`] for request payload structure.
///
/// # Response
/// Returns [`OrderResponse`] with order details and status.
///
/// # Example Request
/// ```bash
/// curl -X POST http://localhost:8080/api/order/order \
///   -H "Authorization: Bearer <jwt_token>" \
///   -H "Content-Type: application/json" \
///   -d '{
///     "price": 5000000,
///     "volume": 100,
///     "market_id": 1,
///     "side": "Bid",
///     "time_in_force": "Gtc"
///   }'
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "order_id": 123456789,
///   "client_order_id": 987654321,
///   "status": "Processing",
///   "message": "Order processed successfully"
/// }
/// ```
///
/// # Example Timeout Response (408)
/// ```json
/// {
///   "order_id": 0,
///   "client_order_id": 987654321,
///   "status": "timeout",
///   "message": "Order processing timeout"
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "order_id": 0,
///   "client_order_id": 987654321,
///   "status": "error",
///   "message": "Failed to send order: connection refused"
/// }
/// ```
pub async fn place_order(
    order_data: web::Json<OrderRequest>,
    auth_user: AuthenticatedUserId,
    publisher: web::Data<Arc<tokio::sync::Mutex<Publisher>>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let start = Instant::now();

    // Extract user ID from JWT
    let raw = auth_user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let order_data = order_data.into_inner();

    // Generate unique client order ID for correlation
    let client_order_id = generate_client_order_id();

    // Register this request - get receiver for response
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build order command
    let command = OrderCommand {
        command: OrderCommandType::PlaceOrder,
        order_id: 0,     // Will be assigned by VEX Core
        client_order_id, // Our correlation ID
        market_id: order_data.market_id,
        user_id,
        price: order_data.price,
        size: order_data.volume,
        side: order_data.side,
        time_in_force: order_data.time_in_force,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64,
        status: Status::Processing,
        events: None,
        balance: [UserBalance::default(); 2],
        l2_data: None,
    };

    // Send to VEX Core
    let send_result = {
        let publisher = publisher.lock().await;
        publisher.send_order_command(&command)
    };

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        error!(
            client_order_id = client_order_id,
            user_id = user_id,
            error = %e,
            "Failed to send order to VEX Core"
        );
        return HttpResponse::InternalServerError().json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "error".to_string(),
            message: format!("Failed to send order: {}", e),
        });
    }

    // Wait for response with timeout - function pauses here
    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => {
            let latency = start.elapsed();

            info!(
                client_order_id = client_order_id,
                order_id = response.order_id,
                user_id = user_id,
                status = ?response.status,
                latency_us = latency.as_micros(),
                "Order processed successfully"
            );

            HttpResponse::Ok().json(OrderResponse {
                order_id: response.order_id,
                client_order_id: response.client_order_id,
                status: format!("{:?}", response.status),
                message: "Order processed successfully".to_string(),
            })
        }
        Err(ResponseError::Timeout) => {
            correlation_service.cancel_request(client_order_id);

            warn!(
                client_order_id = client_order_id,
                user_id = user_id,
                "Order processing timeout"
            );

            HttpResponse::RequestTimeout().json(OrderResponse {
                order_id: 0,
                client_order_id,
                status: "timeout".to_string(),
                message: "Order processing timeout".to_string(),
            })
        }
        Err(ResponseError::Cancelled) => {
            error!(
                client_order_id = client_order_id,
                "Response channel cancelled"
            );

            HttpResponse::InternalServerError().json(OrderResponse {
                order_id: 0,
                client_order_id,
                status: "error".to_string(),
                message: "Internal error".to_string(),
            })
        }
    }
}

/// Cancel an existing order
///
/// Submits a cancellation request for an existing order in the market.
/// The cancellation is processed asynchronously with correlation tracking.
///
/// # Endpoint
/// `POST /api/order/cancel`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Request Body
/// See [`CancelOrderRequest`] for request payload structure.
///
/// # Response
/// Returns [`OrderResponse`] with cancellation status.
///
/// # Example Request
/// ```bash
/// curl -X POST http://localhost:8080/api/order/cancel \
///   -H "Authorization: Bearer <jwt_token>" \
///   -H "Content-Type: application/json" \
///   -d '{
///     "order_id": 123456789,
///     "market_id": 1
///   }'
/// ```
///
/// # Example Success Response (200 OK)
/// ```json
/// {
///   "order_id": 123456789,
///   "client_order_id": 987654321,
///   "status": "Cancelled",
///   "message": "Cancel processed successfully"
/// }
/// ```
///
/// # Example Timeout Response (408)
/// ```json
/// {
///   "order_id": 123456789,
///   "client_order_id": 987654321,
///   "status": "timeout",
///   "message": "Cancel timeout"
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "order_id": 123456789,
///   "client_order_id": 987654321,
///   "status": "error",
///   "message": "Failed to send cancel: connection refused"
/// }
/// ```
pub async fn cancel_order(
    cancel_data: web::Json<CancelOrderRequest>,
    auth_user: AuthenticatedUserId,
    publisher: web::Data<Arc<tokio::sync::Mutex<Publisher>>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let start = Instant::now();

    let raw = auth_user.0.as_u128();
    let user_id = ((raw >> 64) as u64) ^ (raw as u64);
    let cancel_data = cancel_data.into_inner();

    // Generate NEW client order ID for THIS cancel request
    let client_order_id = generate_client_order_id();
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build cancel command
    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id: cancel_data.order_id, // The order to cancel
        client_order_id,                // NEW correlation ID for this cancel request
        market_id: cancel_data.market_id,
        user_id,
        price: 0,
        size: 0,
        side: Side::Bid,
        time_in_force: TimeInForce::Gtc,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64,
        status: Status::Processing,
        events: None,
        balance: [UserBalance::default(); 2],
        l2_data: None,
    };

    // Send cancel command
    let send_result = {
        let publisher = publisher.lock().await;
        publisher.send_order_command(&command)
    };

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        return HttpResponse::InternalServerError().json(OrderResponse {
            order_id: cancel_data.order_id,
            client_order_id,
            status: "error".to_string(),
            message: format!("Failed to send cancel: {}", e),
        });
    }

    // Wait for cancel confirmation
    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => {
            let latency = start.elapsed();

            info!(
                client_order_id = client_order_id,
                order_id = response.order_id,
                user_id = user_id,
                latency_us = latency.as_micros(),
                "Cancel processed successfully"
            );

            HttpResponse::Ok().json(OrderResponse {
                order_id: response.order_id,
                client_order_id: response.client_order_id,
                status: format!("{:?}", response.status),
                message: "Cancel processed successfully".to_string(),
            })
        }
        Err(ResponseError::Timeout) => {
            correlation_service.cancel_request(client_order_id);
            HttpResponse::RequestTimeout().json(OrderResponse {
                order_id: cancel_data.order_id,
                client_order_id,
                status: "timeout".to_string(),
                message: "Cancel timeout".to_string(),
            })
        }
        Err(ResponseError::Cancelled) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id: cancel_data.order_id,
            client_order_id,
            status: "error".to_string(),
            message: "Internal error".to_string(),
        }),
    }
}
