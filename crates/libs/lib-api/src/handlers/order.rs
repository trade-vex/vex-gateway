//! Order Management API Handlers

use actix_web::{HttpRequest, HttpResponse, http::StatusCode, web};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    CompletionWait, CorrelationService, EngineLink, IdempotentOrderOutcome, OrderClaim,
    OrderIdempotencyError, OrderIdempotencyStore, ReleaseReason, ResponseError,
    generate_client_order_id, wait_for_response,
};
use lib_utils::{AuthenticatedUserId, uuid_to_u64};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};
use vex_networking::client::GatewayError;

const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";
const MAX_IDEMPOTENCY_KEY_LENGTH: usize = 128;

fn unix_timestamp_nanos(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64
}

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
#[derive(Clone, Copy, Debug, Deserialize)]
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

fn validate_order_inputs(
    price: u64,
    volume: u64,
    time_in_force: TimeInForce,
    order_type: &OrderType,
) -> Result<(), String> {
    if volume == 0 {
        return Err("order volume must be greater than zero".to_string());
    }

    if matches!(order_type, OrderType::Limit) && (price == 0 || price == u64::MAX) {
        return Err("limit order price must not use market sentinel values".to_string());
    }

    if matches!(order_type, OrderType::Market) && time_in_force == TimeInForce::Gtc {
        return Err("market order time_in_force must be IOC or FOK".to_string());
    }

    Ok(())
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
/// * `order_id` - Unique order identifier assigned by the matching engine ("0" if failed)
/// * `client_order_id` - Client-side correlation ID for request tracking
/// * `status` - Order status (Processing, Filled, Cancelled, Rejected, etc.)
/// * `message` - Human-readable status message
///
/// Note: `order_id` and `client_order_id` are serialized as strings to support JavaScript BigInt.
///
/// # Example Success Response
/// ```json
/// {
///   "order_id": "123456789",
///   "client_order_id": "987654321",
///   "status": "Processing",
///   "message": "Order processed successfully"
/// }
/// ```
///
/// # Example Error Response
/// ```json
/// {
///   "order_id": "0",
///   "client_order_id": "987654321",
///   "status": "error",
///   "message": "Insufficient balance"
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct OrderResponse {
    /// Order ID assigned by the matching engine (serialized as string for JavaScript BigInt support)
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub order_id: u64,

    /// Client order ID for correlation tracking (serialized as string for JavaScript BigInt support)
    #[serde(serialize_with = "serialize_u64_as_string")]
    pub client_order_id: u64,

    /// Order status string
    pub status: String,

    /// Human-readable message describing the result
    pub message: String,
}

const ENGINE_BUSY_MESSAGE: &str = "Matching engine is busy; please retry";
const ENGINE_BUSY_RETRY_AFTER_SECS: u32 = 1;

/// The one answer for a busy engine, whether the publish was back-pressured
/// (`GatewayError::Busy`) or the engine's ingress ring was full (`Status::Busy`): nothing was
/// executed, so the client may resend the same request after `Retry-After`.
fn engine_busy_response(order_id: u64, client_order_id: u64) -> HttpResponse {
    HttpResponse::ServiceUnavailable()
        .insert_header((
            actix_web::http::header::RETRY_AFTER,
            ENGINE_BUSY_RETRY_AFTER_SECS,
        ))
        .json(OrderResponse {
            order_id,
            client_order_id,
            status: "error".to_string(),
            message: ENGINE_BUSY_MESSAGE.to_string(),
        })
}

const ENGINE_UNAVAILABLE_MESSAGE: &str = "Matching engine unavailable";
/// The gateway reconnects to the engine on its own: an attempt waiting for the engine connects as
/// soon as it is back, and failed attempts are at most 10 s apart.
const ENGINE_UNAVAILABLE_RETRY_AFTER_SECS: u32 = 5;

/// The one answer while there is no link to the engine (it is down or restarting, and the gateway
/// is reconnecting): the request was not executed, or it is a cancel, which is safe to send
/// again, so the client may resend it after `Retry-After`.
fn engine_unavailable_response(order_id: u64, client_order_id: u64) -> HttpResponse {
    HttpResponse::ServiceUnavailable()
        .insert_header((
            actix_web::http::header::RETRY_AFTER,
            ENGINE_UNAVAILABLE_RETRY_AFTER_SECS,
        ))
        .json(OrderResponse {
            order_id,
            client_order_id,
            status: "error".to_string(),
            message: ENGINE_UNAVAILABLE_MESSAGE.to_string(),
        })
}

/// Answers a publish that failed, so nothing reached the engine: busy or unavailable, both
/// retryable, or a 500 for any other error, which means this gateway's own publication is
/// unusable.
fn send_failure_response(
    error: &GatewayError,
    request: &str,
    order_id: u64,
    client_order_id: u64,
) -> HttpResponse {
    match error {
        GatewayError::Busy => engine_busy_response(order_id, client_order_id),
        GatewayError::NotConnected => engine_unavailable_response(order_id, client_order_id),
        _ => HttpResponse::InternalServerError().json(OrderResponse {
            order_id,
            client_order_id,
            status: "error".to_string(),
            message: format!("Failed to send {request}: {error}"),
        }),
    }
}

/// Why a failed publish gives its idempotency claim back. A request waiting on the claim then gets
/// the 503 for that reason: busy, or engine unavailable (also when this gateway's own publication
/// failed, which the failed request itself was told with a 500).
fn release_reason(error: &GatewayError) -> ReleaseReason {
    match error {
        GatewayError::Busy => ReleaseReason::Busy,
        _ => ReleaseReason::EngineUnavailable,
    }
}

/// The client request an engine answer belongs to; only the wording of the answer differs.
#[derive(Clone, Copy)]
enum EngineRequest {
    Order,
    Cancel,
}

/// The one mapping from the engine's answer to the HTTP status and message that the client sees,
/// the gateway logs and the idempotency record replays. The match is exhaustive on purpose: a
/// status added in vex-core must be classified here before the gateway compiles, instead of
/// silently becoming a 200. A `Busy` answer goes out through `engine_busy_response`, with
/// `Retry-After`, and is never recorded.
fn engine_outcome(request: EngineRequest, status: Status) -> (StatusCode, &'static str) {
    match status {
        Status::Placed
        | Status::Cancelled
        | Status::PartiallyFilled
        | Status::Filled
        | Status::Processing
        | Status::Processed => (
            StatusCode::OK,
            match request {
                EngineRequest::Order => "Order processed successfully",
                EngineRequest::Cancel => "Cancel processed successfully",
            },
        ),
        Status::Rejected => (
            StatusCode::UNPROCESSABLE_ENTITY,
            match request {
                EngineRequest::Order => "Order rejected by the matching engine",
                EngineRequest::Cancel => {
                    "Cancel rejected: the order is not open or not owned by the caller"
                }
            },
        ),
        Status::Busy => (StatusCode::SERVICE_UNAVAILABLE, ENGINE_BUSY_MESSAGE),
    }
}

/// Answers a cancel from the engine's reply: the retryable 503 for a busy engine, otherwise what
/// `engine_outcome` says.
fn cancel_response(order_id: u64, client_order_id: u64, response: &OrderCommand) -> HttpResponse {
    if response.status == Status::Busy {
        return engine_busy_response(order_id, client_order_id);
    }
    let (http_status, message) = engine_outcome(EngineRequest::Cancel, response.status);
    HttpResponse::build(http_status).json(OrderResponse {
        order_id: response.order_id,
        client_order_id: response.client_order_id,
        status: format!("{:?}", response.status),
        message: message.to_string(),
    })
}

fn serialize_u64_as_string<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn response_from_outcome(outcome: IdempotentOrderOutcome) -> HttpResponse {
    let status =
        StatusCode::from_u16(outcome.http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    HttpResponse::build(status).json(OrderResponse {
        order_id: outcome.order_id,
        client_order_id: outcome.client_order_id,
        status: outcome.status,
        message: outcome.message,
    })
}

/// Client guidance for every 408 `outcome_unknown`. It must never invite a retry of the key: a
/// retry can only replay this same 408 for the claim's lifetime, and if the claim was lost (Redis
/// eviction or failover) the retry is accepted as new and places a second order.
fn outcome_unknown_message(summary: &str, client_order_id: u64) -> String {
    format!(
        "{summary} Retain client_order_id {client_order_id} and check open orders or order \
         history after asynchronous persistence catches up. Do not retry this idempotency key."
    )
}

fn record_idempotent_completion(
    result: Result<bool, OrderIdempotencyError>,
    client_order_id: u64,
    idempotency_key: &str,
) {
    match result {
        Ok(true) => {}
        Ok(false) => error!(
            client_order_id,
            %idempotency_key,
            "idempotency claim vanished before completion could be recorded — retry-safety guarantee no longer holds"
        ),
        Err(e) => error!(
            client_order_id,
            %idempotency_key,
            error = %e,
            "failed to record idempotent completion"
        ),
    }
}

async fn complete_idempotent_order(
    store: &OrderIdempotencyStore,
    user_id: u64,
    idempotency_key: &str,
    fingerprint: &str,
    outcome: IdempotentOrderOutcome,
) -> HttpResponse {
    let result = store
        .complete(user_id, idempotency_key, fingerprint, outcome.clone())
        .await;
    record_idempotent_completion(result, outcome.client_order_id, idempotency_key);
    response_from_outcome(outcome)
}

/// Releases the claim of an order the engine never executed, so a retry with the same key is
/// published again instead of replaying a transient failure for the claim's lifetime. A request
/// already waiting on the claim sees the release and its `reason`, and gets the same answer.
async fn release_idempotency_claim(
    store: &OrderIdempotencyStore,
    user_id: u64,
    idempotency_key: &str,
    fingerprint: &str,
    client_order_id: u64,
    reason: ReleaseReason,
) {
    match store
        .release(
            user_id,
            idempotency_key,
            fingerprint,
            client_order_id,
            reason,
        )
        .await
    {
        Ok(true) => {}
        // The key no longer holds this attempt's claim (expired, evicted, or never ours), so a
        // retry may find it free or still in flight; there is nothing left to release.
        Ok(false) => warn!(
            client_order_id,
            %idempotency_key,
            "idempotency claim was already gone when releasing an order the engine did not execute"
        ),
        Err(error) => error!(
            client_order_id,
            error = %error,
            "Failed to release idempotency claim for an order the engine did not execute"
        ),
    }
}

/// Answers a request whose key is held by another in-flight attempt by waiting for that attempt's
/// outcome. An attempt the engine turned away releases its claim, and the waiter gets the 503 that
/// attempt got: busy, with `Retry-After`, or engine unavailable (a retry claims the key afresh). A
/// claim that vanished without a release may have been published, so that waiter is told not to
/// retry the key.
async fn await_in_flight_order(
    store: &OrderIdempotencyStore,
    user_id: u64,
    idempotency_key: &str,
    fingerprint: &str,
    client_order_id: u64,
) -> HttpResponse {
    match store
        .wait_for_completion(
            user_id,
            idempotency_key,
            fingerprint,
            Duration::from_secs(5),
        )
        .await
    {
        Ok(CompletionWait::Completed(outcome)) => response_from_outcome(outcome),
        Ok(CompletionWait::Released(ReleaseReason::Busy)) => {
            engine_busy_response(0, client_order_id)
        }
        Ok(CompletionWait::Released(ReleaseReason::EngineUnavailable)) => {
            engine_unavailable_response(0, client_order_id)
        }
        Ok(CompletionWait::Unresolved) => HttpResponse::RequestTimeout().json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "outcome_unknown".to_string(),
            message: outcome_unknown_message(
                "Original order is still unresolved.",
                client_order_id,
            ),
        }),
        Err(error) => {
            error!(
                client_order_id,
                error = %error,
                "Failed while waiting for idempotent order outcome"
            );
            HttpResponse::ServiceUnavailable().json(OrderResponse {
                order_id: 0,
                client_order_id,
                status: "error".to_string(),
                message: "Unable to retrieve the original order outcome".to_string(),
            })
        }
    }
}

