//! lib-config/src/lib.rs
//!
//! Configuration management for the VEX Gateway Service.
//!
//! # Quick Start
//!
//! ```rust
//! use lib_config::Config;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Load configuration automatically (finds config files anywhere in parent directories)
//!     let config = Config::load_auto()?;
//!
//!     // Use the configuration
//!     let server_addr = config.server_address()?;
//!     println!("Server: {}:{}", config.server.host, config.server.port);
//!     Ok(())
//! }
//! ```
//!
//! # Configuration Files
//!
//! The system expects TOML configuration files in a `config/` directory:
//!
//! ```text
//! config/
//! +-- development.toml  # Development environment settings
//! +-- testing.toml      # Testing environment settings  
//! +-- production.toml   # Production environment settings (gitignored)
//! ```
//!
//! # Environment Variables
//!
//! Override any configuration value using environment variables with `APP_` prefix:
//!
//! ```bash
//! export APP_ENV=production
//! export APP_SERVER_HOST=0.0.0.0
//! export APP_SERVER_PORT=8080
//! export APP_LOGGING_LEVEL=info
//! ```
//!
//! # Configuration Discovery
//!
//! The system automatically finds configuration files by:
//! 1. Checking `APP_CONFIG_DIR` environment variable
//! 2. Searching for `config/` directory in current and parent directories
//! 3. Falling back to current directory if nothing found

pub mod environment;
mod error;
pub mod logging;
pub mod server;
pub mod util;

// Re-export commonly used types for convenience
pub use environment::Environment;
pub use logging::LoggingConfig;
pub use server::ServerConfig;

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{debug, info};

use crate::error::ConfigError;
use crate::util::find_config_dir;

/// Main configuration structure for the VEX Gateway Service.
///
/// Contains all configuration settings organized by category.
/// This is the primary configuration type that applications should use.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Current runtime environment (determines validation rules)
    pub environment: Environment,
    /// Server-specific configuration (network, connections, timeouts)
    pub server: ServerConfig,
    /// Logging configuration (level, format, output)
    pub logging: LoggingConfig,
}

impl Config {
    /// Load configuration for the specified environment.
    ///
    /// This method loads configuration in the following order:
    /// 1. Environment-specific configuration file (e.g., production.toml, development.toml, testing.toml)
    /// 2. Environment variables (with APP_ prefix)
    /// 3. Command line arguments (if provided)
    ///
    /// The configuration is automatically validated after loading to ensure
    /// it meets environment-specific requirements.
    ///
    /// # Arguments
    ///
    /// * `environment` - The target environment to load configuration for
    /// * `config_dir` - Optional directory containing configuration files (None for auto-discovery)
    ///
    /// # Returns
    ///
    /// Returns `Result<Self, ConfigError>` - Ok(Config) on successful load,
    /// or an error if configuration loading fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lib_config::{Config, Environment};
    /// use std::path::PathBuf;
    ///
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Load development configuration (auto-discovers config files)
    ///     let config = Config::load(Environment::Development, None)?;
    ///
    ///     // Load development configuration from custom directory
    ///     let config = Config::load(Environment::Development, Some(PathBuf::from("/custom/path")))?;
    ///     Ok(())
    /// }
    /// ```
    pub fn load(
        environment: Environment,
        config_dir: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let config_dir = config_dir.unwrap_or_else(find_config_dir);

        // Log what we're trying to load
        info!("Loading configuration for environment: {:?}", environment);
        debug!("Config directory: {:?}", config_dir);
        debug!(
            "Looking for config file: {:?}",
            config_dir
                .join("config")
                .join(environment.config_file_name())
        );

        // Build configuration with proper precedence
        let config = config::Config::builder()
            // 1. Load environment-specific configuration file
            .add_source(
                config::File::from(
                    config_dir
                        .join("config")
                        .join(environment.config_file_name()),
                )
                .required(false),
            )
            // 2. Load from environment variables with APP_ prefix
            .add_source(
                config::Environment::default()
                    .prefix("APP")
                    .separator("_")
                    .list_separator(",")
                    .try_parsing(true),
            )
            .build()
            .map_err(ConfigError::LoadError)?;

        // Deserialize and validate
        let mut config: Config = config.try_deserialize().map_err(ConfigError::LoadError)?;
        config.environment = environment;

        // Log what we loaded before validation
        debug!("Loaded configuration before validation:");
        debug!("  Environment: {:?}", config.environment);
        debug!("  Server host: {}", config.server.host);
        debug!("  Server port: {}", config.server.port);
        debug!("  Log level: {}", config.logging.level);

        config.validate()?;
        info!("Configuration loaded and validated successfully");
        Ok(config)
    }

