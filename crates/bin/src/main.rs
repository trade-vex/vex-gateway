//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use actix_web::{App, HttpResponse, HttpServer, Responder, web};
use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_auth::{
    middleware::jwt_auth::JwtAuthentication,
    service::{oauth_service::OAuthService, token_service::TokenService},
    state::AppState, // Use the AppState from lib-auth
};
use lib_config::Config;
use std::net::TcpListener;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::{filter::LevelFilter, fmt};
// The ServerState now just holds the services.
struct ServerState {
    oauth_service: OAuthService,
    // The token service is now encapsulated within AppState and OAuthService
}

// Import your auth handlers
mod auth_handlers {
    pub use lib_auth::handlers::auth::*;
}
//  Handlers

async fn auth_redirect_handler(
    provider: web::Path<String>,
    state: web::Data<ServerState>,
) -> impl Responder {
    tracing::info!("OAuth redirect request for provider: {}", provider);

    match state.oauth_service.get_auth_url(&provider).await {
        Ok(url) => {
            tracing::info!("Generated OAuth URL: {}", url);
            HttpResponse::Found()
                .append_header(("Location", url))
                .finish()
        }
        Err(e) => {
            tracing::error!("Failed to get auth URL: {}", e);
            HttpResponse::InternalServerError().body(format!("OAuth error: {}", e))
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
///
/// This handler is protected by JWT authentication and
/// returns a greeting message with the current timestamp
async fn me_handler(user: AuthenticatedUser) -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "message": "Hello authenticated user!",
        "user_id": user.user_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "greeting": format!("Welcome, user {}!", user.user_id)
    }))
}

/// Main entry point for the VEX Gateway Service
///
/// This function:
/// 1. Loads the server configuration automatically (finds config files anywhere in parent directories)
/// 2. Binds to the configured address and port
/// 3. Starts the HTTP server and begins listening for requests
///
/// # Returns
///
/// Returns `std::io::Result<()>` - Ok(()) on successful server shutdown,
/// or an error if the server fails to start or encounters an error.
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load environment variables FIRST
    dotenvy::dotenv().ok();

    let config = Config::load_auto().unwrap_or_else(|e| {
        eprintln!("Failed to load configuration: {}", e);
        eprintln!("Falling back to development defaults");
        Config::load_development_defaults()
    });

    // Validate configuration before using any config values
    if let Err(e) = config.validate() {
        eprintln!("Configuration validation failed: {}", e);
        std::process::exit(1);
    }

    fmt()
        .with_max_level(
            config
                .logging
                .level
                .parse::<LevelFilter>()
                .unwrap_or(LevelFilter::INFO),
        )
        .init();

    //  Create proper auth config with OAuth providers
    let auth_config = create_auth_config_with_oauth();

    // Initialize AppState
    let app_state = AppState::new(auth_config)
        .await
        .expect("Failed to initialize auth state");

    // Create TokenService
    let token_service: Arc<TokenService> = Arc::new(TokenService::new(
        Arc::new(app_state.jwt_config.clone()),
        app_state.redis_pool.clone(),
        Arc::new(app_state.encoding_key.clone()),
        Arc::new(app_state.decoding_key.clone()),
    ));

    // Create OAuthService
    let oauth_service: OAuthService = OAuthService::new(app_state.clone(), token_service.clone());

    // Bundle services into server state
    let server_state = web::Data::new(ServerState { oauth_service });

    let listener = TcpListener::bind(config.server_address().unwrap())?;
    info!("Server listening on {}", listener.local_addr().unwrap());

    // Create JWT middleware with proper configuration
    let jwt_middleware = JwtAuthentication::new(
        app_state.decoding_key.clone(),
        "vex-gateway".to_string(), // issuer
        "vex-api".to_string(),     // audience
    );

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(app_state.clone())) // For lib-auth functions
            .app_data(server_state.clone()) // For custom OAuth handlers
            // Public routes (no authentication required)
            .service(
                web::scope("/auth")
                    // Your custom OAuth handlers
                    .route("/{provider}", web::get().to(auth_redirect_handler))
                    .route("/{provider}/callback", web::get().to(auth_callback_handler)),
            )
            // Public auth API routes (login, refresh, etc.)
            .service(
                web::scope("/api/auth").configure(auth_handlers::configure), // login, logout, refresh endpoints
            )
            // Protected API routes (require JWT authentication)
            .service(
                web::scope("/api")
                    .wrap(jwt_middleware.clone())
                    .route("/me", web::get().to(me_handler))
                    // Add more protected routes here
                    .route("/profile", web::get().to(auth_handlers::get_profile)),
            )
            // Health check (public)
            .route(
                "/health",
                web::get().to(|| async {
                    HttpResponse::Ok().json(serde_json::json!({
                        "status": "healthy",
                        "timestamp": chrono::Utc::now().to_rfc3339()
                    }))
                }),
            )
    })
    .listen(listener)?
    .run()
    .await
}

