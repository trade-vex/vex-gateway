//! lib-config/src/server.rs

use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, ToSocketAddrs};
use std::env;

/// Server configuration settings for the VEX Gateway Service.
///
/// Contains all network-related configuration including host, port,
/// connection limits, and timeout settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// The host address to bind the server to (e.g., "127.0.0.1", "0.0.0.0")
    pub host: String,
    /// The port number to bind the server to (0 for ephemeral port in testing)
    pub port: u16,
    /// Maximum number of concurrent connections the server will accept
    pub max_connections: usize,
    /// Connection timeout in seconds (how long to wait for new connections)
    pub connection_timeout: u64,
    /// Keep-alive timeout in seconds (how long to keep idle connections open)
    pub keep_alive_timeout: u64,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            host: env::var("APP_SERVER_HOST")
                .unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("APP_SERVER_PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()
                .map_err(|_| ConfigError::ValidationError(
                    "Invalid APP_SERVER_PORT value".to_string()
                ))?,
            max_connections: env::var("APP_SERVER_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "1000".to_string())
                .parse()
                .map_err(|_| ConfigError::ValidationError(
                    "Invalid APP_SERVER_MAX_CONNECTIONS value".to_string()
                ))?,
            connection_timeout: env::var("APP_SERVER_CONNECTION_TIMEOUT")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .map_err(|_| ConfigError::ValidationError(
                    "Invalid APP_SERVER_CONNECTION_TIMEOUT value".to_string()
                ))?,
            keep_alive_timeout: env::var("APP_SERVER_KEEP_ALIVE_TIMEOUT")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .map_err(|_| ConfigError::ValidationError(
                    "Invalid APP_SERVER_KEEP_ALIVE_TIMEOUT value".to_string()
                ))?,
        })
    }

    /// Validate server configuration values.
    ///
    /// Performs basic validation to ensure the server configuration
    /// contains reasonable values that won't cause runtime issues.
    /// Note: Port 0 is allowed for ephemeral port usage in testing environments.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if validation passes, or `ConfigError::ValidationError`
    /// with a descriptive message if validation fails.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.host.is_empty() {
            return Err(ConfigError::ValidationError(
                "Server host cannot be empty".to_string(),
            ));
        }

        if self.max_connections == 0 {
            return Err(ConfigError::ValidationError(
                "Max connections cannot be 0".to_string(),
            ));
        }

        if self.connection_timeout == 0 {
            return Err(ConfigError::ValidationError(
                "Connection timeout cannot be 0".to_string(),
            ));
        }

        if self.keep_alive_timeout == 0 {
            return Err(ConfigError::ValidationError(
                "Keep-alive timeout cannot be 0".to_string(),
            ));
        }

        Ok(())
    }

    /// Convert server configuration to a SocketAddr for binding.
    ///
    /// Performs name resolution to convert host and port into a format suitable
    /// for binding a TCP listener. Supports IPv4, IPv6, and DNS hostnames.
    ///
    /// # Returns
    ///
    /// Returns `Ok(SocketAddr)` on successful resolution, or `ConfigError::InvalidServerAddress`
    /// if the host:port combination cannot be resolved.
    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|_| ConfigError::InvalidServerAddress {
                host: self.host.clone(),
                port: self.port,
            })?
            .next()
            .ok_or_else(|| ConfigError::InvalidServerAddress {
                host: self.host.clone(),
                port: self.port,
            })
    }
}
