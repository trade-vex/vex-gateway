//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use lib_config::Config;
use lib_server;
use tracing::{error, info};
use tracing_subscriber::{filter::LevelFilter, fmt};

/// Main entry point for the VEX Gateway Service
///
/// This function:
/// 1. Loads environment variables and configuration
/// 2. Initializes logging
/// 3. Starts the HTTP server
///
/// # Returns
///
/// Returns `std::io::Result<()>` - Ok(()) on successful server shutdown,
/// or an error if the server fails to start or encounters an error.
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load environment variables first
    dotenvy::dotenv().ok();

    // Load configuration
    let config = Config::load_auto().unwrap_or_else(|e| {
        error!(
            "Failed to load configuration: {}. Falling back to defaults.",
            e
        );
        Config::load_development_defaults()
    });

    // Validate configuration before using any config values
    if let Err(e) = config.validate() {
        error!("Configuration validation failed: {}", e);
        std::process::exit(1);
    }

    // Initialize logging
    fmt()
        .with_max_level(
            config
                .logging
                .level
                .parse::<LevelFilter>()
                .unwrap_or(LevelFilter::INFO),
        )
        .init();

    info!("Starting VEX Gateway Service");
    info!("{}", config.description());

    // Initialize and start the server
    let (_app_state, server) = lib_server::initialize_server(&config)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    info!("Server startup complete");
    server.await
}
