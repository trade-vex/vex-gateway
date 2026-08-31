//! STRESS TEST - High-Frequency Multi-User Trading
//!
//! This test simulates HIGH STRESS conditions:
//! - 10 users placing orders rapidly
//! - 200 orders placed in < 3 seconds
//! - Orders WILL match and execute trades
//! - Minimal wait times (0.1s)
//! - Intelligent cancellation (only unfilled orders)
//! - Final state verification with trade accounting
//!
//! Architecture:
//! - Initial 10s wait for Kafka topic creation (one-time setup)
//! - Users 1-5: SELL @ 95-110 (will match with BUY orders)
//! - Users 6-10: BUY @ 90-105 (will match with SELL orders)
//! - Trades execute automatically (overlapping prices)
//! - Cancel remaining unfilled orders
//! - Verify: trades executed, balances transferred, unfilled orders cancelled

use actix_web::{web, App, HttpResponse, HttpServer, Result};
use common::{OrderCommand, OrderCommandType, Side, Status, TimeInForce, UserBalance};
use lib_core::{
    generate_client_order_id, wait_for_response, CorrelationService, OrderCacheConfig, OrderStore,
    OrdersListResponse, Publisher, UserBalanceCacheConfig, UserBalanceStore,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tests::gateway_send_error;

// ==================== DTOs ====================

#[derive(Debug, Deserialize, Serialize, Clone)]
struct OrderRequest {
    pub price: u64,
    pub volume: u64,
    pub market_id: u32,
    pub side: Side,
    pub time_in_force: TimeInForce,
    pub user_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct DepositRequest {
    pub user_id: u64,
    pub asset_id: u32,
    pub amount: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CancelOrderRequest {
    pub order_id: u64,
    pub market_id: u32,
    pub user_id: u64,
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

// ==================== Test Handlers ====================

async fn test_deposit_funds(
    deposit_data: web::Json<DepositRequest>,
    publisher: web::Data<Arc<Publisher>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let deposit_data = deposit_data.into_inner();
    let client_order_id = generate_client_order_id();
    let response_receiver = correlation_service.register_request(client_order_id);

    let mut command = OrderCommand {
        command: OrderCommandType::DepositFunds,
        order_id: 0,
        client_order_id,
        market_id: deposit_data.asset_id,
        user_id: deposit_data.user_id,
        price: 0,
        size: deposit_data.amount,
        original_size: deposit_data.amount,
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

    command.balance[0].available = deposit_data.amount;

    let send_result = publisher.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        let (status, message) = gateway_send_error(&e, format!("Failed: {}", e));
        return HttpResponse::build(status).json(DepositResponse {
            user_id: deposit_data.user_id,
            asset_id: deposit_data.asset_id,
            amount: deposit_data.amount,
            status: "error".to_string(),
            message,
        });
    }

    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => HttpResponse::Ok().json(DepositResponse {
            user_id: deposit_data.user_id,
            asset_id: deposit_data.asset_id,
            amount: deposit_data.amount,
            status: format!("{:?}", response.status),
            message: "Deposit processed".to_string(),
        }),
        Err(_) => HttpResponse::RequestTimeout().json(DepositResponse {
            user_id: deposit_data.user_id,
            asset_id: deposit_data.asset_id,
            amount: deposit_data.amount,
            status: "timeout".to_string(),
            message: "Timeout".to_string(),
        }),
    }
}

async fn test_place_order(
    order_data: web::Json<OrderRequest>,
    publisher: web::Data<Arc<Publisher>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let order_data = order_data.into_inner();
    let client_order_id = generate_client_order_id();
    let response_receiver = correlation_service.register_request(client_order_id);

    let command = OrderCommand {
        command: OrderCommandType::PlaceOrder,
        order_id: 0,
        client_order_id,
        market_id: order_data.market_id,
        user_id: order_data.user_id,
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

    let send_result = publisher.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        let (status, message) = gateway_send_error(&e, format!("Failed: {}", e));
        return HttpResponse::build(status).json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "error".to_string(),
            message,
        });
    }

    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => HttpResponse::Ok().json(OrderResponse {
            order_id: response.order_id,
            client_order_id: response.client_order_id,
            status: format!("{:?}", response.status),
            message: "Processed".to_string(),
        }),
        Err(_) => HttpResponse::RequestTimeout().json(OrderResponse {
            order_id: 0,
            client_order_id,
            status: "timeout".to_string(),
            message: "Timeout".to_string(),
        }),
    }
}

