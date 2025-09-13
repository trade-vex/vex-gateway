//! main.rs
//!
//! This is the main entry point for the VEX Gateway Service.
//! It initializes the server configuration and starts the HTTP server.

use lib_config::Config;
use lib_server::run;
use std::net::TcpListener;

/// Main entry point for the VEX Gateway Service
///
/// This function:
/// 1. Loads the server configuration from environment variables or defaults
/// 2. Binds to the configured address and port
/// 3. Starts the HTTP server and begins listening for requests
///
/// # Returns
///
/// Returns `std::io::Result<()>` - Ok(()) on successful server shutdown,
/// or an error if the server fails to start or encounters an error.
#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Load server configuration with  defaults
    let config = Config::load_with_defaults();
    let address = config.server_address();

    // Bind to the configured address and port
    // This creates a TCP listener that will accept incoming connections
    let listener = TcpListener::bind(&address)
        .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", address, e));

    // Log that the server is starting
    println!("Server listening on {}", address);
    // Start the HTTP server and wait for it to complete
    // The server will handle incoming requests until it's shut down
    run(listener)?.await
}
