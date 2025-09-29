//! lib-config/src/logging.rs

use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Logging configuration for the VEX Gateway Service.
///
/// Controls how the application logs messages, including log level,
/// output format, and file logging options.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Whether to enable structured logging (JSON format)
    pub structured: bool,
    /// Whether to enable colored output (development only)
    pub colored: bool,
    /// Optional log file path for file-based logging
    pub file_path: Option<PathBuf>,
}

impl LoggingConfig {
    /// Validate logging configuration values.
    ///
    /// Ensures the log level is valid and other settings are reasonable.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if validation passes, or `ConfigError::ValidationError`
    /// with a descriptive message if validation fails.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.level.to_lowercase().as_str()) {
            return Err(ConfigError::ValidationError(format!(
                "Invalid log level: {}. Must be one of: {:?}",
                self.level, valid_levels
            )));
        }
        Ok(())
    }
}