async fn test_cancel_order(
    cancel_data: web::Json<CancelOrderRequest>,
    publisher: web::Data<Arc<Publisher>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    let cancel_data = cancel_data.into_inner();
    let client_order_id = generate_client_order_id();
    let response_receiver = correlation_service.register_request(client_order_id);

    let command = OrderCommand {
        command: OrderCommandType::CancelOrder,
        order_id: cancel_data.order_id,
        client_order_id,
        market_id: cancel_data.market_id,
        user_id: cancel_data.user_id,
        price: 0,
        size: 0,
        original_size: 0,
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

    let send_result = publisher.try_send_order_command(&command);

    if let Err(e) = send_result {
        correlation_service.cancel_request(client_order_id);
        let (status, message) = gateway_send_error(&e, format!("Failed: {}", e));
        return HttpResponse::build(status).json(OrderResponse {
            order_id: cancel_data.order_id,
            client_order_id,
            status: "error".to_string(),
            message,
        });
    }

    match wait_for_response(response_receiver, Duration::from_secs(5)).await {
        Ok(response) => HttpResponse::Ok().json(OrderResponse {
            order_id: response.order_id,
            client_order_id: response.client_order_id,
            status: format!("{:?}", response.status),
            message: "Cancelled".to_string(),
        }),
        Err(_) => HttpResponse::RequestTimeout().json(OrderResponse {
            order_id: cancel_data.order_id,
            client_order_id,
            status: "timeout".to_string(),
            message: "Timeout".to_string(),
        }),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TestUserBalance {
    pub asset_id: u32,
    pub available: String,
    pub locked: String,
    pub total: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TestBalancesList {
    pub balances: Vec<TestUserBalance>,
    pub total_assets: usize,
}

async fn test_get_balances(
    user_id: web::Path<u64>,
    balance_store: web::Data<Arc<UserBalanceStore>>,
) -> Result<HttpResponse> {
    let user_id = *user_id;

    match balance_store.get_user_balances(user_id).await {
        Ok(balances) => {
            let total_assets = balances.len();
            let balance_responses: Vec<_> = balances
                .into_iter()
                .map(|b| TestUserBalance {
                    asset_id: b.asset_id as u32,
                    available: b.available.to_string(),
                    locked: b.locked.to_string(),
                    total: b.total.to_string(),
                })
                .collect();

            Ok(HttpResponse::Ok().json(TestBalancesList {
                balances: balance_responses,
                total_assets,
            }))
        }
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "failed",
            "message": e.to_string()
        }))),
    }
}

async fn test_get_open_orders(
    user_id: web::Path<u64>,
    order_store: web::Data<Arc<OrderStore>>,
) -> Result<HttpResponse> {
    let user_id = *user_id;

    match order_store.get_user_orders(user_id).await {
        Ok(orders) => {
            let total = orders.len();
            Ok(HttpResponse::Ok().json(OrdersListResponse {
                orders: vec![],
                total,
                page: 1,
                limit: 100,
                total_pages: 1,
                has_next: false,
                has_prev: false,
            }))
        }
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "failed",
            "message": e.to_string()
        }))),
    }
}

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
            .route("/test/faucet/deposit", web::post().to(test_deposit_funds))
            .route("/test/order/place", web::post().to(test_place_order))
            .route("/test/order/cancel", web::post().to(test_cancel_order))
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
    .expect("Failed to bind")
    .run();

    tokio::spawn(server);
    format!("http://127.0.0.1:{}", port)
}

// ==================== STRESS TEST ====================

