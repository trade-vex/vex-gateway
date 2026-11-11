//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use lib_config::Config;
use lib_server::CorrelationService;
use tracing::info;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt};

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

    // Create correlation service
    let correlation_service = CorrelationService::new();
    info!("Initialized correlation service");

    // The gateway serves without vex-core: the link to it is established in the background with
    // the [core] settings, validated with the rest of the configuration, and again after a loss.
    let engine_link =
        lib_server::spawn_engine_link(config.core.networking(), correlation_service.clone())?;
    info!("Connecting to vex-core in the background");

    // Initialize and start the server with the engine link and correlation service
    let server = lib_server::initialize_server(&config, engine_link, correlation_service)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    info!("Server startup complete");
    server.await
}
