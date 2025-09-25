//! lib-config/src/error.rs

use thiserror::Error;

/// Configuration errors that can occur during loading or validation.
///
/// This enum provides detailed error information to help diagnose
/// configuration issues during development and deployment.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to load configuration from files or environment variables
    #[error("Failed to load configuration: {0}")]
    LoadError(#[from] config::ConfigError),

    /// Invalid server address (host:port combination cannot be parsed)
    #[error("Invalid server address: {host}:{port}")]
    InvalidServerAddress { host: String, port: u16 },

    /// Invalid environment name provided
    #[error("Invalid environment: {0}. Must be one of: development, testing, production")]
    InvalidEnvironment(String),

    /// Configuration validation failed (environment-specific rules violated)
    #[error("Configuration validation failed: {0}")]
    ValidationError(String),
}
