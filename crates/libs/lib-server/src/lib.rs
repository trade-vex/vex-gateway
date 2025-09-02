//! lib-server/src/lib.rs
//!
//! Server initialization and application factory

pub mod combined_auth;

use actix_web::{
    dev::Server,
    web, App, HttpResponse, HttpServer,
};
use lib_api::routes;
use lib_auth::{
    handlers::{auth, oauth},
    middleware::jwt_auth::{AuthenticatedUser, JwtAuthentication},
    state::AppState,
};
use lib_config::Config;
use lib_middleware::{api_key::middleware::ApiKeyMiddleware, repository::api_key_repository::PostgresApiKeyRepository,};
use crate::combined_auth::CombinedAuthMiddleware;
use std::net::TcpListener;
use std::sync::Arc;

/// Application factory that creates the Actix-web application with all routes and middleware
pub fn create_app(
    app_state: Arc<AppState>,
    combined_auth: CombinedAuthMiddleware,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    App::new()
        .app_data(web::Data::new(app_state.clone()))
        // Public OAuth routes
        .service(
            web::scope("/auth/oauth")
                .route("/{provider}", web::get().to(oauth::initiate_oauth))
                .route(
                    "/{provider}/callback",
                    web::get().to(oauth::handle_oauth_callback),
                )
                .route("/providers", web::get().to(oauth::get_oauth_providers)),
        )
        // Public auth API routes (login, refresh, etc.)
        .service(web::scope("/api/auth").configure(auth::configure))
        // Protected API routes (require JWT authentication)
        .service(
            web::scope("/api")
                .wrap(combined_auth.clone())
                .route("/me", web::get().to(me_handler))
                .route("/profile", web::get().to(auth::get_profile)),
        )
        // Health check (public)
        .route("/health", web::get().to(health_check))
        // Other API routes
        .service(web::scope("").configure(routes::health_check_route::configure))
        // Orders routes with combined authentication
        .service(
            web::scope("")
                .wrap(combined_auth.clone())
                .configure(routes::orders::configure)
        )
}

/// Health check handler
async fn health_check() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

/// Protected user info endpoint
async fn me_handler(user: AuthenticatedUser) -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "message": "Hello authenticated user!",
        "user_id": user.user_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Initialize the HTTP server with proper configuration
pub async fn initialize_server(config: &Config) -> Result<(Arc<AppState>, Server), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize AppState with auth configuration
    let app_state = AppState::new(config.auth.clone())
        .await
        .map_err(|e| format!("Failed to initialize auth state: {}", e))?;

    // Create JWT middleware
    let jwt_middleware = JwtAuthentication::new(
        app_state.decoding_key.clone(),
        config.auth.jwt.issuer.clone(),
        config.auth.jwt.audience.clone(),
    );

    let api_key_repository: Arc<dyn lib_middleware::repository::api_key_repository::ApiKeyRepository> = 
        Arc::new(PostgresApiKeyRepository::new(app_state.db.clone()));

    let api_key_middleware = ApiKeyMiddleware::new()
        .with_repository(api_key_repository)
        .with_default_security(lib_middleware::api_key::security::SecurityType::Trade)
        .with_strict_timestamp_validation(true);

    let combined_auth = CombinedAuthMiddleware::new(
        jwt_middleware,
        api_key_middleware,
    );

    // Create TCP listener
    let listener = TcpListener::bind(config.server_address()?)?;
    tracing::info!("Server listening on {}", listener.local_addr().unwrap());

    // Create the HTTP server
    let app_state_clone = app_state.clone();
    let combined_auth_clone = combined_auth.clone();
    
    let server = HttpServer::new(move || {
        create_app(app_state_clone.clone(), combined_auth_clone.clone())
    })
    .listen(listener)?
    .run();

    Ok((app_state, server))
}

/// Legacy function for backward compatibility
pub fn run(listener: TcpListener) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(|| {
        App::new()
            .service(web::scope("").configure(routes::health_check_route::configure))
            .service(web::scope("").configure(routes::orders::configure))
    })
    .listen(listener)?
    .run();
    Ok(server)
}
