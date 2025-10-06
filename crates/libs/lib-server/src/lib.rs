//! lib-server/src/lib.rs
//!
//! Server initialization and application factory

use actix_web::HttpServer;
use actix_web::dev::Server;
use lib_config::Config;
use std::net::TcpListener;
use std::sync::Arc;

mod app;
mod state;
mod combined_auth;

pub use app::create_app;
pub use state::AppState;
pub use combined_auth::CombinedAuthMiddleware;

/// Initialize the HTTP server with proper configuration
pub async fn initialize_server(
    config: &Config,
    gateway_publisher: lib_core::Publisher,
) -> Result<(Arc<AppState>, Server), Box<dyn std::error::Error + Send + Sync>> {
    // Use the auth configuration from lib-config
    let auth_config = config.auth.clone();

    // Initialize AppState with all dependencies
    // Currently app state is reading from auth config , it should be reading from global config
    // Auth config can be moved to global config
    let app_state = AppState::new(&auth_config, gateway_publisher).await?;

    // Create TCP listener
    let listener = TcpListener::bind(config.server_address()?)?;
    tracing::info!("Server listening on {}", listener.local_addr().unwrap());

    // Create the HTTP server
    let app_state_clone = app_state.clone();
    let server = HttpServer::new(move || create_app(app_state_clone.clone()))
        .listen(listener)?
        .run();

    Ok((app_state, server))
}
