//! lib-server/src/lib.rs
//!
//! Server initialization and application factory

use actix_web::HttpServer;
use lib_config::Config;
use std::future::Future;
use std::net::TcpListener;
use tracing::info;

mod app;
mod combined_auth;
mod correlation_handler;
mod edge;
mod metrics;
mod rate_limit;
mod state;

pub use app::create_app;
pub use combined_auth::CombinedAuthMiddleware;
pub use correlation_handler::CorrelationHandler;
pub use lib_core::{
    CorrelationService, ResponseError, generate_client_order_id, wait_for_response,
};
pub use rate_limit::RateLimiter;
pub use state::AppState;

/// Initialize the HTTP server, and the internal metrics listener beside it. The returned future runs
/// both until the HTTP server stops.
pub async fn initialize_server(
    config: &Config,
    gateway_publisher: lib_core::Publisher,
    correlation_service: CorrelationService,
) -> Result<impl Future<Output = std::io::Result<()>>, Box<dyn std::error::Error + Send + Sync>> {
    // Use the auth configuration from lib-config
    let auth_config = config.auth.clone();

    // Initialize AppState with all dependencies
    let app_state = AppState::new(
        &auth_config,
        &config.clickhouse,
        config.rate_limit,
        gateway_publisher,
        correlation_service,
    )
    .await?;
    let api_key_store = app_state.api_key_middleware.api_key_store();

    // Create TCP listener
    let listener = TcpListener::bind(config.server_address()?)?;
    info!("Server listening on {}", listener.local_addr().unwrap());
    let metrics_listener = TcpListener::bind(config.metrics.socket_addr()?)?;
    info!("Metrics listening on {}", metrics_listener.local_addr()?);

    // Create the HTTP server
    let server = HttpServer::new(move || create_app(app_state.clone(), api_key_store.clone()))
        .listen(listener)?
        .run();

    Ok(metrics::alongside(server, metrics_listener)?)
}
