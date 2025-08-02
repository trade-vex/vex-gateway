# API Key Middleware

## Security Types

- **NONE**: Endpoint can be accessed freely (no API key required)
- **TRADE**: Endpoint requires a valid API key and signature  
- **USER_DATA**: Endpoint requires a valid API key and signature
- **USER_STREAM**: Endpoint requires a valid API key (no signature required)

## Signature Algorithms

### 1. HMAC SHA256 
```rust
let api_key = ApiKeyConfig::new("your-api-key".to_string())
    .with_hmac_secret("your-secret-key".to_string());
```

### 2. RSA PKCS#1 v1.5 with SHA-256
```rust
let rsa_public_key = r#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...
-----END PUBLIC KEY-----"#;

let api_key = ApiKeyConfig::new("your-api-key".to_string())
    .with_rsa_public_key(rsa_public_key.to_string());
```

### 3. Ed25519 
```rust
let ed25519_public_key = r#"-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEA...
-----END PUBLIC KEY-----"#;

let api_key = ApiKeyConfig::new("your-api-key".to_string())
    .with_ed25519_public_key(ed25519_public_key.to_string());
```

## Complete Usage Example

```rust
use actix_web::{web, App, HttpServer, HttpResponse};
use lib_middleware::{
    ApiKeyMiddleware, SecurityType, ApiKeyConfig, RouteSecurityConfig,
    KeyMaterial
};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Configure API keys with different algorithms
    let api_keys = vec![
        // HMAC API Key for trading
        ApiKeyConfig::new("hmac-key-123".to_string())
            .with_hmac_secret("super-secret-key".to_string())
            .with_permissions(vec![SecurityType::Trade, SecurityType::UserData])
            .with_recv_window(5000)
            .with_ip_whitelist(vec!["192.168.1.0/24".to_string()])
            .with_description("Trading API Key".to_string()),
            
        // RSA API Key for user data
        ApiKeyConfig::new("rsa-key-456".to_string())
            .with_rsa_public_key(include_str!("rsa_public_key.pem").to_string())
            .with_permissions(vec![SecurityType::UserData])
            .with_recv_window(10000),
            
        // Ed25519 API Key (best performance)
        ApiKeyConfig::new("ed25519-key-789".to_string())
            .with_ed25519_public_key(include_str!("ed25519_public_key.pem").to_string())
            .with_permissions(vec![SecurityType::Trade, SecurityType::UserData, SecurityType::UserStream])
            .with_recv_window(5000),
    ];

    // Configure route-specific security
    let route_configs = vec![
        RouteSecurityConfig {
            path_pattern: "/api/v3/order".to_string(),
            security_type: SecurityType::Trade,
        },
        RouteSecurityConfig {
            path_pattern: "/api/v3/account".to_string(),
            security_type: SecurityType::UserData,
        },
        RouteSecurityConfig {
            path_pattern: "/api/v3/userDataStream".to_string(),
            security_type: SecurityType::UserStream,
        },
        RouteSecurityConfig {
            path_pattern: "/api/v3/ping".to_string(),
            security_type: SecurityType::None,
        },
    ];

    HttpServer::new(move || {
        App::new()
            .wrap(
                ApiKeyMiddleware::new()
                    .with_api_keys(api_keys.clone())
                    .with_route_configs(route_configs.clone())
                    .with_strict_timestamp_validation(true)
            )
            .service(
                web::scope("/api/v3")
                    .route("/ping", web::get().to(ping))
                    .route("/order", web::post().to(place_order))
                    .route("/account", web::get().to(get_account))
                    .route("/userDataStream", web::post().to(create_user_stream))
            )
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}

async fn ping() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({}))
}

async fn place_order() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "symbol": "BTCUSDT",
        "orderId": 123456,
        "status": "FILLED"
    }))
}

async fn get_account() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "balances": []
    }))
}

async fn create_user_stream() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "listenKey": "pqia91ma19a5s61cv6a81va65sdf19v8a65a1a5s61cv6a81va65sdf19v8a65a1"
    }))
}
```

## Request Examples

### 1. HMAC SHA256 Signed Request 