    /// Load configuration with automatic environment detection.
    ///
    /// Automatically detects the environment from the `APP_ENV` environment variable
    /// and falls back to development if not specified. This is the recommended
    /// method for most applications.
    ///
    /// # Returns
    ///
    /// Returns `Result<Self, ConfigError>` - Ok(Config) on successful load,
    /// or an error if configuration loading fails.
    ///
    /// # Example
    ///
    /// ```rust
    /// use lib_config::Config;
    ///
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Automatically detect environment from APP_ENV
    ///     let config = Config::load_auto()?;
    ///     Ok(())
    /// }
    /// ```
    pub fn load_auto() -> Result<Self, ConfigError> {
        let environment = std::env::var("APP_ENV")
            .map(|env| Environment::from_str(&env))
            .unwrap_or(Ok(Environment::Development))?;

        info!("Auto-detected environment: {:?}", environment);
        Self::load(environment, None)
    }

    /// Load configuration with sensible defaults for development.
    ///
    /// This method provides a quick way to get a working configuration
    /// for development purposes without requiring configuration files.
    /// Useful for testing, examples, and quick development setup.
    ///
    /// # Returns
    ///
    /// Returns `Self` - A valid configuration with development defaults.
    ///
    /// # Example
    ///
    /// ```rust
    /// use lib_config::Config;
    ///
    /// // Get development defaults without any config files
    /// let config = Config::load_development_defaults();
    /// ```
    pub fn load_development_defaults() -> Self {
        info!("Loading development defaults (no config files required)");
        Self {
            environment: Environment::Development,
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8000,
                max_connections: 1000,
                connection_timeout: 30,
                keep_alive_timeout: 60,
            },
            logging: LoggingConfig {
                level: "debug".to_string(),
                structured: false,
                colored: true,
                file_path: None,
            },
        }
    }

    /// Validate the entire configuration.
    ///
    /// Performs comprehensive validation on all configuration sections and ensures
    /// the configuration is suitable for the current environment. This includes:
    ///
    /// - Basic validation (non-empty values, reasonable ranges)
    /// - Environment-specific validation rules
    /// - Security checks for production environments
    ///
    /// # Returns
    ///
    /// Returns `Result<(), ConfigError>` - Ok(()) if valid, or an error if validation fails.
    ///
    /// # Validation Rules
    ///
    /// ## All Environments
    /// - Server host cannot be empty
    /// - Server port cannot be 0
    /// - Max connections cannot be 0
    /// - Connection timeout cannot be 0
    /// - Log level must be valid
    ///
    /// ## Production Environment
    /// - Cannot bind to localhost (127.0.0.1 or localhost)
    /// - Cannot use trace or debug logging levels
    ///
    /// ## Development Environment
    /// - Max connections should be reasonable (< 10000)
    ///
    /// ## Testing Environment
    /// - Should use non-privileged ports (>= 1024)
    pub fn validate(&self) -> Result<(), ConfigError> {
        debug!(
            "Validating configuration for environment: {:?}",
            self.environment
        );

        // Validate server configuration
        self.server.validate()?;

        // Validate logging configuration
        self.logging.validate()?;

        // Environment-specific validations
        match self.environment {
            Environment::Production => {
                debug!("Applying production-specific validation rules");

                // Production-specific validations
                if self.server.host == "127.0.0.1" || self.server.host == "localhost" {
                    return Err(ConfigError::ValidationError(
                        "Production environment should not bind to localhost".to_string(),
                    ));
                }

                if self.logging.level == "trace" || self.logging.level == "debug" {
                    return Err(ConfigError::ValidationError(
                        "Production environment should not use trace or debug logging".to_string(),
                    ));
                }
            }
            Environment::Development => {
                debug!("Applying development-specific validation rules");

                // Development-specific validations
                if self.server.max_connections > 10000 {
                    return Err(ConfigError::ValidationError(
                        "Development environment max_connections should be reasonable".to_string(),
                    ));
                }
            }
            Environment::Testing => {
                debug!("Applying testing-specific validation rules");

                // Testing-specific validations
                if self.server.port < 1024 {
                    return Err(ConfigError::ValidationError(
                        "Testing environment should use non-privileged ports".to_string(),
                    ));
                }
            }
        }

        debug!("Configuration validation passed");
        Ok(())
    }

    /// Get the server socket address for binding.
    ///
    /// Converts the server configuration into a SocketAddr suitable
    /// for binding a TCP listener.
    ///
    /// # Returns
    ///
    /// Returns `Result<SocketAddr, ConfigError>` - The parsed socket address for server binding.
    ///
    /// # Example
    ///
    /// ```rust
    /// use lib_config::Config;
    ///
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let config = Config::load_auto()?;
    ///     let addr = config.server_address()?;
    ///     // Use addr with your web framework
    ///     Ok(())
    /// }
    /// ```
    pub fn server_address(&self) -> Result<SocketAddr, ConfigError> {
        self.server.socket_addr()
    }

    /// Check if the current environment is development.
    pub fn is_development(&self) -> bool {
        self.environment.is_development()
    }

    /// Check if the current environment is production.
    pub fn is_production(&self) -> bool {
        self.environment.is_production()
    }

    /// Get a human-readable description of the configuration.
    ///
    /// Returns a formatted string describing the current configuration
    /// for logging and debugging purposes.
    ///
    /// # Returns
    ///
    /// Returns a formatted string with environment, server address, and log level.
    pub fn description(&self) -> String {
        format!(
            "VEX Gateway Service Configuration - Environment: {:?}, Server: {}:{}, Log Level: {}",
            self.environment, self.server.host, self.server.port, self.logging.level
        )
    }
}

