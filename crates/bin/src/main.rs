//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use lib_config::Config;
use lib_server::{CorrelationHandler, CorrelationService};
use tracing::info;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt};
use vex_networking::client::VexGateway;

/// Main entry point for the VEX Gateway Service
///
/// This function:
/// 1. Loads the configuration (`config/<APP_ENV>.toml` and its `APP__` overrides)
/// 2. Initializes logging
/// 3. Starts the HTTP server
///
/// # Returns
///
/// Returns `std::io::Result<()>` - Ok(()) on successful server shutdown,
/// or an error if the server fails to start or encounters an error.
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load configuration
    let config = Config::load_auto().map_err(|e| {
        eprintln!("Failed to load configuration: {e}");
        std::io::Error::other(e.to_string())
    })?;

    // Initialize logging.
    //
    // The ORM and driver targets are capped at INFO regardless of the configured application
    // level: at DEBUG/TRACE, SeaORM's instrumented query spans record the `Statement` arguments,
    // which for an API-key insert are the plaintext key and its HMAC signing secret. That path
    // bypasses the redacted `Debug` impls on ApiKeyConfig/KeyMaterial entirely, so the cap is the
    // only thing keeping credentials out of the log (finding G1-1).
    let app_level = config
        .logging
        .level
        .parse::<LevelFilter>()
        .unwrap_or(LevelFilter::INFO);
    let filter = EnvFilter::new(format!(
        "{app_level},sea_orm=info,sqlx=info,sqlx::query=warn"
    ));
    fmt().with_env_filter(filter).init();

    info!("Starting VEX Gateway Service");

    // Initialize VexGateway client
    info!("Initializing VexGateway client");

    // The [core] link settings, validated with the rest of the configuration
    let mut gateway_client = VexGateway::new(config.core.networking())
        .map_err(|e| std::io::Error::other(format!("Failed to initialize VexGateway: {}", e)))?;

    // Create correlation service
    let correlation_service = CorrelationService::new();
    info!("Initialized correlation service");

    // Create Aeron handler with correlation service
    let handler = CorrelationHandler::new(gateway_client.gateway_id(), correlation_service.clone());
    info!("Initialized handler");

    // Start the gateway client and get the publisher
    info!("Starting VexGateway with correlation handler");
    let publisher = gateway_client
        .start(handler)
        .map_err(|e| std::io::Error::other(format!("Failed to start VexGateway: {}", e)))?;
    info!("VexGateway client started successfully");

    // Initialize and start the server with publisher and correlation service
    let server = lib_server::initialize_server(&config, publisher, correlation_service)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    info!("Server startup complete");
    server.await
}
