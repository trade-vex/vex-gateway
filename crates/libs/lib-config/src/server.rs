//! lib-config/src/server.rs

use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::env;
use std::net::{SocketAddr, ToSocketAddrs};

/// Server configuration settings for the VEX Gateway Service.
///
/// Contains all network-related configuration including host, port,
/// connection limits, and timeout settings.
/// Default worker count when neither config nor env var is set. `None` means
/// "derive from CPU count at startup" (see `effective_workers`).
const DEFAULT_WORKERS: Option<usize> = None;

/// Default idle deadline for an incoming request's headers+body. Actix's own
/// default is 5s; we bump it modestly to tolerate transient client hiccups
/// under load without inviting slowloris-style holding of worker threads.
const DEFAULT_CLIENT_REQUEST_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// The host address to bind the server to (e.g., "127.0.0.1", "0.0.0.0")
    pub host: String,
    /// The port number to bind the server to (0 for ephemeral port in testing)
    pub port: u16,
    /// Number of actix worker threads. When `None`, the server derives the
    /// count from the logical CPU count at startup. Pinning this is useful
    /// when the container CPU quota is lower than the host's core count or
    /// when co-located threads (Aeron, Kafka consumers) compete for cores.
    #[serde(default)]
    pub workers: Option<usize>,
    /// Time (seconds) allotted for a client to finish sending the request
    /// line + headers + body before actix closes the connection. Bounds the
    /// impact of slow / stuck clients on a pinned worker pool.
    #[serde(default = "default_client_request_timeout_secs")]
    pub client_request_timeout_secs: u64,
}

fn default_client_request_timeout_secs() -> u64 {
    DEFAULT_CLIENT_REQUEST_TIMEOUT_SECS
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            // Default to 127.0.0.1 for development safety; production should explicitly set APP_SERVER_HOST
            host: env::var("APP_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: env::var("APP_SERVER_PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::ValidationError("Invalid APP_SERVER_PORT value".to_string())
                })?,
            workers: match env::var("APP_SERVER_WORKERS") {
                Ok(v) if !v.is_empty() => Some(v.parse().map_err(|_| {
                    ConfigError::ValidationError("Invalid APP_SERVER_WORKERS value".to_string())
                })?),
                _ => DEFAULT_WORKERS,
            },
            client_request_timeout_secs: env::var("APP_SERVER_CLIENT_REQUEST_TIMEOUT_SECS")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| {
                    v.parse().map_err(|_| {
                        ConfigError::ValidationError(
                            "Invalid APP_SERVER_CLIENT_REQUEST_TIMEOUT_SECS value".to_string(),
                        )
                    })
                })
                .transpose()?
                .unwrap_or(DEFAULT_CLIENT_REQUEST_TIMEOUT_SECS),
        })
    }

    /// Resolve the worker count to use at startup, falling back to the logical
    /// CPU count when unset or zero. Capped at the physical CPU count to avoid
    /// oversubscription when `APP_SERVER_WORKERS` is misconfigured.
    pub fn effective_workers(&self) -> usize {
        let cpus = num_cpus::get().max(1);
        match self.workers {
            Some(n) if n > 0 => n.min(cpus * 2),
            _ => cpus,
        }
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