/// Settles the claim once the engine has answered. A busy engine executed nothing, so its answer
/// is not recorded: the claim is released, as for an unsent order, and a retry with the same key
/// is published again. Any other answer is final and is recorded, so a retry replays it.
async fn settle_idempotent_order(
    store: &OrderIdempotencyStore,
    user_id: u64,
    idempotency_key: &str,
    fingerprint: &str,
    engine_status: Status,
    outcome: IdempotentOrderOutcome,
) -> HttpResponse {
    if engine_status == Status::Busy {
        release_idempotency_claim(
            store,
            user_id,
            idempotency_key,
            fingerprint,
            outcome.client_order_id,
            ReleaseReason::Busy,
        )
        .await;
        return engine_busy_response(0, outcome.client_order_id);
    }
    complete_idempotent_order(store, user_id, idempotency_key, fingerprint, outcome).await
}

/// The answer to an order sent on a link that was lost before the engine answered. The engine may
/// have executed it, so it is a timeout's answer, given at once: a 503 would tell the client that
/// nothing was executed.
fn link_lost_outcome(client_order_id: u64) -> IdempotentOrderOutcome {
    IdempotentOrderOutcome {
        http_status: StatusCode::REQUEST_TIMEOUT.as_u16(),
        order_id: 0,
        client_order_id,
        status: "outcome_unknown".to_string(),
        message: outcome_unknown_message(
            "The link to the matching engine was lost after the order was sent, so its outcome \
             is unknown.",
            client_order_id,
        ),
    }
}

