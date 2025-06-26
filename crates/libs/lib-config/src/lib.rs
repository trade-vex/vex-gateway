//! lib-config/src/lib.rs
//!
//! Configuration management for the VEX Gateway Service.
//! This module handles loading server configuration from environment variables,
//! .env files, and provides sensible defaults.

use serde::Deserialize;
use std::net::SocketAddr;

/// Main configuration structure for the VEX Gateway Service
///
/// Contains all configuration settings needed to run the service.
/// Currently includes server configuration, but can be extended
/// to include database, logging, and other service configurations.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Server-specific configuration settings
    pub server: ServerConfig,
}

/// Server configuration settings
///
/// Defines the network interface and port that the server will bind to.
#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// The host address to bind the server to (e.g., "127.0.0.1", "0.0.0.0")
    pub host: String,
    /// The port number to bind the server to
    pub port: u16,
}

impl Config {
    /// Loads configuration from environment variables and .env files
    ///
    /// This method attempts to load configuration using the `config` crate,
    /// which supports loading from environment variables with the "_" separator.
    /// For example, `SERVER_HOST` becomes `server.host` in the config.
    ///
    /// # Returns
    ///
    /// Returns `Result<Self, config::ConfigError>` - Ok(Config) on successful load,
    /// or an error if configuration loading fails.
    pub fn load() -> Result<Self, config::ConfigError> {
        // Load .env file if it exists in the current directory
        let _ = dotenv::dotenv();

        // Build configuration from environment variables
        // Uses "_" as separator, so SERVER_HOST becomes server.host
        let config = config::Config::builder()
            .add_source(config::Environment::default().separator("_"))
            .build()?;

        // Deserialize the configuration into our Config struct
        config.try_deserialize()
    }

    /// Loads configuration with fallback to sensible defaults
    ///
    /// This method first attempts to load configuration using `load()`.
    /// If that fails, it falls back to reading individual environment variables
    /// with hardcoded defaults as a last resort.
    ///
    /// Environment variable precedence:
    /// 1. SERVER_HOST (primary) or HOST (fallback) → defaults to "127.0.0.1"
    /// 2. SERVER_PORT (primary) or PORT (fallback) → defaults to 8000
    ///
    /// # Returns
    ///
    /// Returns `Self` - Always succeeds, using defaults if no configuration is found.
    pub fn load_with_defaults() -> Self {
        // Load .env file if it exists
        let _ = dotenv::dotenv();

        // Try to load configuration, fall back to environment variables if it fails
        Self::load().unwrap_or_else(|_| {
            // Use environment variables with fallbacks, not hardcoded values
            let host = std::env::var("SERVER_HOST").unwrap_or_else(|_| {
                std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
            });

            // Parse port from environment variable, with fallback to default
            let port = std::env::var("SERVER_PORT")
                .unwrap_or_else(|_| std::env::var("PORT").unwrap_or_else(|_| "8000".to_string()))
                .parse()
                .unwrap_or(8000);

            // Create configuration with loaded/fallback values
            Self {
                server: ServerConfig { host, port },
            }
        })
    }

    /// Converts the server configuration to a SocketAddr
    ///
    /// Combines the host and port from the server configuration
    /// into a format suitable for binding a TCP listener.
    ///
    /// # Returns
    ///
    /// Returns `SocketAddr` - The parsed socket address for server binding.
    ///
    /// # Panics
    ///
    /// Panics if the host and port combination cannot be parsed as a valid socket address.
    pub fn server_address(&self) -> SocketAddr {
        // Format host:port and parse as SocketAddr
        format!("{}:{}", self.server.host, self.server.port)
            .parse()
            .expect("Invalid server address")
    }
}
