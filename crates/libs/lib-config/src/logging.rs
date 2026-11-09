//! lib-config/src/logging.rs

use crate::ConfigError;
use serde::{Deserialize, Serialize};

/// Logging configuration for the VEX Gateway Service.
///
/// The application's log level.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
}

impl LoggingConfig {
    /// Validate logging configuration values.
    ///
    /// Ensures the log level is valid.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if validation passes, or `ConfigError::ValidationError`
    /// with a descriptive message if validation fails.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        let level = self.level.trim().to_lowercase();
        if !valid_levels.contains(&level.as_str()) {
            return Err(ConfigError::ValidationError(format!(
                "logging.level must be one of: {}",
                valid_levels.join(", ")
            )));
        }
        Ok(())
    }
}
