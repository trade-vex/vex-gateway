//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use lib_config::Config;
use lib_server::run;
use std::net::TcpListener;
use tracing::info;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;

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
#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Load server configuration automatically
    // This will find config files in the current directory or any parent directory
    let config = Config::load_auto().unwrap_or_else(|e| {
        eprintln!("Failed to load configuration: {}", e);
        eprintln!("Falling back to development defaults");
        Config::load_development_defaults()
    });

    // Parse log level from config
    let log_level = config
        .logging
        .level
        .parse::<LevelFilter>()
        .unwrap_or(LevelFilter::INFO);
    // Initialize tracing subscriber with dynamic log level from config
    fmt().with_max_level(log_level).init();

    let address = config.server_address().unwrap_or_else(|e| {
        eprintln!("Failed to get server address: {}", e);
        std::process::exit(1);
    });

    // Bind to the configured address and port
    // This creates a TCP listener that will accept incoming connections
    let listener = TcpListener::bind(address).unwrap_or_else(|e| {
        eprintln!("Failed to bind to {}: {}", address, e);
        std::process::exit(1);
    });

    // Log that the server is starting
    info!("Server listening on {}", address);
    info!("Environment: {:?}", config.environment);
    info!("Log level: {}", config.logging.level);

    // Start the HTTP server and wait for it to complete
    // The server will handle incoming requests until it's shut down
    run(listener)?.await
}
