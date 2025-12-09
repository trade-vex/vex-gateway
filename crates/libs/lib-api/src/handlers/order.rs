//! Order Management API Handlers

use actix_web::{HttpResponse, web};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    CorrelationService, Publisher, ResponseError, TradingRepository, generate_client_order_id,
    model::orders::Model as OrderModel, wait_for_response,
};
use lib_utils::AuthenticatedUserId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

// Request/Response DTOs

/// Request payload for placing a new order
///
/// # Fields
/// * `price` - Order price in smallest currency unit (e.g., cents, satoshis)
/// * `volume` - Order size/quantity in base asset units
/// * `market_id` - Unique identifier for the trading market/pair
/// * `side` - Order side (Bid for buy, Ask for sell)
/// * `time_in_force` - Order duration policy (GTC, IOC, FOK, etc.)
/// * `order_type` - Determines how the order is handled (`Limit` submits at the provided price,
///   `Market` ignores the price: Ask orders use 0, Bid orders use u64::MAX for vex-core processing)
///
/// # Example
/// ```json
/// {
///   "price": 5000000,
///   "volume": 100,
///   "market_id": 1,
///   "side": "Bid",
///   "time_in_force": "Gtc",
///   "order_type": "Limit"
/// }
/// ```
#[derive(Debug, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
}

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

    /// Order execution type
    pub order_type: OrderType,
}

/// Request payload for cancelling an existing order
///
/// # Fields
/// * `order_id` - Unique identifier of the order to cancel (as string to support JavaScript BigInt)
/// * `market_id` - Market identifier where the order exists
///
/// # Example
/// ```json
/// {
///   "order_id": "123456789",
///   "market_id": 1
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct CancelOrderRequest {
    /// Unique identifier of the order to cancel (string format for JavaScript BigInt support)
    pub order_id: String,

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
///     "time_in_force": "Gtc",
///     "order_type": "Limit"
///   }'
/// ```
///
/// Market orders automatically adjust the submitted price when forwarded to the
/// matching engine: Ask orders use `0`, Bid orders use `u64::MAX`.
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
    trading_repo: web::Data<Arc<dyn TradingRepository>>,
) -> HttpResponse {
    let start = Instant::now();

    // Extract user ID from JWT
    let user_id = auth_user.as_u64();
    let order_data = order_data.into_inner();

    let OrderRequest {
        price,
        volume,
        market_id,
        side,
        time_in_force,
        order_type,
    } = order_data;

    let command_price = match order_type {
        OrderType::Limit => price,
        OrderType::Market => {
            // Market orders: Ask uses 0, Bid uses u64::MAX
            match side {
                Side::Ask => 0,
                Side::Bid => u64::MAX,
            }
        }
    };

    // Generate unique client order ID for correlation
    let client_order_id = generate_client_order_id();

    // Register this request - get receiver for response
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build order command
    let command = OrderCommand {
        command: OrderCommandType::PlaceOrder,
        order_id: 0,     // Will be assigned by VEX Core
        client_order_id, // Our correlation ID
        market_id,
        user_id,
        price: command_price,
        size: volume,
        side,
        time_in_force,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
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
            let status_str = format!("{:?}", response.status);

            info!(
                client_order_id = client_order_id,
                order_id = response.order_id,
                user_id = user_id,
                status = ?response.status,
                latency_us = latency.as_micros(),
                "Order processed successfully"
            );

            // Store cancelled/rejected orders in database if they won't come via Kafka
            // This ensures FOK orders that are cancelled appear in order history
            if response.order_id != 0 && matches!(response.status, Status::Cancelled) {
                // Check if order already exists (might have been stored via Kafka)
                match trading_repo
                    .get_order_by_id(response.order_id, user_id)
                    .await
                {
                    Ok(Some(_)) => {
                        // Order already exists, skip insertion
                        debug!(
                            "Order {} already exists in database, skipping insertion",
                            response.order_id
                        );
                    }
                    Ok(None) => {
                        // Order doesn't exist, create it
                        let order_model = OrderModel {
                            id: 0, // Will be auto-generated
                            order_order_id: Decimal::from(response.order_id),
                            order_user_id: Decimal::from(user_id),
                            order_price: Decimal::from(price),
                            order_size: Decimal::from(volume),
                            filled_size: Decimal::from(0),
                            order_side: format!("{:?}", side),
                            order_timestamp: command.timestamp as i64, // Store nanoseconds as i64
                            market_id: market_id as i32,
                            time_in_force: format!("{:?}", time_in_force),
                            status: status_str.clone(),
                            created_at: None,
                        };

                        if let Err(e) = trading_repo.create_order(order_model).await {
                            warn!(
                                order_id = response.order_id,
                                error = %e,
                                "Failed to store cancelled order in database"
                            );
                        } else {
                            info!(
                                order_id = response.order_id,
                                "Stored cancelled FOK order in database for order history"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            order_id = response.order_id,
                            error = %e,
                            "Failed to check if order exists before storing"
                        );
                    }
                }
            }

            HttpResponse::Ok().json(OrderResponse {
                order_id: response.order_id,
                client_order_id: response.client_order_id,
                status: status_str,
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
///     "order_id": "123456789",
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

    let user_id = auth_user.as_u64();
    let cancel_data = cancel_data.into_inner();

    // Parse order_id from string to u64
    let order_id = match cancel_data.order_id.parse::<u64>() {
        Ok(id) => id,
        Err(e) => {
            error!(
                order_id = cancel_data.order_id,
                error = %e,
                "Invalid order_id format"
            );
            return HttpResponse::BadRequest().json(OrderResponse {
                order_id: 0,
                client_order_id: 0,
                status: "error".to_string(),
                message: "Invalid order_id: must be a valid u64 integer string".to_string(),
            });
        }
    };

    // Generate NEW client order ID for THIS cancel request
    let client_order_id = generate_client_order_id();
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build cancel command
    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id,        // The order to cancel
        client_order_id, // NEW correlation ID for this cancel request
        market_id: cancel_data.market_id,
        user_id,
        price: 0,
        size: 0,
        side: Side::Bid,
        time_in_force: TimeInForce::Gtc,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
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
            order_id,
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
                order_id,
                client_order_id,
                status: "timeout".to_string(),
                message: "Cancel timeout".to_string(),
            })
        }
        Err(ResponseError::Cancelled) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id,
            client_order_id,
            status: "error".to_string(),
            message: "Internal error".to_string(),
        }),
    }
}