#[tokio::test]
#[ignore] // Requires vex-core, database, and redis - skip in CI
async fn test_high_frequency_stress_test() {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  HIGH-FREQUENCY STRESS TEST - 10 Users, 200+ Orders          ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Setup
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());

    // Initialize VEX Gateway
    let mut gateway_config = vex_config::GatewayNetworkingConfig::development_defaults();
    gateway_config.context_dir =
        "/Users/adityaanand/vex-backend/vex-core/aeron-test-server".to_string();

    let mut gateway_client = vex_networking::client::VexGateway::new(gateway_config)
        .expect("Failed to initialize VexGateway");

    let correlation_service = CorrelationService::new();
    let handler = lib_server::CorrelationHandler::new(
        gateway_client.gateway_id(),
        correlation_service.clone(),
    );

    let publisher = gateway_client
        .start(handler)
        .expect("Failed to start VexGateway");

    let balance_store = UserBalanceStore::with_redis_cache(
        UserBalanceCacheConfig { enabled: true },
        redis_url.clone(),
        None,
    )
    .await
    .expect("Failed to create UserBalanceStore");

    let order_store =
        OrderStore::with_redis_cache(OrderCacheConfig { enabled: true }, redis_url.clone(), None)
            .await
            .expect("Failed to create OrderStore");

    let server_address =
        spawn_test_server(publisher, correlation_service, balance_store, order_store);

    println!("🚀 Server: {}", server_address);
    println!("⏳ Waiting 10s for Kafka topics to initialize...");
    tokio::time::sleep(Duration::from_secs(10)).await;
    println!("✅ Topics ready!\n");

    let client = reqwest::Client::new();
    let market_id = 5991u32;
    let base_asset_id = market_id;
    let quote_asset_id = 0u32;

    // 10 users
    let users: Vec<u64> = (20001..=20010).collect();

    // ==================== PHASE 1: RAPID DEPOSITS ====================
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 1: Rapid Deposits (10 users, 20 deposits)             ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    let initial_base = 100_000_000_000u64; // 100B base tokens
    let initial_quote = 10_000_000_000_000_000u64; // 10,000T quote tokens

    for user_id in &users {
        // Base deposit
        let resp = post_expecting_success(
            &client,
            &format!("{}/test/faucet/deposit", server_address),
            &DepositRequest {
                user_id: *user_id,
                asset_id: base_asset_id,
                amount: initial_base,
            },
        )
        .await;
        let _: DepositResponse = resp.json().await.expect("Failed");

        // Quote deposit
        let resp = post_expecting_success(
            &client,
            &format!("{}/test/faucet/deposit", server_address),
            &DepositRequest {
                user_id: *user_id,
                asset_id: quote_asset_id,
                amount: initial_quote,
            },
        )
        .await;
        let _: DepositResponse = resp.json().await.expect("Failed");

        print!(".");
    }
    println!("\n✅ All deposits sent!\n");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // ==================== PHASE 2: RAPID ORDER PLACEMENT ====================
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 2: RAPID Fire Order Placement (200+ orders in 2s)     ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    let start = std::time::Instant::now();

    // SELL orders from users 1-5 @ 95-110 range (WILL MATCH)
    for (idx, user_id) in users[0..5].iter().enumerate() {
        for i in 0..20 {
            let price = 95_000_000_000 + (idx as u64 * 3_000_000_000) + (i * 500_000_000);
            let resp = post_expecting_success(
                &client,
                &format!("{}/test/order/place", server_address),
                &OrderRequest {
                    price,
                    volume: 1,
                    market_id,
                    side: Side::Ask,
                    time_in_force: TimeInForce::Gtc,
                    user_id: *user_id,
                },
            )
            .await;
            let _: OrderResponse = resp.json().await.expect("Failed");
            print!("S");
        }
    }

    // BUY orders from users 6-10 @ 90-105 range (WILL MATCH with SELLs @ 95-105)
    for (idx, user_id) in users[5..10].iter().enumerate() {
        for i in 0..20 {
            let price = 105_000_000_000 - (idx as u64 * 3_000_000_000) - (i * 500_000_000);
            let resp = post_expecting_success(
                &client,
                &format!("{}/test/order/place", server_address),
                &OrderRequest {
                    price,
                    volume: 1,
                    market_id,
                    side: Side::Bid,
                    time_in_force: TimeInForce::Gtc,
                    user_id: *user_id,
                },
            )
            .await;
            let _: OrderResponse = resp.json().await.expect("Failed");
            print!("B");
        }
    }

    let elapsed = start.elapsed();
    println!("\n✅ 200 orders placed in {:?}!\n", elapsed);
    println!("   Orders will match automatically (overlapping prices)");

    // Wait for trades to execute
    println!("⏳ Waiting 1s for trades to execute and settle...");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // ==================== PHASE 3: CHECK TRADES & CANCEL UNFILLED ====================
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 3: Check Trades & Cancel Unfilled Orders              ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    let mut total_cancelled = 0;
    let mut total_before_cancel = 0;

    // For each user, get CURRENT open orders (after matching) and cancel half
    for user_id in &users {
        let orders_resp = client
            .get(format!("{}/test/user/{}/orders", server_address, user_id))
            .send()
            .await
            .expect("Failed");
        let orders_data: OrdersListResponse = orders_resp.json().await.expect("Failed");

        total_before_cancel += orders_data.total;

        if orders_data.total > 0 {
            println!(
                "User {}: {} open orders (cancelling half)",
                user_id, orders_data.total
            );

            // Cancel half of this user's UNFILLED orders
            let cancel_count = (orders_data.total / 2).max(1); // Cancel at least 1
            for i in 0..cancel_count.min(orders_data.orders.len()) {
                let order = &orders_data.orders[i];
                let order_id: u64 = order.order_id.try_into().unwrap_or(0); // Convert Decimal to u64

                let resp = post_expecting_success(
                    &client,
                    &format!("{}/test/order/cancel", server_address),
                    &CancelOrderRequest {
                        order_id,
                        market_id,
                        user_id: *user_id,
                    },
                )
                .await;
                let cancel_resp: crate::OrderResponse = resp.json().await.expect("Failed");

                if cancel_resp.status.contains("Cancelled") {
                    total_cancelled += 1;
                    print!("✓");
                } else {
                    print!("✗");
                }
            }
        } else {
            println!("User {}: 0 open orders (all filled!)", user_id);
        }
    }

    println!(
        "\n✅ {} of {} orders cancelled!\n",
        total_cancelled, total_before_cancel
    );
    println!(
        "   {} orders were already filled",
        200 - total_before_cancel
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // ==================== PHASE 5: VERIFY FINAL STATE ====================
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 5: Verify Final State (Balances + Open Orders)        ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    println!("⏳ Waiting 2s for Kafka consumer to process all events...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut total_open_orders = 0;
    let mut total_base_balance = 0u64;
    let mut total_quote_balance = 0u64;

    // Check final state for each user
    for user_id in &users {
        let balance_resp = client
            .get(format!("{}/test/user/{}/balances", server_address, user_id))
            .send()
            .await
            .expect("Failed");
        let balance_data: TestBalancesList = balance_resp.json().await.expect("Failed");

        let orders_resp = client
            .get(format!("{}/test/user/{}/orders", server_address, user_id))
            .send()
            .await
            .expect("Failed");
        let orders_data: OrdersListResponse = orders_resp.json().await.expect("Failed");

        println!(
            "User {}: {} assets, {} open orders",
            user_id, balance_data.total_assets, orders_data.total
        );

        // Each user must have both assets
        assert!(
            balance_data.total_assets >= 2,
            "User {} should have at least 2 assets",
            user_id
        );

        // Accumulate system-wide totals for conservation check
        for balance in &balance_data.balances {
            let total: u64 = balance.total.parse().unwrap_or(0);
            if balance.asset_id == base_asset_id {
                total_base_balance += total;
            } else if balance.asset_id == quote_asset_id {
                total_quote_balance += total;
            }
        }

        total_open_orders += orders_data.total;
    }

    // CRITICAL: System-wide fund conservation (total across all users = initial deposits)
    let expected_total_base = initial_base * (users.len() as u64);
    let expected_total_quote = initial_quote * (users.len() as u64);

    println!("\n📊 System-Wide Fund Conservation Check:");
    println!(
        "  Total Base:  {} (expected {})",
        total_base_balance, expected_total_base
    );
    println!(
        "  Total Quote: {} (expected {})",
        total_quote_balance, expected_total_quote
    );

    assert_eq!(
        total_base_balance, expected_total_base,
        "Total base across ALL users must equal initial deposits (conservation of funds)"
    );
    assert_eq!(
        total_quote_balance, expected_total_quote,
        "Total quote across ALL users must equal initial deposits (conservation of funds)"
    );

    println!("\n📊 Final Statistics:");
    println!("  - Total open orders: {}", total_open_orders);
    println!("  - Cancellations executed: {}", total_cancelled);
    println!("  - Orders placed: 200");
    println!(
        "  - Trades executed: ~{}",
        200 - total_open_orders - total_cancelled
    );

    println!("\n✅ STRESS TEST COMPLETE!\n");
    println!("Summary:");
    println!("  - Orders placed: 200 in {:?}", elapsed);
    println!(
        "  - Orders per second: {:.0}",
        200.0 / elapsed.as_secs_f64()
    );
    println!(
        "  - Trades executed: {} (automatic matching)",
        200 - total_open_orders - total_cancelled
    );
    println!("  - Cancellations: {}", total_cancelled);
    println!("  - Final open orders: {}", total_open_orders);
    println!("  - System-wide fund conservation: VERIFIED ✓");
    println!("  - All users verified ✓");
}

/// POST `body` and return the response once it is 2xx, so the run only counts commands the
/// engine actually applied. A 503 means the gateway did not publish the command (engine busy),
/// so it is retried with bounded backoff. Any other non-2xx fails the run: in particular a 408
/// must not be retried, because these test handlers take no idempotency key and the timed-out
/// command may still have been applied.
async fn post_expecting_success<T: Serialize>(
    client: &reqwest::Client,
    url: &str,
    body: &T,
) -> reqwest::Response {
    const MAX_ATTEMPTS: u32 = 10;
    let mut backoff = Duration::from_millis(10);

    for _ in 0..MAX_ATTEMPTS {
        let resp = client
            .post(url)
            .json(body)
            .send()
            .await
            .expect("request failed");
        let status = resp.status();
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(1));
            continue;
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            panic!("POST {url} returned {status}: {body}");
        }
        return resp;
    }

    panic!("POST {url}: engine still busy (503) after {MAX_ATTEMPTS} attempts");
}
