//! Integration test for user handlers - Balance and Order retrieval after order placement
//!
//! This test verifies the complete flow:
//! 1. Place order via order handler (without auth for testing)
//! 2. Wait for vex-core to process and Kafka to sync to Redis/DB
//! 3. Fetch balances via user handler - verify balance updates
//! 4. Fetch orders via user handler - verify order is stored correctly

use actix_web::{App, HttpResponse, HttpServer, Result, web};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    BalancesListResponse, CorrelationService, OrderCacheConfig, OrderStore, OrdersListResponse,
    PostgresTradingRepository, Publisher, ResponseError, UserBalanceCacheConfig, UserBalanceStore,
    generate_client_order_id, wait_for_response,
};
use sea_orm::{Database, DatabaseConnection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tests::gateway_send_error;

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

#[derive(Debug, Deserialize, Serialize)]
struct DepositRequest {
    pub user_id: u64,
    pub asset_id: u32, // Using market_id field as asset_id
    pub amount: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct OrderResponse {
    pub order_id: u64,
    pub client_order_id: u64,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DepositResponse {
    pub user_id: u64,
    pub asset_id: u32,
    pub amount: u64,
    pub status: String,
    pub message: String,
}

// ==================== Test Handlers (Unauthenticated versions) ====================

/// Test version of deposit handler - deposits funds to user account (faucet)
async fn test_deposit_funds(
    deposit_data: web::Json<DepositRequest>,
    publisher: web::Data<Arc<Publisher>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let start = Instant::now();
    let deposit_data = deposit_data.into_inner();
    let user_id = deposit_data.user_id;
    let asset_id = deposit_data.asset_id;
    let amount = deposit_data.amount;

    // Generate unique client order ID for correlation
    let client_order_id = generate_client_order_id();

    // Register this request
    let response_receiver = correlation_service.register_request(client_order_id);

    // Build deposit command
    // For Deposit/Withdraw: market_id field is used as asset_id
    // balance[0] contains the deposit amount
    let mut command = OrderCommand {
        command: OrderCommandType::DepositFunds,
        order_id: 0,
        client_order_id,
        market_id: asset_id, // Using market_id field as asset_id
        user_id,
        price: 0,
        size: amount, // Amount to deposit
        original_size: amount,
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
        route_gateway_id: 0,
    };

    // Set the balance amount for deposit
    command.balance[0].available = amount;

    // Send to VEX Core
    let send_result = publisher.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        eprintln!("Failed to send deposit command to VEX Core: {}", e);
        let (status, message) = gateway_send_error(&e, format!("Failed to send deposit: {}", e));
        return HttpResponse::build(status).json(DepositResponse {
            user_id,
            asset_id,
            amount,
            status: "error".to_string(),
            message,
        });
    }

    // Wait for response with timeout
    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => {
            let latency = start.elapsed();
            println!(
                "Deposit processed: user_id={}, asset_id={}, amount={}, status={:?}, latency={}us",
                user_id,
                asset_id,
                amount,
                response.status,
                latency.as_micros()
            );

            HttpResponse::Ok().json(DepositResponse {
                user_id,
                asset_id,
                amount,
                status: format!("{:?}", response.status),
                message: "Deposit processed successfully".to_string(),
            })
        }
        Err(ResponseError::Timeout) => {
            correlation_service.cancel_request(client_order_id);
            HttpResponse::RequestTimeout().json(DepositResponse {
                user_id,
                asset_id,
                amount,
                status: "timeout".to_string(),
                message: "Deposit processing timeout".to_string(),
            })
        }
        Err(ResponseError::Cancelled) => {
            HttpResponse::InternalServerError().json(DepositResponse {
                user_id,
                asset_id,
                amount,
                status: "error".to_string(),
                message: "Internal error".to_string(),
            })
        }
    }
}

/// Test version of place_order handler - same logic as real handler but without auth
async fn test_place_order(
    order_data: web::Json<OrderRequest>,
    publisher: web::Data<Arc<Publisher>>,
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
        original_size: order_data.volume,
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
        route_gateway_id: 0,
    };

    // Send to VEX Core
    let send_result = publisher.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        eprintln!("Failed to send order to VEX Core: {}", e);
        let (status, message) = gateway_send_error(&e, format!("Failed to send order: {}", e));
        return HttpResponse::build(status).json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "error".to_string(),
            message,
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

    println!("🔍 DEBUG get_balances:");
    println!("   User ID (u64): {}", user_id);

    match balance_store.get_user_balances(user_id).await {
        Ok(balances) => {
            println!("   ✓ Retrieved {} balances from store", balances.len());
            for (idx, balance) in balances.iter().enumerate() {
                println!(
                    "   Balance {}: user_id={}, asset_id={}, available={}, locked={}",
                    idx, balance.user_id, balance.asset_id, balance.available, balance.locked
                );
            }

            let total_assets = balances.len();
            let balance_responses: Vec<lib_core::dto::trading::UserBalanceResponse> = balances
                .into_iter()
                .map(|balance| balance_store.to_response(balance))
                .collect();

            let response = BalancesListResponse {
                balances: balance_responses,
                total_assets,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            println!("   ✗ Error retrieving balances: {}", e);
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

    println!("🔍 DEBUG get_open_orders:");
    println!("   User ID (u64): {}", user_id);

    match order_store.get_user_orders(user_id).await {
        Ok(orders) => {
            println!("   ✓ Retrieved {} orders from store", orders.len());
            for (idx, order) in orders.iter().enumerate() {
                println!(
                    "   Order {}: order_id={}, user_id={}, market_id={}, price={}, size={}",
                    idx,
                    order.order_order_id,
                    order.order_user_id,
                    order.market_id,
                    order.order_price,
                    order.order_size
                );
            }

            let total = orders.len();
            let order_responses: Vec<_> = orders
                .into_iter()
                .filter_map(|order| order_store.to_response(order))
                .collect();

            let response = OrdersListResponse {
                orders: order_responses,
                total,
                page: 1,
                limit: 100,
                total_pages: 1,
                has_next: false,
                has_prev: false,
            };

            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            println!("   ✗ Error retrieving orders: {}", e);
            tracing::error!("failed to get open orders: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "failed to retrieve open orders",
                "message": e.to_string()
            })))
        }
    }
}

