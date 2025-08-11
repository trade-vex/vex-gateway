//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use actix_web::{web, App, HttpServer, Result, HttpResponse, Responder};
use lib_auth::{AuthConfig, AppState, middleware::jwt_auth::{JwtAuthentication, AuthenticatedUser}};
use lib_auth::service::{oauth_service::OAuthService, token_service::TokenService};
use lib_middleware::{ApiKeyMiddleware, SecurityType, RouteSecurityConfig, ApiKeyConfig};
use lib_api::{trading_routes, user_routes, health_check_route};
use std::sync::Arc;
use tracing::{error, info};

// The ServerState now just holds the services.
struct ServerState {
    oauth_service: OAuthService,
}

//  OAuth Handlers 

async fn auth_redirect_handler(
    provider: web::Path<String>,
    state: web::Data<ServerState>,
) -> impl Responder {
    info!("OAuth redirect request for provider: {}", provider);

    match state.oauth_service.get_auth_url(&provider).await {
        Ok(url) => {
            info!("Generated OAuth URL: {}", url);
            HttpResponse::Found()
                .append_header(("Location", url))
                .finish()
        }
        Err(e) => {
            error!("Failed to get auth URL: {}", e);
            HttpResponse::InternalServerError().body(format!("OAuth error: {e}"))
        }
    }
}

async fn auth_callback_handler(
    provider: web::Path<String>,
    query: web::Query<serde_json::Value>,
    state: web::Data<ServerState>,
) -> impl Responder {
    let code = query.get("code").and_then(|v| v.as_str());
    let state_param = query.get("state").and_then(|v| v.as_str());

    if code.is_none() || state_param.is_none() {
        return HttpResponse::BadRequest().body("Missing code or state");
    }

    match state
        .oauth_service
        .handle_callback(&provider, code.unwrap(), state_param.unwrap())
        .await
    {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            error!("Callback error: {}", e);
            HttpResponse::InternalServerError().body(e.to_string())
        }
    }
}

/// Handler for the `/me` endpoint
async fn me_handler(user: AuthenticatedUser) -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "message": "Hello authenticated user!",
        "user_id": user.user_id,
        "email": user.email,
        "username": user.username,
        "auth_provider": user.auth_provider,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "greeting": format!("Welcome, user {}!", user.user_id)
    }))
}

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables
    dotenvy::dotenv().ok();
    
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Use AuthConfig directly (bypassing lib-config for now)
    let auth_config = AuthConfig::init()?;
    println!("🔧 Loaded auth configuration");
    
    // AppState::new() already returns Arc<AppState>, so don't wrap it again
    let app_state = AppState::new(auth_config.clone()).await?;

    // Create TokenService from AppState components
    let token_service = Arc::new(TokenService::new(
        Arc::new(app_state.jwt_config.clone()),
        app_state.redis_pool.clone(),
        Arc::new(app_state.encoding_key.clone()),
        Arc::new(app_state.decoding_key.clone()),
    ));

    // Create OAuthService - app_state is already Arc<AppState>
    let oauth_service = OAuthService::new(app_state.clone(), token_service.clone());

    // Create ServerState with web::Data::new() OUTSIDE the closure
    let server_state = web::Data::new(ServerState {
        oauth_service,
    });

    // JWT middleware
    let jwt_middleware = JwtAuthentication::new(
        app_state.decoding_key.clone(),
        auth_config.jwt.issuer.clone(),
        auth_config.jwt.audience.clone(),
    );

    // API key middleware setup
    let api_keys = vec![
        ApiKeyConfig::new("vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A".to_string())
            .with_hmac_secret("NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j".to_string())
            .with_permissions(vec![SecurityType::Trade, SecurityType::UserData, SecurityType::UserStream])
            .with_recv_window(5000)
            .with_description("Full trading access".to_string()),
    ];

    let route_configs = vec![
        RouteSecurityConfig { path_pattern: "/api/v3/ping".to_string(), security_type: SecurityType::None },
        RouteSecurityConfig { path_pattern: "/api/v3/time".to_string(), security_type: SecurityType::None },
        RouteSecurityConfig { path_pattern: "/api/v3/exchangeInfo".to_string(), security_type: SecurityType::None },
        RouteSecurityConfig { path_pattern: "/api/v3/order".to_string(), security_type: SecurityType::Trade },
        RouteSecurityConfig { path_pattern: "/api/v3/account".to_string(), security_type: SecurityType::UserData },
        RouteSecurityConfig { path_pattern: "/api/v3/openOrders".to_string(), security_type: SecurityType::UserData },
        RouteSecurityConfig { path_pattern: "/api/v3/userDataStream".to_string(), security_type: SecurityType::UserStream },
    ];

    let api_key_middleware = ApiKeyMiddleware::new()
        .with_api_keys(api_keys)
        .with_route_configs(route_configs)
        .with_strict_timestamp_validation(true);

    let port = auth_config.server.port;
    let host = "127.0.0.1";
    
    println!("🚀 Starting VEX Gateway on http://{host}:{port}");

    HttpServer::new(move || {
        App::new()
            //  Register AppState for lib-auth functions
            .app_data(web::Data::new(app_state.clone()))
            //  Register ServerState for custom OAuth handlers (using clone, not new)
            .app_data(server_state.clone())
            
            // Public health check
            .service(
                web::scope("/public")
                    .configure(health_check_route::configure)
            )
            
            // OAuth authentication flow - using your sophisticated handlers
            .service(
                web::scope("/auth")
                    .route("/{provider}", web::get().to(auth_redirect_handler))
                    .route("/{provider}/callback", web::get().to(auth_callback_handler))
            )
            
            // User-facing web API (JWT authentication)
            .service(
                web::scope("/api/user")
                    .wrap(jwt_middleware.clone())
                    .route("/me", web::get().to(me_handler))
                    .configure(user_routes::configure)
            )
            
            // Trading API (API key authentication) - Binance-compatible
            .service(
                web::scope("/api/v3")
                    .wrap(api_key_middleware.clone())
                    .configure(trading_routes::configure)
            )
    })
    .bind(format!("{host}:{port}"))?
    .run()
    .await?;

    Ok(())
}
