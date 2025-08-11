use actix_web::{web, HttpRequest, HttpResponse, Result, Error};
use lib_middleware::ApiKeyConfig;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct PlaceOrderRequest {
    pub symbol: String,
    pub side: String, // BUY or SELL
    pub r#type: String, // LIMIT, MARKET, etc.
    #[serde(rename = "timeInForce")]
    pub time_in_force: Option<String>,
    pub quantity: String,
    pub price: Option<String>,
    #[serde(rename = "recvWindow")]
    pub recv_window: Option<u64>,
    pub timestamp: u64,
}

#[derive(Deserialize)]
pub struct CancelOrderRequest {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: u64,
    #[serde(rename = "recvWindow")]
    pub recv_window: Option<u64>,
    pub timestamp: u64,
}

// Market Data Endpoints (NONE security)
pub async fn ping() -> Result<HttpResponse, Error> {
    Ok(HttpResponse::Ok().json(json!({})))
}

pub async fn server_time() -> Result<HttpResponse, Error> {
    let server_time = chrono::Utc::now().timestamp_millis();
    Ok(HttpResponse::Ok().json(json!({
        "serverTime": server_time
    })))
}

pub async fn exchange_info() -> Result<HttpResponse, Error> {
    Ok(HttpResponse::Ok().json(json!({
        "timezone": "UTC",
        "serverTime": chrono::Utc::now().timestamp_millis(),
        "symbols": [
            {
                "symbol": "BTCUSDT",
                "status": "TRADING",
                "baseAsset": "BTC",
                "quoteAsset": "USDT"
            }
        ]
    })))
}

// Trading Endpoints (TRADE security)
// Change from JSON to Form data to match signature middleware
pub async fn place_order(
    api_config: ApiKeyConfig, 
    req: web::Form<PlaceOrderRequest>, // Changed from web::Json to web::Form
) -> Result<HttpResponse, Error> {
    // Check trading permissions
    if !api_config.can_trade() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    // Simulate order placement
    Ok(HttpResponse::Ok().json(json!({
        "symbol": req.symbol,
        "orderId": 12345,
        "orderListId": -1,
        "clientOrderId": format!("web_{}", chrono::Utc::now().timestamp_millis()),
        "transactTime": chrono::Utc::now().timestamp_millis(),
        "price": req.price.as_deref().unwrap_or("0.00000000"),
        "origQty": req.quantity,
        "executedQty": "0.00000000",
        "cummulativeQuoteQty": "0.00000000",
        "status": "NEW",
        "timeInForce": req.time_in_force.as_deref().unwrap_or("GTC"),
        "type": req.r#type,
        "side": req.side
    })))
}

pub async fn cancel_order(
    api_config: ApiKeyConfig,
    query: web::Query<CancelOrderRequest>,
) -> Result<HttpResponse, Error> {
    if !api_config.can_trade() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    Ok(HttpResponse::Ok().json(json!({
        "symbol": query.symbol,
        "orderId": query.order_id,
        "status": "CANCELED"
    })))
}

// Account Endpoints (USER_DATA security)
pub async fn get_account_info(
    api_config: ApiKeyConfig,
) -> Result<HttpResponse, Error> {
    if !api_config.can_access_user_data() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    Ok(HttpResponse::Ok().json(json!({
        "makerCommission": 10,
        "takerCommission": 10,
        "buyerCommission": 0,
        "sellerCommission": 0,
        "canTrade": api_config.can_trade(),
        "canWithdraw": false,
        "canDeposit": false,
        "balances": [
            {
                "asset": "BTC",
                "free": "1000.00000000",
                "locked": "0.00000000"
            },
            {
                "asset": "USDT", 
                "free": "50000.00000000",
                "locked": "0.00000000"
            }
        ]
    })))
}

pub async fn get_open_orders(
    api_config: ApiKeyConfig,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> Result<HttpResponse, Error> {
    if !api_config.can_access_user_data() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    let symbol = query.get("symbol");
    Ok(HttpResponse::Ok().json(json!({
        "orders": [],
        "symbol": symbol
    })))
}

// User Stream Endpoints (USER_STREAM security)
pub async fn create_listen_key(
    api_config: ApiKeyConfig,
) -> Result<HttpResponse, Error> {
    if !api_config.can_access_user_streams() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    Ok(HttpResponse::Ok().json(json!({
        "listenKey": "pqia91ma19a5s61cv6a81va65sdf19v8a65a1a5s61cv6a81va65sdf19v8a65a1"
    })))
}

pub async fn keep_alive_listen_key(
    api_config: ApiKeyConfig,
) -> Result<HttpResponse, Error> {
    if !api_config.can_access_user_streams() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    Ok(HttpResponse::Ok().json(json!({})))
}

pub async fn close_listen_key(
    api_config: ApiKeyConfig,
) -> Result<HttpResponse, Error> {
    if !api_config.can_access_user_streams() {
        return Ok(HttpResponse::Forbidden().json(json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })));
    }

    Ok(HttpResponse::Ok().json(json!({})))
}