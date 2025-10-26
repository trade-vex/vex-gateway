//! lib-server/src/lib.rs
//!
//! Server initialization and application factory

use tracing::{info};
use actix_web::HttpServer;
use actix_web::dev::Server;
use lib_config::Config;
use std::net::TcpListener;

mod app;
mod combined_auth;
mod correlation_handler;
mod state;

pub use app::create_app;
pub use combined_auth::CombinedAuthMiddleware;
pub use correlation_handler::CorrelationHandler;
pub use lib_core::{
    CorrelationService, ResponseError, generate_client_order_id, wait_for_response,
};
pub use state::AppState;

/// Initialize the HTTP server with proper configuration
pub async fn initialize_server(
    config: &Config,
    gateway_publisher: lib_core::Publisher,
    correlation_service: CorrelationService,
) -> Result<Server, Box<dyn std::error::Error + Send + Sync>> {
    // Use the auth configuration from lib-config
    let auth_config = config.auth.clone();

    // Initialize AppState with all dependencies
    let app_state = AppState::new(&auth_config, gateway_publisher, correlation_service).await?;

    // Create TCP listener
    let listener = TcpListener::bind(config.server_address()?)?;
    info!("Server listening on {}", listener.local_addr().unwrap());

    // Create the HTTP server
    let server = HttpServer::new(move || create_app(app_state.clone()))
        .listen(listener)?
        .run();

    Ok(server)
}