fn parse_order_id(order_id: &str) -> Result<u64, std::num::ParseIntError> {
    order_id.parse()
}

// API Handlers

/// Place a new order in the market
///
/// Submits a new order to the VEX Core matching engine for processing.
/// The market ID is checked for the database storage range, then the order is sent
/// asynchronously and tracked using a correlation ID.
///
/// # Endpoint
/// `POST /api/order`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Request Body
/// See [`OrderRequest`] for request payload structure.
///
/// # Response
/// Returns [`OrderResponse`] with order details and status.
/// A matching-engine `Rejected` status is returned as HTTP 422 Unprocessable Entity, and a `Busy`
/// one (the engine was too busy to accept the request) as 503 with `Retry-After`. While there is
/// no link to the engine, the order is not sent and the answer is 503 `Matching engine
/// unavailable` with `Retry-After`. An order whose link is lost after it was sent gets the 408
/// `outcome_unknown` of a timeout at once, recorded under its idempotency key.
///
/// # Example Request
/// ```bash
/// curl -X POST http://localhost:8081/api/order \
///   -H "Authorization: Bearer <jwt_token>" \
///   -H "Content-Type: application/json" \
///   -H "Idempotency-Key: <unique-logical-order-key>" \
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
///   "order_id": "123456789",
///   "client_order_id": "987654321",
///   "status": "Processing",
///   "message": "Order processed successfully"
/// }
/// ```
///
/// # Example Timeout Response (408)
/// ```json
/// {
///   "order_id": "0",
///   "client_order_id": "987654321",
///   "status": "outcome_unknown",
///   "message": "Order was sent but its outcome is unknown. Retain client_order_id 987654321 and check open orders or order history after asynchronous persistence catches up. Do not retry this idempotency key."
/// }
/// ```
///
/// # Example Error Response (503)
/// ```json
/// {
///   "order_id": "0",
///   "client_order_id": "987654321",
///   "status": "error",
///   "message": "Matching engine is busy; please retry"
/// }
/// ```
pub async fn place_order(
    request: HttpRequest,
    order_data: web::Json<OrderRequest>,
    auth_user: AuthenticatedUserId,
    engine_link: web::Data<EngineLink>,
    correlation_service: web::Data<Arc<CorrelationService>>,
    idempotency_store: web::Data<Arc<OrderIdempotencyStore>>,
) -> HttpResponse {
    let start = Instant::now();

    // Extract user ID from JWT
    let user_id = uuid_to_u64(auth_user.0);
    let idempotency_key = match request
        .headers()
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    {
        Some(key)
            if !key.is_empty()
                && key.len() <= MAX_IDEMPOTENCY_KEY_LENGTH
                && !key.chars().any(char::is_control) =>
        {
            key.to_owned()
        }
        _ => {
            return HttpResponse::BadRequest().json(OrderResponse {
                order_id: 0,
                client_order_id: 0,
                status: "rejected".to_string(),
                message: format!(
                    "A non-empty {IDEMPOTENCY_KEY_HEADER} header of at most \
                     {MAX_IDEMPOTENCY_KEY_LENGTH} characters is required"
                ),
            });
        }
    };
    let order_data = order_data.into_inner();

    let OrderRequest {
        price,
        volume,
        market_id,
        side,
        time_in_force,
        order_type,
    } = order_data;

    // Validate market_id fits in i32 (database storage type)
    if market_id > i32::MAX as u32 {
        return HttpResponse::BadRequest().json(OrderResponse {
            order_id: 0,
            client_order_id: 0,
            status: "rejected".to_string(),
            message: format!(
                "market_id {} exceeds maximum allowed value {}",
                market_id,
                i32::MAX
            ),
        });
    }

    if let Err(message) = validate_order_inputs(price, volume, time_in_force, &order_type) {
        return HttpResponse::BadRequest().json(OrderResponse {
            order_id: 0,
            client_order_id: 0,
            status: "rejected".to_string(),
            message,
        });
    }

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

    let fingerprint =
        format!("{price}:{volume}:{market_id}:{side:?}:{time_in_force:?}:{order_type:?}");

    // Generate unique client order ID for correlation
    let client_order_id = generate_client_order_id();

    match idempotency_store
        .claim(user_id, &idempotency_key, &fingerprint, client_order_id)
        .await
    {
        Ok(OrderClaim::Acquired) => {}
        Ok(OrderClaim::Completed(outcome)) => return response_from_outcome(outcome),
        Ok(OrderClaim::InFlight { client_order_id }) => {
            return await_in_flight_order(
                &idempotency_store,
                user_id,
                &idempotency_key,
                &fingerprint,
                client_order_id,
            )
            .await;
        }
        Ok(OrderClaim::PayloadMismatch) => {
            return HttpResponse::Conflict().json(OrderResponse {
                order_id: 0,
                client_order_id: 0,
                status: "rejected".to_string(),
                message: format!(
                    "{IDEMPOTENCY_KEY_HEADER} was already used with a different order payload"
                ),
            });
        }
        Err(error) => {
            error!(error = %error, "Failed to claim order idempotency key");
            return HttpResponse::ServiceUnavailable().json(OrderResponse {
                order_id: 0,
                client_order_id: 0,
                status: "error".to_string(),
                message: "Order was not sent because idempotency storage is unavailable"
                    .to_string(),
            });
        }
    }

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
        original_size: volume,
        side,
        time_in_force,
        timestamp: unix_timestamp_nanos(SystemTime::now()),
        status: Status::Processing,
        events: None,
        balance: [UserBalance::default(); 2],
        l2_data: None,
        route_gateway_id: 0,
    };

    // Send to VEX Core
    let send_result = engine_link.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        error!(
            client_order_id = client_order_id,
            user_id = user_id,
            error = %e,
            "Failed to send order to VEX Core"
        );
        release_idempotency_claim(
            &idempotency_store,
            user_id,
            &idempotency_key,
            &fingerprint,
            client_order_id,
            release_reason(&e),
        )
        .await;
        // PR #92 distinguishes engine back-pressure (503) from a hard send failure (500);
        // PR #105's unconditional 503 would mislabel the latter.
        return send_failure_response(&e, "order", 0, client_order_id);
    }

    // Wait for response with timeout - function pauses here
    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => {
            let latency = start.elapsed();
            let status_str = format!("{:?}", response.status);
            let (http_status, message) = engine_outcome(EngineRequest::Order, response.status);

            if http_status.is_success() {
                info!(
                    client_order_id = client_order_id,
                    order_id = response.order_id,
                    user_id = user_id,
                    status = ?response.status,
                    latency_us = latency.as_micros(),
                    "{message}"
                );
            } else {
                warn!(
                    client_order_id = client_order_id,
                    order_id = response.order_id,
                    user_id = user_id,
                    status = ?response.status,
                    latency_us = latency.as_micros(),
                    "{message}"
                );
            }

            // PR #83 removed the gateway-side cancelled-order insert: order history is
            // written by the Kafka consumer, and the duplicate writer used a status
            // vocabulary no read query matched.
            // PR #81: an engine rejection must not be a 200. PR #105 records the outcome
            // for idempotent replays, so it has to record the SAME status the client sees.
            settle_idempotent_order(
                &idempotency_store,
                user_id,
                &idempotency_key,
                &fingerprint,
                response.status,
                IdempotentOrderOutcome {
                    http_status: http_status.as_u16(),
                    order_id: response.order_id,
                    client_order_id: response.client_order_id,
                    status: status_str,
                    message: message.to_string(),
                },
            )
            .await
        }
        Err(ResponseError::Timeout) => {
            correlation_service.timeout_request(client_order_id);

            warn!(
                client_order_id = client_order_id,
                user_id = user_id,
                "Order processing timeout"
            );

            complete_idempotent_order(
                &idempotency_store,
                user_id,
                &idempotency_key,
                &fingerprint,
                IdempotentOrderOutcome {
                    http_status: StatusCode::REQUEST_TIMEOUT.as_u16(),
                    order_id: 0,
                    client_order_id,
                    status: "outcome_unknown".to_string(),
                    message: outcome_unknown_message(
                        "Order was sent but its outcome is unknown.",
                        client_order_id,
                    ),
                },
            )
            .await
        }
        Err(ResponseError::LinkLost) => {
            warn!(
                client_order_id = client_order_id,
                user_id = user_id,
                "Engine link lost before the order's response"
            );

            // The order was sent, so the engine may have executed it: the outcome is recorded
            // like a timeout's instead of the claim being released, so a retry with this key
            // cannot place it twice.
            complete_idempotent_order(
                &idempotency_store,
                user_id,
                &idempotency_key,
                &fingerprint,
                link_lost_outcome(client_order_id),
            )
            .await
        }
    }
}

