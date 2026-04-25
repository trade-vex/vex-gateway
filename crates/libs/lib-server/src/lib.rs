//! lib-server/src/lib.rs
//!
//! Server initialization and application factory

use actix_web::HttpServer;
use actix_web::dev::Server;
use lib_config::Config;
use std::time::Duration;
use tracing::info;
use utoipa::OpenApi;

const SERVER_BACKLOG: u32 = 8192;

mod app;
mod combined_auth;
mod correlation_handler;
mod docs;
mod request_guard;
mod state;

pub use app::create_app;
pub use combined_auth::CombinedAuthMiddleware;
pub use correlation_handler::CorrelationHandler;
pub use lib_core::{
    CorrelationService, ResponseError, generate_client_order_id, wait_for_response,
};
pub use state::AppState;

pub fn openapi_document() -> utoipa::openapi::OpenApi {
    docs::ApiDoc::openapi()
}

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

    let server_addr = config.server_address()?;
    info!("Server listening on {}", server_addr);

    // Pin the worker count explicitly. Actix otherwise spawns one worker per
    // logical CPU, which on a 32-core host over-subscribes with the Aeron
    // client, Kafka consumer, and sea-orm pool threads we already run. See
    // `ServerConfig::effective_workers` for the resolution rules.
    let workers = config.server.effective_workers();
    let request_limits = request_guard::RequestLimits::default_limits();
    // Clamp the slow-client deadline so a stuck TCP peer can't occupy a
    // worker indefinitely. This complements the Tokio-level timeouts we
    // apply to downstream calls (Redis, Postgres, ClickHouse).
    let client_request_timeout =
        Duration::from_secs(config.server.client_request_timeout_secs.max(1));
    let keep_alive = Duration::from_secs(config.server.keep_alive_secs.max(1));
    info!(
        "HTTP server: workers={}, backlog={}, client_request_timeout={}s, keep_alive={}s",
        workers,
        SERVER_BACKLOG,
        client_request_timeout.as_secs(),
        keep_alive.as_secs()
    );

    let server = HttpServer::new(move || create_app(app_state.clone(), request_limits.clone()))
        .workers(workers)
        .backlog(SERVER_BACKLOG)
        .client_request_timeout(client_request_timeout)
        .keep_alive(keep_alive)
        .bind(server_addr)?
        .run();

    Ok(server)
}