// Update the create_auth_config_with_oauth function to use the correct redirect URI

fn create_auth_config_with_oauth() -> lib_auth::config::AuthConfig {
    use lib_auth::config::{
        AuthConfig, DatabaseConfig, EmailConfig, JwtConfig, OAuthConfig, OAuthProviderConfig,
        RedisConfig, ServerConfig,
    };
    use std::collections::HashMap;
    use std::env;

    // Create OAuth providers map
    let mut providers = HashMap::new();

    // Google OAuth provider
    let google_client_id = env::var("GOOGLE_CLIENT_ID")
        .or_else(|_| env::var("APP_PROVIDERS__GOOGLE__CLIENT_ID"))
        .expect("GOOGLE_CLIENT_ID not set in environment");
    let google_client_secret = env::var("GOOGLE_CLIENT_SECRET")
        .or_else(|_| env::var("APP_PROVIDERS__GOOGLE__CLIENT_SECRET"))
        .expect("GOOGLE_CLIENT_SECRET not set in environment");

    providers.insert(
        "google".to_string(),
        OAuthProviderConfig {
            client_id: google_client_id,
            client_secret: google_client_secret,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            user_info_url: "https://www.googleapis.com/oauth2/v2/userinfo".to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            response_type: "code".to_string(),
        },
    );

    // GitHub OAuth provider
    if let (Ok(github_client_id), Ok(github_client_secret)) = (
        env::var("GITHUB_CLIENT_ID"),
        env::var("GITHUB_CLIENT_SECRET"),
    ) {
        providers.insert(
            "github".to_string(),
            OAuthProviderConfig {
                client_id: github_client_id,
                client_secret: github_client_secret,
                auth_url: "https://github.com/login/oauth/authorize".to_string(),
                token_url: "https://github.com/login/oauth/access_token".to_string(),
                user_info_url: "https://api.github.com/user".to_string(),
                // Update scopes to request email access and user profile
                scopes: vec![
                    "user".to_string(),       // Access to user profile info
                    "user:email".to_string(), // Access to private email addresses
                ],
                response_type: "code".to_string(),
            },
        );
    } else {
        tracing::warn!("GitHub OAuth credentials not found in environment variables");
    }

    AuthConfig {
        server: ServerConfig {
            base_url: env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8081".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()
                .unwrap_or(8081),
        },
        database: DatabaseConfig {
            url: env::var("DATABASE_URL").expect("DATABASE_URL not set in environment"),
            max_connections: 10,
        },
        redis: RedisConfig {
            url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            max_connections: 10,
        },
        jwt: JwtConfig {
            private_key_path: env::var("JWT_PRIVATE_KEY_PATH")
                .unwrap_or_else(|_| "./certs/jwt_private_key.pem".to_string()),
            public_key_path: env::var("JWT_PUBLIC_KEY_PATH")
                .unwrap_or_else(|_| "./certs/jwt_public_key.pem".to_string()),
            issuer: env::var("JWT_ISSUER").unwrap_or_else(|_| "vex-gateway".to_string()),
            audience: env::var("JWT_AUDIENCE").unwrap_or_else(|_| "vex-api".to_string()),
            access_token_ttl_minutes: env::var("JWT_ACCESS_TOKEN_TTL_MINUTES")
                .unwrap_or_else(|_| "15".to_string())
                .parse()
                .unwrap_or(15),
            refresh_token_ttl_days: env::var("JWT_REFRESH_TOKEN_TTL_DAYS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap_or(30),
        },
        oauth: OAuthConfig { providers },
        email: EmailConfig {
            smtp_host: env::var("SMTP_HOST").unwrap_or_else(|_| "smtp.gmail.com".to_string()),
            smtp_port: env::var("SMTP_PORT")
                .unwrap_or_else(|_| "587".to_string())
                .parse()
                .unwrap_or(587),
            smtp_username: env::var("SMTP_USERNAME")
                .unwrap_or_else(|_| "test@example.com".to_string()),
            smtp_password: env::var("SMTP_PASSWORD").unwrap_or_else(|_| "password".to_string()),
            from_email: env::var("FROM_EMAIL")
                .unwrap_or_else(|_| "noreply@vex-gateway.com".to_string()),
        },
    }
}