impl Default for Config {
    /// Default implementation returns development defaults.
    ///
    /// This allows Config to be used in structs that require Default,
    /// providing sensible defaults for development.
    fn default() -> Self {
        Self::load_development_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_environment_parsing() {
        assert_eq!(
            Environment::from_str("development").unwrap(),
            Environment::Development
        );
        assert_eq!(
            Environment::from_str("dev").unwrap(),
            Environment::Development
        );
        assert_eq!(
            Environment::from_str("production").unwrap(),
            Environment::Production
        );
        assert_eq!(
            Environment::from_str("prod").unwrap(),
            Environment::Production
        );
        assert_eq!(
            Environment::from_str("testing").unwrap(),
            Environment::Testing
        );
        assert_eq!(Environment::from_str("test").unwrap(), Environment::Testing);

        assert!(Environment::from_str("invalid").is_err());
    }

    #[test]
    fn test_server_config_validation() {
        let valid_config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8000,
            max_connections: 1000,
            connection_timeout: 30,
            keep_alive_timeout: 60,
        };
        assert!(valid_config.validate().is_ok());

        let invalid_config = ServerConfig {
            host: "".to_string(),
            port: 8000,
            max_connections: 1000,
            connection_timeout: 30,
            keep_alive_timeout: 60,
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::load_development_defaults();
        assert!(config.validate().is_ok());

        // Test production validation
        config.environment = Environment::Production;
        config.server.host = "127.0.0.1".to_string();
        assert!(config.validate().is_err());

        config.server.host = "0.0.0.0".to_string();
        config.logging.level = "debug".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_load_development_defaults() {
        let config = Config::load_development_defaults();
        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8000);
        assert_eq!(config.logging.level, "debug");
        assert!(config.logging.colored);
        assert!(!config.logging.structured);
    }

    #[test]
    fn test_server_address_parsing() {
        let config = Config::load_development_defaults();
        let addr = config.server_address().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_eq!(addr.port(), 8000);
    }
}
