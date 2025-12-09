//! Integration test for order placement and cancellation
//!
//! This test starts a web server and tests the order handlers without authentication,
//! directly testing the integration with vex-core through the correlation service.

use actix_web::{web, App, HttpResponse, HttpServer};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    generate_client_order_id, wait_for_response, CorrelationService, Publisher, ResponseError,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize, Serialize)]
struct OrderRequest {
    pub price: u64,
    pub volume: u64,
    pub market_id: u32,
    pub side: Side,
    pub time_in_force: TimeInForce,
    pub user_id: u64, // Directly pass user_id for testing
}

#[derive(Debug, Deserialize, Serialize)]
struct CancelOrderRequest {
    pub order_id: u64,
    pub market_id: u32,
    pub user_id: u64, // Directly pass user_id for testing
}

#[derive(Debug, Serialize, Deserialize)]
struct OrderResponse {
    pub order_id: u64,
    pub client_order_id: u64,
    pub status: String,
    pub message: String,
}

/// Test version of place_order handler that doesn't require authentication
async fn test_place_order(
    order_data: web::Json<OrderRequest>,
    publisher: web::Data<Arc<tokio::sync::Mutex<Publisher>>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let start = Instant::now();
    let order_data = order_data.into_inner();
    let user_id = order_data.user_id;

    // Generate unique client order ID for correlation
    let client_order_id = generate_client_order_id();

    // Register this request - get receiver for response
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build order command
    let command = OrderCommand {
        command: OrderCommandType::PlaceOrder,
        order_id: 0, // Will be assigned by VEX Core
        client_order_id,
        market_id: order_data.market_id,
        user_id,
        price: order_data.price,
        size: order_data.volume,
        side: order_data.side,
        time_in_force: order_data.time_in_force,
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
        eprintln!("Failed to send order to VEX Core: {}", e);
        return HttpResponse::InternalServerError().json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "error".to_string(),
            message: format!("Failed to send order: {}", e),
        });
    }

    // Wait for response with timeout
    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => {
            let latency = start.elapsed();
            println!(
                "Order processed: client_order_id={}, order_id={}, status={:?}, latency={}us",
                client_order_id,
                response.order_id,
                response.status,
                latency.as_micros()
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
            HttpResponse::RequestTimeout().json(OrderResponse {
                order_id: 0,
                client_order_id,
                status: "timeout".to_string(),
                message: "Order processing timeout".to_string(),
            })
        }
        Err(ResponseError::Cancelled) => HttpResponse::InternalServerError().json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "error".to_string(),
            message: "Internal error".to_string(),
        }),
    }
}

/// Test version of cancel_order handler that doesn't require authentication
async fn test_cancel_order(
    cancel_data: web::Json<CancelOrderRequest>,
    publisher: web::Data<Arc<tokio::sync::Mutex<Publisher>>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let start = Instant::now();
    let cancel_data = cancel_data.into_inner();
    let user_id = cancel_data.user_id;

    // Generate NEW client order ID for THIS cancel request
    let client_order_id = generate_client_order_id();
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build cancel command
    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id: cancel_data.order_id,
        client_order_id,
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
            println!(
                "Cancel processed: client_order_id={}, order_id={}, latency={}us",
                client_order_id,
                response.order_id,
                latency.as_micros()
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

/// Test helper to spawn the test server
fn spawn_test_server(publisher: Publisher, correlation_service: CorrelationService) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();

    let publisher = Arc::new(tokio::sync::Mutex::new(publisher));
    let correlation_service = Arc::new(correlation_service);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(publisher.clone()))
            .app_data(web::Data::new(correlation_service.clone()))
            .route("/test/order/place", web::post().to(test_place_order))
            .route("/test/order/cancel", web::post().to(test_cancel_order))
    })
    .listen(listener)
    .expect("Failed to bind address")
    .run();

    tokio::spawn(server);

    format!("http://127.0.0.1:{}", port)
}

#[tokio::test]
#[ignore] // Requires vex-core - skip in CI
async fn test_order_placement_and_cancellation() {
    // Initialize VexGateway client
    println!("Initializing VexGateway client for testing");

    // Use DEVELOPMENT config to match vex-core (ports 40001/40002)
    let mut gateway_config = vex_config::GatewayNetworkingConfig::development_defaults();
    gateway_config.context_dir =
        "/Users/adityaanand/vex-backend/vex-core/aeron-test-server".to_string();

    println!(
        "Using Aeron context directory: {}",
        gateway_config.context_dir
    );
    println!(
        "Connecting to vex-core on ports: {}:{}",
        gateway_config.core_port, gateway_config.core_control_port
    );

    let mut gateway_client = vex_networking::client::VexGateway::new(gateway_config)
        .expect("Failed to initialize VexGateway");

    // Create correlation service
    let correlation_service = CorrelationService::new();

    // Create Aeron handler with correlation service
    let handler = lib_server::CorrelationHandler::new(
        gateway_client.gateway_id(),
        correlation_service.clone(),
    );

    // Start the gateway client and get the publisher
    println!("Starting VexGateway with correlation handler");
    let publisher = gateway_client
        .start(handler)
        .expect("Failed to start VexGateway");

    // Spawn test server
    let server_address = spawn_test_server(publisher, correlation_service);
    println!("Test server running at: {}", server_address);

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create HTTP client for testing
    let client = reqwest::Client::new();

    // Test 1: Place an order
    println!("\n=== Test 1: Place Order ===");
    let place_order_request = OrderRequest {
        price: 50000_00000000, // $50,000 with 8 decimals
        volume: 1_00000000,    // 1 BTC with 8 decimals
        market_id: 5991,       // Use valid market_id loaded in vex-core
        side: Side::Bid,
        time_in_force: TimeInForce::Gtc,
        user_id: 12345,
    };

    let response = client
        .post(format!("{}/test/order/place", server_address))
        .json(&place_order_request)
        .send()
        .await
        .expect("Failed to send place order request");

    println!("Place order response status: {}", response.status());
    assert!(
        response.status().is_success() || response.status() == 408,
        "Expected success or timeout, got: {}",
        response.status()
    );

    let order_response: OrderResponse = response.json().await.expect("Failed to parse response");
    println!(
        "Order placed: order_id={}, client_order_id={}, status={}, message={}",
        order_response.order_id,
        order_response.client_order_id,
        order_response.status,
        order_response.message
    );

    // Test 2: Cancel the order (if it was placed successfully)
    if order_response.order_id > 0 {
        println!("\n=== Test 2: Cancel Order ===");
        let cancel_order_request = CancelOrderRequest {
            order_id: order_response.order_id,
            market_id: 5991, // Must match the order's market_id
            user_id: 12345,
        };

        let response = client
            .post(format!("{}/test/order/cancel", server_address))
            .json(&cancel_order_request)
            .send()
            .await
            .expect("Failed to send cancel order request");

        println!("Cancel order response status: {}", response.status());
        assert!(
            response.status().is_success() || response.status() == 408,
            "Expected success or timeout, got: {}",
            response.status()
        );

        let cancel_response: OrderResponse =
            response.json().await.expect("Failed to parse response");
        println!(
            "Order cancelled: order_id={}, client_order_id={}, status={}, message={}",
            cancel_response.order_id,
            cancel_response.client_order_id,
            cancel_response.status,
            cancel_response.message
        );
    } else {
        println!("\n=== Skipping Test 2: No order to cancel ===");
        println!("Order placement timed out or failed, so we can't test cancellation");
    }
}