```bash
# Generate timestamp
timestamp=$(date +%s000)

# Create parameters
symbol="LTCBTC"
side="BUY"
type="LIMIT"
timeInForce="GTC"
quantity="1"
price="0.1"
recvWindow="5000"

# Create query string
query_string="symbol=${symbol}&side=${side}&type=${type}&timeInForce=${timeInForce}&quantity=${quantity}&price=${price}&recvWindow=${recvWindow}&timestamp=${timestamp}"

# Generate HMAC SHA256 signature
signature=$(echo -n "$query_string" | openssl dgst -sha256 -hmac "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j" | cut -d' ' -f2)

# Make request as query string
curl -H "X-MBX-APIKEY: vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A" \
     -X POST \
     "http://localhost:8080/api/v3/order?${query_string}&signature=${signature}"
```

### 2. RSA Signed Request 

```bash
# Generate timestamp
timestamp=$(date +%s000)

# Create parameters
params="symbol=BTCUSDT&side=SELL&type=LIMIT&timeInForce=GTC&quantity=1&price=0.2&timestamp=${timestamp}&recvWindow=5000"

# Sign with RSA private key
signature=$(echo -n "$params" | openssl dgst -sha256 -sign ./test-prv-key.pem | openssl enc -base64 -A)

# URL encode the signature
encoded_signature=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$signature'))")

# Make request
curl -H "X-MBX-APIKEY: CAvIjXy3F44yW6Pou5k8Dy1swsYDWJZLeoK2r8G4cFDnE9nosRppc2eKc1T8TRTQ" \
     -X POST \
     "http://localhost:8080/api/v3/order?${params}&signature=${encoded_signature}"
```

### 3. Mixed Query/Body Request 

```bash
# Generate timestamp  
timestamp=$(date +%s000)

# Query parameters
query="symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC"

# Body parameters
body="quantity=1&price=0.1&recvWindow=5000&timestamp=${timestamp}"

# Create signature payload (concatenated without &)
payload="${query}${body}"

# Generate signature
signature=$(echo -n "$payload" | openssl dgst -sha256 -hmac "your-secret-key" | cut -d' ' -f2)

# Make request
curl -H "X-MBX-APIKEY: your-api-key" \
     -H "Content-Type: application/x-www-form-urlencoded" \
     -X POST \
     "http://localhost:8080/api/v3/order?${query}" \
     -d "${body}&signature=${signature}"
```

### 4. USER_STREAM Request (No Signature)

```bash
curl -H "X-MBX-APIKEY: your-api-key" \
     -X POST \
     http://localhost:8080/api/v3/userDataStream
```

## Advanced Configuration

### API Key with All Features

```rust
use chrono::{Utc, Duration};

let advanced_key = ApiKeyConfig::new("advanced-key".to_string())
    .with_hmac_secret("secret".to_string())
    .with_permissions(vec![SecurityType::Trade])
    .with_recv_window(3000)
    .with_enabled(true)
    .with_description("Advanced trading key".to_string())
    .with_expiration(Utc::now() + Duration::days(30))
    .with_ip_whitelist(vec![
        "192.168.1.100".to_string(),
        "10.0.0.0/8".to_string(),
    ]);
```



## Error Codes 

- **-2014**: API key format invalid / API key invalid
- **-2015**: Invalid API key, IP, or permissions for action
- **-1021**: Timestamp outside of recv_window
- **-1022**: Signature verification failed

## Signature Generation Helper

```rust
use lib_middleware::{SignatureVerifier, SignatureAlgorithm};
use std::collections::HashMap;

// Build parameters
let mut params = HashMap::new();
params.insert("symbol".to_string(), "BTCUSDT".to_string());
params.insert("side".to_string(), "BUY".to_string());
params.insert("timestamp".to_string(), "1640995200000".to_string());

// Generate payload
let payload = SignatureVerifier::build_payload(&params, &HashMap::new());

// Generate HMAC signature
let signature = SignatureVerifier::generate_hmac(&payload, "your-secret-key")?;

// URL encode if needed
let encoded_signature = SignatureVerifier::url_encode_signature(&signature);
```


## Testing

```bash
# Run tests
cargo test

# Test with different algorithms
cargo test test_hmac_signature
cargo test test_rsa_signature  
cargo test test_ed25519_signature

# Load testing
cargo test --release -- --nocapture test_performance
```