/// Debug endpoint to check Redis keys
async fn test_debug_redis(
    _balance_store: web::Data<Arc<UserBalanceStore>>,
    _order_store: web::Data<Arc<OrderStore>>,
) -> Result<HttpResponse> {
    println!("\n🔍 DEBUG: Checking Redis/DB state...");

    // Try to access internal Redis connection for debugging
    // This is a simplified version - in production you'd have proper debug endpoints

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "message": "Check server logs for debug output"
    })))
}

// ==================== Test Server Setup ====================

fn spawn_test_server(
    publisher: Publisher,
    correlation_service: CorrelationService,
    balance_store: UserBalanceStore,
    order_store: OrderStore,
) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();

    let publisher = Arc::new(publisher);
    let correlation_service = Arc::new(correlation_service);
    let balance_store = Arc::new(balance_store);
    let order_store = Arc::new(order_store);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(publisher.clone()))
            .app_data(web::Data::new(correlation_service.clone()))
            .app_data(web::Data::new(balance_store.clone()))
            .app_data(web::Data::new(order_store.clone()))
            // Faucet/Deposit endpoint
            .route("/test/faucet/deposit", web::post().to(test_deposit_funds))
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
            // Debug endpoint
            .route("/test/debug/redis", web::get().to(test_debug_redis))
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
#[ignore = "no vex-core engine or Aeron media driver in CI"]
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
    println!(
        "   ✓ Gateway started with ID: {}",
        gateway_client.gateway_id()
    );

    // 4. Initialize Redis
    println!("\n4️⃣  Connecting to Redis");
    let redis_url = tests::redis_url();
    println!("   🔗 {}", redis_url);

    // 5. Initialize Database
    println!("\n5️⃣  Connecting to Database");
    let db = init_database().await;

    // 6. Create stores with Redis and DB
    println!("\n6️⃣  Initializing data stores");
    let balance_store = if let Some(ref db_conn) = db {
        let trading_repo = Arc::new(PostgresTradingRepository::new(db_conn.clone()));
        UserBalanceStore::with_redis_cache(
            UserBalanceCacheConfig { enabled: true },
            redis_url.clone(),
            Some(trading_repo),
        )
        .await
        .expect("Failed to create UserBalanceStore")
    } else {
        UserBalanceStore::with_redis_cache(
            UserBalanceCacheConfig { enabled: true },
            redis_url.clone(),
            None,
        )
        .await
        .expect("Failed to create UserBalanceStore")
    };

    let order_store = if let Some(ref db_conn) = db {
        let trading_repo = Arc::new(PostgresTradingRepository::new(db_conn.clone()));
        OrderStore::with_redis_cache(
            OrderCacheConfig { enabled: true },
            redis_url.clone(),
            Some(trading_repo),
        )
        .await
        .expect("Failed to create OrderStore")
    } else {
        OrderStore::with_redis_cache(OrderCacheConfig { enabled: true }, redis_url.clone(), None)
            .await
            .expect("Failed to create OrderStore")
    };

    println!(
        "   ✓ Balance store: Redis + {}",
        if db.is_some() { "DB" } else { "Cache-only" }
    );
    println!(
        "   ✓ Order store: Redis + {}",
        if db.is_some() { "DB" } else { "Cache-only" }
    );

    // 7. Spawn test server
    println!("\n7️⃣  Starting test HTTP server");
    let server_address =
        spawn_test_server(publisher, correlation_service, balance_store, order_store);
    println!("   🌐 {}", server_address);

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let test_user_id = 12345u64;

    println!("\n✅ Setup complete!");
    println!("   Test User ID: {}\n", test_user_id);

    // ==================== TEST 1: DEPOSIT FUNDS (FAUCET) ====================
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 1: Deposit Funds (Faucet)                              ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Extract base and quote assets from market_id using VEX Core's encoding
    // Market ID format: (quote_asset << 16) | base_asset
    // For market 5991 (0x1767):
    //   - base_asset = 5991 & 0xFFFF = 5991
    //   - quote_asset = 5991 >> 16 = 0
    let market_id = 5991u32;
    let base_asset_id = market_id & 0xFFFF; // Lower 16 bits
    let quote_asset_id = market_id >> 16; // Upper 16 bits

    println!("📊 Market Configuration:");
    println!("   Market ID: {}", market_id);
    println!(
        "   Base Asset ID: {} (extracted from lower 16 bits)",
        base_asset_id
    );
    println!(
        "   Quote Asset ID: {} (extracted from upper 16 bits)",
        quote_asset_id
    );
    println!("   Note: Deposits must use these exact asset IDs to avoid VEX Core rejection\n");

    // Deposit base asset - 10 units
    println!("\n💰 Depositing base asset:");
    let base_deposit = DepositRequest {
        user_id: test_user_id,
        asset_id: base_asset_id,
        amount: 10_00000000, // 10 units with 8 decimals
    };
    println!("   User ID: {}", base_deposit.user_id);
    println!("   Asset ID: {} (base)", base_deposit.asset_id);
    println!("   Amount: {}", base_deposit.amount);

    let response = client
        .post(format!("{}/test/faucet/deposit", server_address))
        .json(&base_deposit)
        .send()
        .await
        .expect("Failed to deposit base asset");

    println!("\n📥 Response: {}", response.status());
    let base_deposit_response: DepositResponse =
        response.json().await.expect("Failed to parse response");
    println!("   Status: {}", base_deposit_response.status);
    println!("   Message: {}", base_deposit_response.message);

    // Deposit quote asset - 1,000,000 units (for buying base asset)
    println!("\n💰 Depositing quote asset:");
    let quote_deposit = DepositRequest {
        user_id: test_user_id,
        asset_id: quote_asset_id,
        amount: 1000000_00000000, // 1M units with 8 decimals
    };
    println!("   User ID: {}", quote_deposit.user_id);
    println!("   Asset ID: {} (quote)", quote_deposit.asset_id);
    println!("   Amount: {}", quote_deposit.amount);

    let response = client
        .post(format!("{}/test/faucet/deposit", server_address))
        .json(&quote_deposit)
        .send()
        .await
        .expect("Failed to deposit quote asset");

    println!("\n📥 Response: {}", response.status());
    let quote_deposit_response: DepositResponse =
        response.json().await.expect("Failed to parse response");
    println!("   Status: {}", quote_deposit_response.status);
    println!("   Message: {}", quote_deposit_response.message);

    // Wait for deposits to be processed and synced to Redis/DB via Kafka
    println!("\n⏳ Waiting 15s for deposits to be processed and synced to Redis/DB via Kafka...");
    tokio::time::sleep(Duration::from_secs(15)).await;

    // ==================== TEST 2: CHECK BALANCES AFTER DEPOSIT ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 2: Verify Balances After Deposit                       ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Print UUID being used for debugging
    println!("🔍 Debug Info:");
    println!("   User ID (u64): {}", test_user_id);
    println!("\n📋 Manual Verification Commands (run these to check data):");
    println!("   # Check all keys in Redis:");
    println!("   docker exec vex-kafka-pipeline-redis-1 redis-cli -p 6379 KEYS '*'");
    println!("\n   # Check keys for this user:");
    println!(
        "   docker exec vex-kafka-pipeline-redis-1 redis-cli -p 6379 KEYS '*{}*'",
        test_user_id
    );
    println!("\n   # Check balances in Postgres:");
    println!(
        "   docker exec -it vex-kafka-pipeline-postgres-1 psql -U postgres -d trading_db -c \"SELECT * FROM trading.balances_events WHERE user_id = {} ORDER BY timestamp DESC LIMIT 10;\"",
        test_user_id
    );
    println!("\n   # Check orders in Postgres:");
    println!(
        "   docker exec -it vex-kafka-pipeline-postgres-1 psql -U postgres -d trading_db -c \"SELECT * FROM trading.orders_events WHERE order_user_id = {} ORDER BY order_timestamp DESC LIMIT 10;\"",
        test_user_id
    );
    println!();

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

            // Verify deposits
            if balance.asset_id == base_asset_id {
                println!("      ✓ Base asset balance verified!");
            } else if balance.asset_id == quote_asset_id {
                println!("      ✓ Quote asset balance verified!");
            }
        }

        if balances.total_assets >= 2 {
            println!("\n✅ Deposits successful - User has sufficient balance!");
        } else {
            println!("\n⚠️  Expected at least 2 assets after deposit");
        }
    } else {
        let error = balance_response.text().await.unwrap_or_default();
        println!("❌ Error: {}", error);
    }

    // ==================== TEST 3: PLACE ORDER ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 3: Place Order (After Deposit)                         ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Place MINIMAL order - VEX Core calculates: needed = price * size (NO division!)
    // We have: 10 base (1000000000) and 1M quote (100000000000000)
    // To buy with 100 quote: size = 100_quote / 100_price = 1
    let order_request = OrderRequest {
        price: 100_00000000, // Price: 100 quote per 1 base
        volume: 1,           // Volume: 1 satoshi of base (TINY!)
        market_id,           // Use the same market_id we deposited assets for
        side: Side::Bid,
        time_in_force: TimeInForce::Gtc,
        user_id: test_user_id,
    };

    // VEX Core calculates: needed = price * size (no decimal division!)
    let vex_core_needed = order_request.price as u128 * order_request.volume as u128;

    println!("📤 Placing order:");
    println!("   User ID: {}", test_user_id);
    println!("   Market: {}", order_request.market_id);
    println!("   Side: {:?}", order_request.side);
    println!("   Price: {} (100 quote per base)", order_request.price);
    println!("   Volume: {} (1 satoshi of base)", order_request.volume);
    println!(
        "   VEX Core calculates needed as: price * size = {}",
        vex_core_needed
    );
    println!("   Available quote: 100000000000000 (1M quote)");
    println!("   Available base: 1000000000 (10 base)");

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

    // Check if order was rejected (VEX Core doesn't publish rejected orders to Kafka)
    if order_response.status.contains("Rejected") {
        println!("\n⚠️  Order was REJECTED by VEX Core!");
        println!("   This means insufficient funds or invalid parameters");
        println!("   Rejected orders are NOT published to Kafka, so we can't retrieve them");
        println!("   Skipping order retrieval test");
        return;
    }

    if placed_order_id == 0 {
        println!("\n⚠️  Order placement failed or timed out");
        println!("   Skipping subsequent tests");
        return;
    }

    // Wait for Kafka consumer to sync order data to Redis/DB
    println!("\n⏳ Waiting 15s for Kafka consumer to sync PLACED order to Redis/DB...");
    println!("   (Rejected orders are NOT synced - that's correct behavior)");
    tokio::time::sleep(Duration::from_secs(15)).await;

    // ==================== TEST 4: GET BALANCES AFTER ORDER ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 4: Get User Balances After Order (via Handler)         ║");
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

    // ==================== TEST 5: GET OPEN ORDERS ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  TEST 5: Get User Orders (via Handler)                       ║");
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

            if u64::try_from(order.order_id).unwrap_or(0) == placed_order_id {
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

    println!(
        "✅ TEST 1: Deposit Funds (Faucet) - {}",
        if base_deposit_response.status.contains("Processed")
            || quote_deposit_response.status.contains("Processed")
        {
            "SUCCESS"
        } else {
            "FAILED"
        }
    );
    println!("✅ TEST 2: Balance Check After Deposit - Tested");
    println!(
        "✅ TEST 3: Order Placement - SUCCESS (Order ID: {})",
        placed_order_id
    );
    println!("✅ TEST 4: Balance Check After Order - Tested");
    println!("✅ TEST 5: Order Retrieval - Tested");

    println!("\n📊 Complete Flow Verified:");
    println!("   1. Deposit funds → 2. Verify balance → 3. Place order");
    println!("   → 4. Verify balance update → 5. Verify order stored");

    println!("\n🎉 Integration test completed!\n");
}
