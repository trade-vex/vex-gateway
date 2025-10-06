//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use lib_config::Config;
use std::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::{filter::LevelFilter, fmt};
use vex_config::GatewayNetworkingConfig;
use vex_networking::client::{OrderCommandHandler, VexGateway};

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

    // Initialize VexGateway client and handler
    info!("Initializing VexGateway client");
    let gateway_config = GatewayNetworkingConfig::test_defaults();
    let mut gateway_client = VexGateway::new(gateway_config)
        .map_err(|e| std::io::Error::other(format!("Failed to initialize VexGateway: {}", e)))?;

    // Create channel for order responses
    let (order_sender, _order_receiver) = mpsc::channel();
    let handler = OrderCommandHandler::new(gateway_client.gateway_id(), order_sender);

    // Start the gateway client and get the publisher
    info!("Starting VexGateway client");
    let publisher = gateway_client
        .start(handler)
        .map_err(|e| std::io::Error::other(format!("Failed to start VexGateway: {}", e)))?;
    info!("VexGateway client started successfully");

    // Initialize and start the server with the publisher
    let (_app_state, server) = lib_server::initialize_server(&config, publisher)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    info!("Server startup complete");
    server.await
}