/// Cancel an existing order
///
/// Submits a cancellation request for an existing order in the market.
/// The cancellation is processed asynchronously with correlation tracking.
///
/// # Endpoint
/// `POST /api/cancel`
///
/// # Authentication
/// Requires valid JWT token in Authorization header: `Bearer <token>`
///
/// # Request Body
/// See [`CancelOrderRequest`] for request payload structure.
///
/// # Response
/// Returns [`OrderResponse`] with cancellation status.
/// A matching-engine `Rejected` status is returned as HTTP 422 Unprocessable Entity, and a `Busy`
/// one (the engine was too busy to accept the request) as 503 with `Retry-After`. While there is
/// no link to the engine, or when it is lost before the engine answers, the answer is 503
/// `Matching engine unavailable` with `Retry-After`. A lost link may have delivered the cancel,
/// but sending it again is safe: it is rejected (422) once the order is no longer open.
///
/// # Example Request
/// ```bash
/// curl -X POST http://localhost:8081/api/cancel \
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
///   "order_id": "123456789",
///   "client_order_id": "987654321",
///   "status": "Cancelled",
///   "message": "Cancel processed successfully"
/// }
/// ```
///
/// # Example Timeout Response (408)
/// ```json
/// {
///   "order_id": "123456789",
///   "client_order_id": "987654321",
///   "status": "timeout",
///   "message": "Cancel timeout"
/// }
/// ```
///
/// # Example Error Response (500)
/// ```json
/// {
///   "order_id": "123456789",
///   "client_order_id": "987654321",
///   "status": "error",
///   "message": "Failed to send cancel: connection refused"
/// }
/// ```
pub async fn cancel_order(
    cancel_data: web::Json<CancelOrderRequest>,
    auth_user: AuthenticatedUserId,
    engine_link: web::Data<EngineLink>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let start = Instant::now();

    let user_id = uuid_to_u64(auth_user.0);
    let cancel_data = cancel_data.into_inner();

    // Parse order_id from string to u64
    let order_id = match parse_order_id(&cancel_data.order_id) {
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
        original_size: 0,
        side: Side::Bid,
        time_in_force: TimeInForce::Gtc,
        timestamp: unix_timestamp_nanos(SystemTime::now()),
        status: Status::Processing,
        events: None,
        balance: [UserBalance::default(); 2],
        l2_data: None,
        route_gateway_id: 0,
    };

    // Send cancel command
    let send_result = engine_link.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        return send_failure_response(&e, "cancel", order_id, client_order_id);
    }

    // Wait for cancel confirmation
    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => {
            let latency = start.elapsed();
            let (http_status, message) = engine_outcome(EngineRequest::Cancel, response.status);

            if http_status.is_success() {
                info!(
                    client_order_id = client_order_id,
                    order_id = response.order_id,
                    user_id = user_id,
                    status = ?response.status,
                    latency_us = latency.as_micros(),
                    "{message}"
                );
            } else {
                warn!(
                    client_order_id = client_order_id,
                    order_id = response.order_id,
                    user_id = user_id,
                    status = ?response.status,
                    latency_us = latency.as_micros(),
                    "{message}"
                );
            }

            cancel_response(order_id, client_order_id, &response)
        }
        Err(ResponseError::Timeout) => {
            correlation_service.timeout_request(client_order_id);
            HttpResponse::RequestTimeout().json(OrderResponse {
                order_id,
                client_order_id,
                status: "timeout".to_string(),
                message: "Cancel timeout".to_string(),
            })
        }
        Err(ResponseError::LinkLost) => engine_unavailable_response(order_id, client_order_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use vex_networking::client::GatewayError;

    async fn body_message(response: HttpResponse) -> String {
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        body["message"].as_str().unwrap().to_string()
    }

    #[actix_web::test]
    async fn busy_engine_is_a_retryable_503_whether_back_pressured_or_ring_full() {
        for response in [
            send_failure_response(&GatewayError::Busy, "order", 0, 7),
            engine_busy_response(0, 7),
        ] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                response
                    .headers()
                    .get(actix_web::http::header::RETRY_AFTER)
                    .unwrap(),
                "1"
            );
            assert_eq!(body_message(response).await, ENGINE_BUSY_MESSAGE);
        }
    }

    /// The gateway reconnects on its own, so a client may resend after `Retry-After`.
    fn assert_engine_unavailable_headers(response: &HttpResponse) {
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(actix_web::http::header::RETRY_AFTER)
                .unwrap(),
            "5"
        );
    }

    #[actix_web::test]
    async fn disconnected_engine_is_a_retryable_unavailable_503() {
        let response = send_failure_response(&GatewayError::NotConnected, "cancel", 9, 7);

        assert_engine_unavailable_headers(&response);
        assert_eq!(body_message(response).await, ENGINE_UNAVAILABLE_MESSAGE);
    }

    /// An order sent before the link was lost may have executed: it gets a timeout's answer, which
    /// never invites a resend, unlike the 503s.
    #[actix_web::test]
    async fn order_whose_link_was_lost_has_an_unknown_outcome_and_no_retry_after() {
        let response = response_from_outcome(link_lost_outcome(7));

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert!(
            !response
                .headers()
                .contains_key(actix_web::http::header::RETRY_AFTER)
        );
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "outcome_unknown");
        assert_eq!(
            body["message"],
            "The link to the matching engine was lost after the order was sent, so its outcome is \
             unknown. Retain client_order_id 7 and check open orders or order history after \
             asynchronous persistence catches up. Do not retry this idempotency key."
        );
    }

    #[actix_web::test]
    async fn order_endpoints_answer_unavailable_while_the_engine_link_is_down() {
        use actix_web::HttpMessage as _;
        use actix_web::dev::Service as _;

        let Some(store) = live_idempotency_store().await else {
            return;
        };
        // No engine link has been established, and the caller is authenticated.
        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(web::Data::new(EngineLink::new()))
                .app_data(web::Data::new(Arc::new(CorrelationService::new())))
                .app_data(web::Data::new(Arc::new(store)))
                .wrap_fn(|request, service| {
                    request.extensions_mut().insert(
                        lib_auth::middleware::jwt_auth::AuthenticatedUser {
                            user_id: uuid::Uuid::new_v4(),
                        },
                    );
                    service.call(request)
                })
                .configure(crate::routes::order_route::configure),
        )
        .await;

        let order = actix_web::test::TestRequest::post()
            .uri("/order")
            .insert_header((
                IDEMPOTENCY_KEY_HEADER,
                format!("no-link-{}", generate_client_order_id()),
            ))
            .set_json(serde_json::json!({
                "price": 100,
                "volume": 1,
                "market_id": 1,
                "side": "Bid",
                "time_in_force": "Gtc",
                "order_type": "Limit",
            }))
            .to_request();
        let cancel = actix_web::test::TestRequest::post()
            .uri("/cancel")
            .set_json(serde_json::json!({ "order_id": "9", "market_id": 1 }))
            .to_request();

        for request in [order, cancel] {
            let response = actix_web::test::call_service(&app, request).await;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                response
                    .headers()
                    .get(actix_web::http::header::RETRY_AFTER)
                    .unwrap(),
                "5"
            );
            let body: serde_json::Value = actix_web::test::read_body_json(response).await;
            assert_eq!(body["message"], ENGINE_UNAVAILABLE_MESSAGE);
        }
    }

    #[actix_web::test]
    async fn unusable_publication_is_a_500_naming_the_request() {
        let response = send_failure_response(
            &GatewayError::SendError("publication closed".to_string()),
            "cancel",
            9,
            7,
        );

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !response
                .headers()
                .contains_key(actix_web::http::header::RETRY_AFTER)
        );
        assert_eq!(
            body_message(response).await,
            "Failed to send cancel: Failed to send message: publication closed"
        );
    }

    #[test]
    fn engine_outcome_classifies_every_status() {
        use EngineRequest::{Cancel, Order};

        for status in [
            Status::Placed,
            Status::Cancelled,
            Status::PartiallyFilled,
            Status::Filled,
            Status::Processing,
            Status::Processed,
        ] {
            assert_eq!(
                engine_outcome(Order, status),
                (StatusCode::OK, "Order processed successfully")
            );
            assert_eq!(
                engine_outcome(Cancel, status),
                (StatusCode::OK, "Cancel processed successfully")
            );
        }
        assert_eq!(
            engine_outcome(Order, Status::Rejected),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Order rejected by the matching engine"
            )
        );
        assert_eq!(
            engine_outcome(Cancel, Status::Rejected),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Cancel rejected: the order is not open or not owned by the caller"
            )
        );
        for request in [Order, Cancel] {
            assert_eq!(
                engine_outcome(request, Status::Busy),
                (StatusCode::SERVICE_UNAVAILABLE, ENGINE_BUSY_MESSAGE)
            );
        }
    }

    #[actix_web::test]
    async fn cancel_answers_follow_the_engine_reply() {
        let reply = |status| OrderCommand {
            order_id: 9,
            client_order_id: 7,
            status,
            ..OrderCommand::default()
        };

        let busy = cancel_response(9, 7, &reply(Status::Busy));
        assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            busy.headers()
                .contains_key(actix_web::http::header::RETRY_AFTER)
        );
        assert_eq!(body_message(busy).await, ENGINE_BUSY_MESSAGE);

        let rejected = cancel_response(9, 7, &reply(Status::Rejected));
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body_message(rejected).await,
            "Cancel rejected: the order is not open or not owned by the caller"
        );

        let cancelled = cancel_response(9, 7, &reply(Status::Cancelled));
        assert_eq!(cancelled.status(), StatusCode::OK);
        assert_eq!(
            body_message(cancelled).await,
            "Cancel processed successfully"
        );
    }

    #[test]
    fn protocol_error_is_a_500() {
        let response = send_failure_response(
            &GatewayError::ProtocolError("invalid command".to_string()),
            "order",
            0,
            7,
        );

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    use std::sync::{Arc, Mutex};
    use tracing::{
        Event, Metadata, Subscriber,
        field::{Field, Visit},
        span::{Attributes, Id, Record},
    };

    #[derive(Clone)]
    struct CapturingSubscriber {
        events: Arc<Mutex<Vec<String>>>,
    }

    struct FieldVisitor<'a> {
        output: &'a mut String,
    }

    impl Visit for FieldVisitor<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.output
                .push_str(&format!(" {}={value:?}", field.name()));
        }
    }

    impl Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut output = String::new();
            event.record(&mut FieldVisitor {
                output: &mut output,
            });
            self.events.lock().unwrap().push(output);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[test]
    fn outcome_unknown_message_never_invites_a_retry() {
        let message = outcome_unknown_message("Order was sent but its outcome is unknown.", 42);

        assert_eq!(
            message,
            "Order was sent but its outcome is unknown. Retain client_order_id 42 and check \
             open orders or order history after asynchronous persistence catches up. Do not \
             retry this idempotency key."
        );
    }

    /// The store on `REDIS_URL` (default local 6379), or `None` when Redis is unreachable so the
    /// calling test skips, as lib-core's own idempotency tests do.
    async fn live_idempotency_store() -> Option<OrderIdempotencyStore> {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = redis::Client::open(redis_url.as_str()).expect("valid Redis URL");
        match redis::aio::ConnectionManager::new(client).await {
            Ok(connection) => Some(OrderIdempotencyStore::new(connection)),
            Err(error) => {
                eprintln!("Skipping live-Redis test: unable to connect to {redis_url}: {error}");
                None
            }
        }
    }

    fn engine_outcome_record(client_order_id: u64, status: &str) -> IdempotentOrderOutcome {
        IdempotentOrderOutcome {
            http_status: StatusCode::OK.as_u16(),
            order_id: 42,
            client_order_id,
            status: status.to_string(),
            message: "Order processed successfully".to_string(),
        }
    }

    #[tokio::test]
    async fn busy_engine_answer_releases_the_claim_so_the_same_key_is_published_again() {
        let Some(store) = live_idempotency_store().await else {
            return;
        };
        let user_id = generate_client_order_id();
        let key = format!("busy-answer-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();
        assert_eq!(
            store
                .claim(user_id, &key, "payload", client_order_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );

        let response = settle_idempotent_order(
            &store,
            user_id,
            &key,
            "payload",
            Status::Busy,
            engine_outcome_record(client_order_id, "Busy"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            response
                .headers()
                .contains_key(actix_web::http::header::RETRY_AFTER)
        );
        let retry_client_order_id = generate_client_order_id();
        assert_eq!(
            store
                .claim(user_id, &key, "payload", retry_client_order_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        store
            .release(
                user_id,
                &key,
                "payload",
                retry_client_order_id,
                ReleaseReason::Busy,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn final_engine_answer_is_recorded_and_replayed_for_the_same_key() {
        let Some(store) = live_idempotency_store().await else {
            return;
        };
        let user_id = generate_client_order_id();
        let key = format!("final-answer-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();
        store
            .claim(user_id, &key, "payload", client_order_id)
            .await
            .unwrap();
        let outcome = engine_outcome_record(client_order_id, "Placed");

        let response = settle_idempotent_order(
            &store,
            user_id,
            &key,
            "payload",
            Status::Placed,
            outcome.clone(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            store
                .claim(user_id, &key, "payload", generate_client_order_id())
                .await
                .unwrap(),
            OrderClaim::Completed(outcome)
        );
    }

    #[tokio::test]
    async fn request_waiting_on_a_released_claim_gets_a_retryable_503() {
        let Some(store) = live_idempotency_store().await else {
            return;
        };
        let user_id = generate_client_order_id();
        let key = format!("released-wait-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();
        assert_eq!(
            store
                .claim(user_id, &key, "payload", client_order_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        // The engine turned the owner away, so the owner releases its claim.
        settle_idempotent_order(
            &store,
            user_id,
            &key,
            "payload",
            Status::Busy,
            engine_outcome_record(client_order_id, "Busy"),
        )
        .await;

        let response =
            await_in_flight_order(&store, user_id, &key, "payload", client_order_id).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(actix_web::http::header::RETRY_AFTER)
                .unwrap(),
            "1"
        );
        assert_eq!(body_message(response).await, ENGINE_BUSY_MESSAGE);
        let retry_client_order_id = generate_client_order_id();
        assert_eq!(
            store
                .claim(user_id, &key, "payload", retry_client_order_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        store
            .release(
                user_id,
                &key,
                "payload",
                retry_client_order_id,
                ReleaseReason::Busy,
            )
            .await
            .unwrap();
    }

    #[test]
    fn only_back_pressure_releases_a_claim_as_busy() {
        assert_eq!(release_reason(&GatewayError::Busy), ReleaseReason::Busy);
        for error in [
            GatewayError::NotConnected,
            GatewayError::SendError("publication closed".to_string()),
        ] {
            assert_eq!(release_reason(&error), ReleaseReason::EngineUnavailable);
        }
    }

    #[tokio::test]
    async fn request_waiting_on_a_claim_released_without_a_core_gets_the_unavailable_503() {
        let Some(store) = live_idempotency_store().await else {
            return;
        };
        let user_id = generate_client_order_id();
        let key = format!("unavailable-wait-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();
        assert_eq!(
            store
                .claim(user_id, &key, "payload", client_order_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        // The owner's publish failed because no core is connected, so it releases its claim.
        release_idempotency_claim(
            &store,
            user_id,
            &key,
            "payload",
            client_order_id,
            release_reason(&GatewayError::NotConnected),
        )
        .await;

        let response =
            await_in_flight_order(&store, user_id, &key, "payload", client_order_id).await;

        assert_engine_unavailable_headers(&response);
        assert_eq!(body_message(response).await, ENGINE_UNAVAILABLE_MESSAGE);
    }

    #[tokio::test]
    async fn request_waiting_on_a_missing_claim_is_still_told_not_to_retry() {
        let Some(store) = live_idempotency_store().await else {
            return;
        };
        // No claim and no release marker under the key: an evicted claim looks exactly like this,
        // and its order may have been published.
        let user_id = generate_client_order_id();
        let key = format!("missing-wait-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();

        let response =
            await_in_flight_order(&store, user_id, &key, "payload", client_order_id).await;

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert!(
            body_message(response)
                .await
                .ends_with("Do not retry this idempotency key.")
        );
    }

    #[test]
    fn missing_idempotency_claim_is_logged() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            events: events.clone(),
        };
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let idempotency_key = format!("missing-claim-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();

        record_idempotent_completion(Ok(false), client_order_id, &idempotency_key);

        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| {
            event.contains("idempotency claim vanished before completion could be recorded")
                && event.contains(&idempotency_key)
                && event.contains(&client_order_id.to_string())
        }));
        assert!(
            !events
                .iter()
                .any(|event| event.contains("failed to record idempotent completion"))
        );
    }

    #[test]
    fn rejects_zero_volume() {
        let result = validate_order_inputs(100, 0, TimeInForce::Gtc, &OrderType::Limit);

        assert_eq!(
            result,
            Err("order volume must be greater than zero".to_string())
        );
    }

    #[test]
    fn rejects_zero_price_limit_order() {
        let result = validate_order_inputs(0, 1, TimeInForce::Gtc, &OrderType::Limit);

        assert_eq!(
            result,
            Err("limit order price must not use market sentinel values".to_string())
        );
    }

    #[test]
    fn rejects_max_price_limit_order() {
        let result = validate_order_inputs(u64::MAX, 1, TimeInForce::Gtc, &OrderType::Limit);

        assert_eq!(
            result,
            Err("limit order price must not use market sentinel values".to_string())
        );
    }

    #[test]
    fn rejects_gtc_market_order() {
        let result = validate_order_inputs(100, 1, TimeInForce::Gtc, &OrderType::Market);

        assert_eq!(
            result,
            Err("market order time_in_force must be IOC or FOK".to_string())
        );
    }

    #[test]
    fn accepts_valid_order() {
        let result = validate_order_inputs(100, 1, TimeInForce::Gtc, &OrderType::Limit);

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn parses_cancel_order_id() {
        assert_eq!(parse_order_id("18446744073709551615"), Ok(u64::MAX));
        assert!(parse_order_id("not-an-order-id").is_err());
    }

    #[test]
    fn order_timestamp_saturates_at_epoch_when_clock_is_before_epoch() {
        let before_epoch = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();

        assert_eq!(unix_timestamp_nanos(before_epoch), 0);
    }
}
