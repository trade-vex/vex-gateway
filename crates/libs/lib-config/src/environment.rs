//! lib-config/src/environment.rs

use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Supported runtime environments for the VEX Gateway Service.
///
/// Each environment has different configuration requirements and validation rules.
/// The environment determines which configuration file is loaded and what
/// validation rules are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    /// Development environment
    Development,
    /// Testing environment
    Testing,
    /// Production environment
    Production,
}

impl FromStr for Environment {
    type Err = ConfigError;

    /// Parse environment from string
    ///
    /// Supports both full names and abbreviations:
    /// - "development" or "dev" → Development
    /// - "testing" or "test" → Testing  
    /// - "production" or "prod" → Production
    ///
    /// # Arguments
    ///
    /// * `s` - The environment string to parse
    ///
    /// # Returns
    ///
    /// Returns `Ok(Environment)` on successful parse, or `ConfigError::InvalidEnvironment`
    /// if the string doesn't match any known environment.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "development" | "dev" => Ok(Environment::Development),
            "testing" | "test" => Ok(Environment::Testing),
            "production" | "prod" => Ok(Environment::Production),
            _ => Err(ConfigError::InvalidEnvironment(s.to_string())),
        }
    }
}

impl Environment {
    /// Get the configuration file name for this environment.
    ///
    /// Returns the filename (without path) that should contain
    /// the configuration for this environment.
    pub fn config_file_name(&self) -> &'static str {
        match self {
            Environment::Development => "development.toml",
            Environment::Testing => "testing.toml",
            Environment::Production => "production.toml",
        }
    }

    /// Check if this is a development environment.
    pub fn is_development(&self) -> bool {
        matches!(self, Environment::Development)
    }

    /// Check if this is a production environment.
    pub fn is_production(&self) -> bool {
        matches!(self, Environment::Production)
    }
}
