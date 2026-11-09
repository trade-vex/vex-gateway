//! lib-config/src/error.rs

use std::path::PathBuf;
use thiserror::Error;

/// Configuration errors that can occur during loading or validation.
///
/// This enum provides detailed error information to help diagnose
/// configuration issues during development and deployment.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The runtime environment was not explicitly selected
    #[error("APP_ENV must be set explicitly to one of: development, testing, production")]
    MissingEnvironment,

    /// `APP_CONFIG_DIR` is set but does not name a directory
    #[error("APP_CONFIG_DIR is set to {0:?}, which is not a directory")]
    ConfigDirNotFound(PathBuf),

    /// Failed to load configuration from files or environment variables. The message never shows
    /// a value, which may be a secret set in the wrong place.
    #[error("Failed to load configuration: {0}")]
    LoadError(String),

    /// `server.host` and `server.port` name no address to bind
    #[error("Invalid server address: server.host and server.port do not resolve to an address")]
    InvalidServerAddress,

    /// `APP_ENV` names no environment
    #[error("Invalid environment in APP_ENV. Must be one of: development, testing, production")]
    InvalidEnvironment,

    /// Configuration validation failed (environment-specific rules violated)
    #[error("Configuration validation failed: {0}")]
    ValidationError(String),

    /// Settings that no configuration key reads: each `APP__` variable by name, each file key by
    /// its path and file
    #[error("Unknown configuration settings: {}", .0.join(", "))]
    UnknownSettings(Vec<String>),
}
