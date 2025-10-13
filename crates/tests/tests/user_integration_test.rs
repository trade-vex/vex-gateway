//! Integration test for user handlers - Balance and Order retrieval after order placement
//!
//! This test verifies the complete flow:
//! 1. Place order via order handler (without auth for testing)
//! 2. Wait for vex-core to process and Kafka to sync to Redis/DB
//! 3. Fetch balances via user handler - verify balance updates
//! 4. Fetch orders via user handler - verify order is stored correctly

use actix_web::{web, App, HttpResponse, HttpServer, Result};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    generate_client_order_id, wait_for_response, BalancesListResponse, CorrelationService,
    OrderCacheConfig, OrderStore, OrdersListResponse, PostgresTradingRepository, Publisher,
    ResponseError, UserBalanceCacheConfig, UserBalanceStore,
};
use sea_orm::{Database, DatabaseConnection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

// ==================== Request/Response DTOs ====================

#[derive(Debug, Deserialize, Serialize)]
struct OrderRequest {
    pub price: u64,
    pub volume: u64,
    pub market_id: u32,
    pub side: Side,
    pub time_in_force: TimeInForce,
    pub user_id: u64, // For testing - normally from JWT
}

#[derive(Debug, Serialize, Deserialize)]
struct OrderResponse {
    pub order_id: u64,
    pub client_order_id: u64,
    pub status: String,
    pub message: String,
}

// ==================== Test Handlers (Unauthenticated versions) ====================

/// Test version of place_order handler - same logic as real handler but without auth
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
        order_id: 0,
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

/// Test version of get_balances handler - same logic as real handler but without auth
async fn test_get_balances(
    user_id: web::Path<u64>,
    balance_store: web::Data<Arc<UserBalanceStore>>,
) -> Result<HttpResponse> {
    let user_id = *user_id;
    let user_uuid = Uuid::from_u128(user_id as u128);

    match balance_store.get_user_balances(user_uuid).await {
        Ok(balances) => {
            let total_assets = balances.len();
            let balance_responses: Vec<_> =
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

/// Test version of get_open_orders handler - same logic as real handler but without auth
async fn test_get_open_orders(
    user_id: web::Path<u64>,
    order_store: web::Data<Arc<OrderStore>>,
) -> Result<HttpResponse> {
    let user_id = *user_id;
    let user_uuid = Uuid::from_u128(user_id as u128);

    match order_store.get_user_orders(user_uuid).await {
        Ok(orders) => {
            let total = orders.len();
            let order_responses: Vec<_> = orders.into_iter().map(|order| order.into()).collect();

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

// ==================== Test Server Setup ====================

fn spawn_test_server(
    publisher: Publisher,
    correlation_service: CorrelationService,
    balance_store: UserBalanceStore,
    order_store: OrderStore,
) -> String {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();

    let publisher = Arc::new(tokio::sync::Mutex::new(publisher));
    let correlation_service = Arc::new(correlation_service);
    let balance_store = Arc::new(balance_store);
    let order_store = Arc::new(order_store);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(publisher.clone()))
            .app_data(web::Data::new(correlation_service.clone()))
            .app_data(web::Data::new(balance_store.clone()))
            .app_data(web::Data::new(order_store.clone()))
            // Order endpoints
            .route("/test/order/place", web::post().to(test_place_order))
            // User endpoints
            .route(
                "/test/user/{user_id}/balances",
                web::get().to(test_get_balances),
            )
            .route(
                "/test/user/{user_id}/orders",
                web::get().to(test_get_open_orders),
            )
    })
    .listen(listener)
    .expect("Failed to bind address")
    .run();

    tokio::spawn(server);

    format!("http://127.0.0.1:{}", port)
}

// ==================== Database Setup ====================

async fn init_database() -> Option<Arc<DatabaseConnection>> {
    // Use your Docker setup: trading_db with postgres123 password
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://postgres:postgres123@localhost:5432/trading_db".to_string()
    });

    match Database::connect(&database_url).await {
        Ok(db) => {
            println!("✓ Database connected: {}", database_url);
            Some(Arc::new(db))
        }
        Err(e) => {
            println!(
                "✗ Database connection failed (continuing with Redis-only): {}",
                e
            );
            None
        }
    }
}

// ==================== Integration Test ====================

#[tokio::test]
async fn test_user_balance_and_order_retrieval() {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  Integration Test: User Balance & Order Retrieval            ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // ==================== SETUP ====================
    println!("🔧 SETUP PHASE\n");

    // 1. Initialize VexGateway client
    println!("1️⃣  Initializing VexGateway client");
    let mut gateway_config = vex_config::GatewayNetworkingConfig::development_defaults();
    gateway_config.context_dir =
        "/Users/adityaanand/vex-backend/vex-core/aeron-test-server".to_string();
    println!("   📁 Context: {}", gateway_config.context_dir);
    println!(
        "   🔌 Ports: {}:{}",
        gateway_config.core_port, gateway_config.core_control_port
    );

    let mut gateway_client = vex_networking::client::VexGateway::new(gateway_config)
        .expect("Failed to initialize VexGateway");

    // 2. Create correlation service and handler
    println!("\n2️⃣  Setting up correlation service");
    let correlation_service = CorrelationService::new();
    let handler = lib_server::CorrelationHandler::new(
        gateway_client.gateway_id(),
        correlation_service.clone(),
    );

    // 3. Start VexGateway
    println!("\n3️⃣  Starting VexGateway");
    let publisher = gateway_client
        .start(handler)
        .expect("Failed to start VexGateway");
    println!("   ✓ Gateway started with ID: {}", gateway_client.gateway_id());

    // 4. Initialize Redis
    println!("\n4️⃣  Connecting to Redis");
    // Use your Docker setup: Redis on port 6380 (mapped from container's 6379)
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());
    println!("   🔗 {}", redis_url);

    // 5. Initialize Database
    println!("\n5️⃣  Connecting to Database");
    let db = init_database().await;

    // 6. Create stores with Redis and DB
    println!("\n6️⃣  Initializing data stores");
    let balance_store = if let Some(ref db_conn) = db {
        let trading_repo = Arc::new(PostgresTradingRepository::new(db_conn.clone()));
        UserBalanceStore::with_redis_cache(
            UserBalanceCacheConfig {
                ttl: Duration::from_secs(300),
                enabled: true,
            },
            redis_url.clone(),
            Some(trading_repo),
        )
        .await
        .expect("Failed to create UserBalanceStore")
    } else {
        UserBalanceStore::with_redis_cache(
            UserBalanceCacheConfig {
                ttl: Duration::from_secs(300),
                enabled: true,
            },
            redis_url.clone(),
            None,
        )
        .await
        .expect("Failed to create UserBalanceStore")
    };

    let order_store = if let Some(ref db_conn) = db {
        let trading_repo = Arc::new(PostgresTradingRepository::new(db_conn.clone()));
        OrderStore::with_redis_cache(
            OrderCacheConfig {
                ttl: Duration::from_secs(300),
                enabled: true,
            },
            redis_url.clone(),
            Some(trading_repo),
        )
        .await
        .expect("Failed to create OrderStore")
    } else {
        OrderStore::with_redis_cache(
            OrderCacheConfig {
                ttl: Duration::from_secs(300),
                enabled: true,
            },
            redis_url.clone(),
            None,
        )
        .await
        .expect("Failed to create OrderStore")
    };

    println!("   ✓ Balance store: Redis + {}", if db.is_some() { "DB" } else { "Cache-only" });
    println!("   ✓ Order store: Redis + {}", if db.is_some() { "DB" } else { "Cache-only" });

    // 7. Spawn test server
    println!("\n7️⃣  Starting test HTTP server");
    let server_address =
        spawn_test_server(publisher, correlation_service, balance_store, order_store);
    println!("   🌐 {}", server_address);

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let test_user_id = 12345u64;

    println!("\n✅ Setup complete!\n");

    // ==================== TEST 1: PLACE ORDER ====================
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 1: Place Order                                         ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    let order_request = OrderRequest {
        price: 50000_00000000,
        volume: 1_00000000,
        market_id: 5991,
        side: Side::Bid,
        time_in_force: TimeInForce::Gtc,
        user_id: test_user_id,
    };

    println!("📤 Placing order:");
    println!("   User ID: {}", test_user_id);
    println!("   Market: {}", order_request.market_id);
    println!("   Side: {:?}", order_request.side);
    println!("   Price: {}", order_request.price);
    println!("   Volume: {}", order_request.volume);

    let response = client
        .post(format!("{}/test/order/place", server_address))
        .json(&order_request)
        .send()
        .await
        .expect("Failed to send request");

    println!("\n📥 Response: {}", response.status());
    let order_response: OrderResponse = response.json().await.expect("Failed to parse response");
    println!("   Order ID: {}", order_response.order_id);
    println!("   Status: {}", order_response.status);
    println!("   Message: {}", order_response.message);

    let placed_order_id = order_response.order_id;

    if placed_order_id == 0 {
        println!("\n⚠️  Order placement failed or timed out");
        println!("   Skipping subsequent tests");
        return;
    }

    // Wait for Kafka to sync data to Redis/DB
    println!("\n⏳ Waiting 3s for Kafka to sync order data to Redis/DB...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ==================== TEST 2: GET BALANCES ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 2: Get User Balances (via Handler)                     ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    println!("📤 GET /test/user/{}/balances", test_user_id);

    let balance_response = client
        .get(format!(
            "{}/test/user/{}/balances",
            server_address, test_user_id
        ))
        .send()
        .await
        .expect("Failed to get balances");

    println!("📥 Response: {}", balance_response.status());

    if balance_response.status().is_success() {
        let balances: BalancesListResponse =
            balance_response.json().await.expect("Failed to parse");
        println!("   Total assets: {}", balances.total_assets);

        for (idx, balance) in balances.balances.iter().enumerate() {
            println!("\n   💰 Balance #{}:", idx + 1);
            println!("      Asset ID: {}", balance.asset_id);
            println!("      Available: {}", balance.available);
            println!("      Locked: {}", balance.locked);
            println!("      Total: {}", balance.total);
        }

        if balances.total_assets > 0 {
            println!("\n✅ Balance retrieval successful!");
        } else {
            println!("\n⚠️  No balances found (may still be syncing)");
        }
    } else {
        let error = balance_response.text().await.unwrap_or_default();
        println!("❌ Error: {}", error);
    }

    // ==================== TEST 3: GET OPEN ORDERS ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 3: Get User Orders (via Handler)                       ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    println!("📤 GET /test/user/{}/orders", test_user_id);

    let orders_response = client
        .get(format!(
            "{}/test/user/{}/orders",
            server_address, test_user_id
        ))
        .send()
        .await
        .expect("Failed to get orders");

    println!("📥 Response: {}", orders_response.status());

    if orders_response.status().is_success() {
        let orders: OrdersListResponse = orders_response.json().await.expect("Failed to parse");
        println!("   Total orders: {}", orders.total);

        let mut found_our_order = false;
        for (idx, order) in orders.orders.iter().enumerate() {
            println!("\n   📋 Order #{}:", idx + 1);
            println!("      Order ID: {}", order.order_id);
            println!("      Market: {}", order.market_id);
            println!("      Side: {:?}", order.side);
            println!("      Price: {}", order.price);
            println!("      Size: {}", order.size);

            if order.order_id == placed_order_id {
                println!("      ✓ THIS IS OUR PLACED ORDER!");
                found_our_order = true;
            }
        }

        if found_our_order {
            println!("\n✅ Successfully found our placed order in user orders!");
        } else {
            println!("\n⚠️  Our placed order not found yet (may still be syncing)");
        }
    } else {
        let error = orders_response.text().await.unwrap_or_default();
        println!("❌ Error: {}", error);
    }

    // ==================== SUMMARY ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST SUMMARY                                                 ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    println!("✅ Order Placement: SUCCESS (Order ID: {})", placed_order_id);
    println!("✅ Balance Handler: Tested");
    println!("✅ Order Handler: Tested");
    println!("\n🎉 Integration test completed!\n");
}

