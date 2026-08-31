//! lib-config/src/lib.rs
//!
//! Configuration management for the VEX Gateway Service.
//!
//! # Quick Start
//!
//! ```no_run
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
//! `APP_ENV` selects the environment and so the configuration file, and `APP_CONFIG_DIR` may name
//! the directory holding it.
//!
//! Any setting can be overridden with `APP__<SECTION>__<KEY>`: its key path in upper case with `__`
//! between the levels, like vex-core's `VEX__` variables. A list takes comma-separated values. A
//! variable that names no key, like a file key that no setting reads, is an error, and so is one
//! whose name or value is not UTF-8. Apart from `APP_ENV` and `APP_CONFIG_DIR`, no other variable
//! is read.
//!
//! ```bash
//! export APP_ENV=production
//! export APP__SERVER__HOST=0.0.0.0                       # server.host
//! export APP__LOGGING__LEVEL=info                        # logging.level
//! export APP__AUTH__SERVER__TRUSTED_PROXIES=10.0.1.0/24  # auth.server.trusted_proxies
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
pub use error::ConfigError;
pub use logging::LoggingConfig;
pub use server::ServerConfig;

use config::Source;
use serde::Deserialize;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::{debug, info};

use crate::util::find_config_dir;

/// Re-export AuthConfig from lib-auth for convenience
pub use lib_auth::config::AuthConfig;
use lib_auth::config::OAUTH_PROVIDERS;

/// Main configuration structure for the VEX Gateway Service.
///
/// Contains all configuration settings organized by category.
/// This is the primary configuration type that applications should use.
///
/// # Security Note
/// This struct intentionally does not derive `Serialize` to prevent accidental
/// serialization of sensitive configuration values (database credentials, JWT keys, etc.)
/// into logs, responses, or debug output.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Current runtime environment (determines validation rules)
    pub environment: Environment,
    /// Server-specific configuration (network, connections, timeouts)
    pub server: ServerConfig,
    /// Logging configuration (level, format, output)
    pub logging: LoggingConfig,
    /// Authentication configuration
    pub auth: AuthConfig,
}

impl Config {
    /// Load configuration for the specified environment.
    ///
    /// This method loads configuration in the following order:
    /// 1. Environment-specific configuration file (e.g., production.toml, development.toml, testing.toml),
    ///    which must exist
    /// 2. `APP__<SECTION>__<KEY>` environment variables
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
        Self::load_with(
            environment,
            config_dir,
            Self::app_overrides(std::env::vars_os())?,
        )
    }

    /// The override layer, named like vex-core's `VEX__` variables: `APP__<SECTION>__<KEY>` is the
    /// key path in upper case with `__` between its levels (`APP__AUTH__SERVER__TRUSTED_PROXIES`
    /// sets `auth.server.trusted_proxies`). The bootstrap variables `APP_ENV` and `APP_CONFIG_DIR`
    /// have a single `_` and are not part of it. Values stay strings until the setting that reads
    /// them converts them, so a text setting such as a password is taken verbatim.
    ///
    /// It holds only the `APP__` variables of `variables`, the process environment outside tests.
    /// Left to read the environment itself, the config library would read every variable through
    /// `std::env::vars()`, which panics, printing the variable, on one that is not UTF-8, even one
    /// that is not the gateway's. An `APP__` variable that is not UTF-8 is an error that names it
    /// (lossily) and never shows its value.
    fn app_overrides(
        variables: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Result<config::Environment, ConfigError> {
        let mut overrides = config::Map::new();
        for (name, value) in variables {
            // Case-insensitively (ASCII), as the config library matches the prefix.
            let prefix = name.as_encoded_bytes().get(..5);
            if !prefix.is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"APP__")) {
                continue;
            }
            let (Some(name), Ok(value)) = (name.to_str(), value.into_string()) else {
                return Err(ConfigError::LoadError(format!(
                    "{} is not valid UTF-8",
                    name.to_string_lossy()
                )));
            };
            overrides.insert(name.to_string(), value);
        }
        Ok(config::Environment::with_prefix("APP")
            .separator("__")
            .source(Some(overrides)))
    }

    fn load_with(
        environment: Environment,
        config_dir: Option<PathBuf>,
        overrides: config::Environment,
    ) -> Result<Self, ConfigError> {
        let config_dir = config_dir.unwrap_or_else(find_config_dir);
        let file = config_dir.join(environment.config_file_name());

        // Log what we're trying to load
        info!("Loading configuration for environment: {:?}", environment);
        debug!("Config directory: {:?}", config_dir);
        debug!("Looking for config file: {:?}", file);

        // The key path each `APP__` variable sets, to name the variable if no setting reads it.
        let overridden = overrides
            .collect()
            .map_err(|error| load_error(error, &Default::default(), &file))?;

        // Build configuration with proper precedence
        let config = config::Config::builder()
            // 1. Load environment-specific configuration file, which must exist
            .add_source(config::File::from(file.clone()))
            // 2. Override it with the `APP__` environment variables
            .add_source(overrides)
            .build()
            .map_err(|error| load_error(error, &overridden, &file))?;

        // Deserialize, collecting every key that no setting reads: a misspelt or retired key, in
        // the file or from an `APP__` variable, stops startup instead of being ignored. It is the
        // error even when deserialization fails too, as when a renamed key leaves one missing.
        let mut unknown = Vec::new();
        let result: Result<Config, _> =
            serde_ignored::deserialize(config, |path| unknown.push(path.to_string()));
        // A variable below a setting (`APP__SERVER__PORT__X`) turns that setting into a table, which
        // is a type error at the setting; such a variable names no key either.
        if let Err(config::ConfigError::Type { key: Some(key), .. }) = &result {
            let below = format!("{key}.");
            unknown.extend(
                overridden
                    .keys()
                    .filter(|path| path.starts_with(&below))
                    .cloned(),
            );
        }
        if !unknown.is_empty() {
            return Err(ConfigError::UnknownSettings(unknown_settings(
                &unknown,
                &overridden,
                &file,
            )));
        }
        let config = result.map_err(|error| load_error(error, &overridden, &file))?;

        // The selected environment chose the file, so the file (or `APP__ENVIRONMENT`) must agree.
        if config.environment != environment {
            return Err(ConfigError::ValidationError(format!(
                "the configuration's environment ({:?}) is not the selected one ({:?}, from APP_ENV)",
                config.environment, environment
            )));
        }

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
    /// Loads the environment selected by the `APP_ENV` environment variable.
    /// `APP_ENV` must be set explicitly, and a configuration that cannot be loaded or
    /// validated is an error in every environment.
    ///
    /// # Returns
    ///
    /// Returns `Result<Self, ConfigError>` - Ok(Config) on successful load,
    /// or an error if configuration loading fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lib_config::Config;
    ///
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Automatically detect environment from APP_ENV
    ///     let config = Config::load_auto()?;
    ///     Ok(())
    /// }
    /// ```
    pub fn load_auto() -> Result<Self, ConfigError> {
        let app_env = std::env::var("APP_ENV").ok();
        Self::load_selected(app_env.as_deref(), None)
    }

    fn load_selected(
        app_env: Option<&str>,
        config_dir: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let app_env = app_env.ok_or(ConfigError::MissingEnvironment)?;
        let environment = Environment::from_str(app_env)?;

        info!("Auto-detected environment: {:?}", environment);
        Self::load(environment, config_dir)
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
    /// - Max connections cannot be 0
    /// - Connection timeout cannot be 0
    /// - Log level must be valid
    /// - Every `auth.server.trusted_proxies` entry must be an IP address or CIDR range
    /// - `auth.oauth.providers` may list only `google`, `github` and `twitter`
    ///
    /// ## Production Environment
    /// - Server port cannot be 0 (must be explicitly configured)
    /// - Cannot bind to localhost (127.0.0.1 or localhost)
    /// - Cannot use trace or debug logging levels
    /// - `auth.server.trusted_proxies` cannot be empty: production runs behind a load balancer
    ///
    /// ## Development Environment
    /// - Max connections should be reasonable (< 10000)
    ///
    /// ## Testing Environment
    /// - Port 0 is allowed for ephemeral port usage
    /// - If port is not 0, should use non-privileged ports (>= 1024)
    pub fn validate(&self) -> Result<(), ConfigError> {
        debug!(
            "Validating configuration for environment: {:?}",
            self.environment
        );

        // Validate server configuration
        self.server.validate()?;

        // Validate logging configuration
        self.logging.validate()?;

        // Parsed here, before any network startup, though only AppState::new uses the result.
        if let Err(e) = self.auth.server.parsed_trusted_proxies() {
            return Err(ConfigError::ValidationError(format!(
                "auth.server.trusted_proxies: {e}"
            )));
        }

        // OAuthService knows the scopes and user-info endpoint of these providers only; any other
        // would load, then fail at its callback.
        let mut unsupported: Vec<&str> = self
            .auth
            .oauth
            .providers
            .keys()
            .map(String::as_str)
            .filter(|provider| !OAUTH_PROVIDERS.contains(provider))
            .collect();
        if !unsupported.is_empty() {
            unsupported.sort_unstable();
            return Err(ConfigError::ValidationError(format!(
                "auth.oauth.providers: unsupported provider {} (supported: {})",
                unsupported.join(", "),
                OAUTH_PROVIDERS.join(", ")
            )));
        }

        // Environment-specific validations
        match self.environment {
            Environment::Production => {
                debug!("Applying production-specific validation rules");

                // Production-specific validations
                if self.server.port == 0 {
                    return Err(ConfigError::ValidationError(
                        "Production environment must have an explicitly configured port (cannot be 0)".to_string(),
                    ));
                }

                if self.server.host == "127.0.0.1" || self.server.host == "localhost" {
                    return Err(ConfigError::ValidationError(
                        "Production environment should not bind to localhost".to_string(),
                    ));
                }

                let log_level = self.logging.level.trim().to_lowercase();
                if log_level == "trace" || log_level == "debug" {
                    return Err(ConfigError::ValidationError(
                        "Production environment should not use trace or debug logging".to_string(),
                    ));
                }

                // Behind the load balancer every TCP peer is the balancer, so without its
                // addresses all clients would share one rate-limit key.
                if self.auth.server.trusted_proxies.is_empty() {
                    return Err(ConfigError::ValidationError(
                        "Production environment requires auth.server.trusted_proxies (the load \
                         balancer's addresses or CIDR ranges)"
                            .to_string(),
                    ));
                }
            }
            Environment::Development => {
                debug!("Applying development-specific validation rules");
            }
            Environment::Testing => {
                debug!("Applying testing-specific validation rules");

                // Testing-specific validations
                // Port 0 is allowed for ephemeral ports, but if a specific port is used, it should be non-privileged
                if self.server.port != 0 && self.server.port < 1024 {
                    return Err(ConfigError::ValidationError(
                        "Testing environment should use non-privileged ports (>= 1024) or port 0 for ephemeral ports".to_string(),
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
    /// ```no_run
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

/// Where each unknown key path came from: the `APP__` variables that set it or a key below it, or
/// else the configuration file.
fn unknown_settings(
    unknown: &[String],
    overridden: &config::Map<String, config::Value>,
    file: &Path,
) -> Vec<String> {
    let mut settings = Vec::new();
    for key in unknown {
        let below = format!("{key}.");
        let mut variables: Vec<String> = overridden
            .keys()
            .filter(|path| *path == key || path.starts_with(&below))
            .map(|path| variable(path))
            .collect();
        if variables.is_empty() {
            settings.push(format!("{key} (in {})", file.display()));
        } else {
            variables.sort();
            settings.append(&mut variables);
        }
    }
    settings
}

/// The `APP__` variable that sets a key path.
fn variable(key: &str) -> String {
    format!("APP__{}", key.to_uppercase().replace('.', "__"))
}

/// Where a key was set: the `APP__` variable that set it, or else the configuration file.
fn setting(key: &str, overridden: &config::Map<String, config::Value>, file: &Path) -> String {
    if overridden.contains_key(key) {
        variable(key)
    } else {
        format!("{key} (in {})", file.display())
    }
}

/// The loading error, which never shows a value: a secret set in the wrong place, such as a
/// password in `APP__SERVER__PORT`, must not reach the log. Where the config library gives the
/// key, it names the setting (its `APP__` variable, or its key and the file) and what it expects.
fn load_error(
    error: config::ConfigError,
    overridden: &config::Map<String, config::Value>,
    file: &Path,
) -> ConfigError {
    let reason = match error {
        // `unexpected` is the value.
        config::ConfigError::Type {
            key: Some(key),
            expected,
            ..
        } => format!("{} must be {expected}", setting(&key, overridden, file)),
        config::ConfigError::Type { expected, .. } => format!("a value must be {expected}"),
        // The TOML parser quotes the offending line below the position.
        config::ConfigError::FileParse { cause, .. } => {
            let cause = cause.to_string();
            match cause
                .lines()
                .next()
                .filter(|line| line.starts_with("TOML parse error at line "))
            {
                Some(position) => format!("{}: {position}", file.display()),
                None => format!("{} is not valid TOML", file.display()),
            }
        }
        // A message names no key. serde's for a missing setting names its field.
        config::ConfigError::Message(message) if message.starts_with("missing field ") => message,
        // config-rs's for `environment`, the only enum, quotes the value after a fixed prefix.
        config::ConfigError::Message(message)
            if message.starts_with("enum Environment ")
                || message.starts_with("value of enum Environment ") =>
        {
            format!(
                "{} must be one of: development, testing, production",
                setting("environment", overridden, file)
            )
        }
        // serde's for a section or list given a single value quotes it before what was expected
        // (`invalid type: string "…", expected struct DatabaseConfig`).
        config::ConfigError::Message(message) => match message.rsplit_once(", expected ") {
            Some((found, expected)) if found.starts_with("invalid ") => {
                format!("a section or list has a value of the wrong type: expected {expected}")
            }
            _ => "an invalid value".to_string(),
        },
        // `NotFound` names a key; `Foreign` (the file is missing, which names it, or cannot be
        // read), `Frozen` and `PathParse` carry no value.
        error => error.to_string(),
    };
    ConfigError::LoadError(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The committed development example (config/development.toml.example), loaded as a
    /// development.toml through the real loader.
    fn development_config() -> Config {
        load_development_example(&[]).unwrap()
    }

    /// The development example loaded with `variables` as the only environment variables the
    /// `APP__` layer sees.
    fn load_development_example(variables: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let config_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join("development.toml"),
            include_str!("../../../../config/development.toml.example"),
        )
        .unwrap();
        let variables = variables
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        Config::load_with(
            Environment::Development,
            Some(config_dir.path().to_path_buf()),
            Config::app_overrides(variables)?,
        )
    }

    /// The development example with `line` replaced, loaded without `APP__` variables, and the
    /// file it was loaded from.
    fn load_edited_example(
        line: &str,
        replacement: &str,
    ) -> (Result<Config, ConfigError>, PathBuf) {
        let config_dir = tempfile::tempdir().unwrap();
        let example = include_str!("../../../../config/development.toml.example");
        assert_eq!(example.matches(line).count(), 1);
        let file = config_dir.path().join("development.toml");
        fs::write(&file, example.replace(line, replacement)).unwrap();
        let result = Config::load_with(
            Environment::Development,
            Some(config_dir.path().to_path_buf()),
            Config::app_overrides([]).unwrap(),
        );
        (result, file)
    }

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
    fn test_environment_selection_requires_app_env() {
        let error = Config::load_selected(None, None).unwrap_err();

        assert!(matches!(error, ConfigError::MissingEnvironment));
        assert_eq!(
            error.to_string(),
            "APP_ENV must be set explicitly to one of: development, testing, production"
        );
    }

    #[test]
    fn test_development_config_errors_are_not_replaced_by_defaults() {
        let config_dir = tempfile::tempdir().unwrap();
        let load =
            || Config::load_selected(Some("development"), Some(config_dir.path().to_path_buf()));

        // No configuration file: the required settings are missing.
        assert!(matches!(load(), Err(ConfigError::LoadError(_))));

        fs::write(
            config_dir.path().join("development.toml"),
            "[server\nhost = ",
        )
        .unwrap();
        assert!(matches!(load(), Err(ConfigError::LoadError(_))));
    }

    #[test]
    fn test_explicit_production_loads_complete_configuration() {
        let config_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join("production.toml"),
            r#"
environment = "production"

[server]
host = "0.0.0.0"
port = 8081

[logging]
level = "info"
structured = true
colored = false

[auth.jwt]
private_key_path = "/run/secrets/jwt-private.pem"
public_key_path = "/run/secrets/jwt-public.pem"
access_token_ttl_minutes = 15
refresh_token_ttl_days = 7
issuer = "vex-gateway"
audience = "vex-api"

[auth.database]
url = "postgresql://configured@db.internal/vex"
max_connections = 20

[auth.redis]
url = "redis://redis.internal:6379"
max_connections = 20

[auth.oauth]
providers = {}
redirect_base = "https://gateway.example.com"

[auth.server]
base_url = "https://gateway.example.com"
port = 8081
frontend_url = "https://app.example.com"
trusted_proxies = ["10.0.1.0/24", "10.0.2.0/24"]

[auth.email]
smtp_host = "smtp.example.com"
smtp_port = 587
smtp_username = "gateway@example.com"
smtp_password = "configured-test-value"
from_email = "gateway@example.com"

[auth.otp]
otp_length = 6
email_verification_expiry_minutes = 15
"#,
        )
        .unwrap();

        let config =
            Config::load_selected(Some("production"), Some(config_dir.path().to_path_buf()))
                .unwrap();

        assert_eq!(config.environment, Environment::Production);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.logging.level, "info");
        assert_eq!(
            config.auth.database.url,
            "postgresql://configured@db.internal/vex"
        );
        assert_eq!(
            config.auth.server.trusted_proxies,
            ["10.0.1.0/24", "10.0.2.0/24"]
        );
    }

    #[test]
    fn test_server_config_validation() {
        let valid_config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8081,
        };
        assert!(valid_config.validate().is_ok());

        let invalid_config = ServerConfig {
            host: "".to_string(),
            port: 8081,
        };
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_config_validation() {
        let mut config = development_config();
        assert!(config.validate().is_ok());

        // Test production validation
        config.environment = Environment::Production;
        config.auth.server.trusted_proxies = vec!["10.0.0.0/16".to_string()];
        config.server.host = "127.0.0.1".to_string();
        assert!(config.validate().is_err());

        config.server.host = "0.0.0.0".to_string();
        config.logging.level = "debug".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_production_requires_trusted_proxies() {
        let mut config = development_config();
        config.environment = Environment::Production;
        config.server.host = "0.0.0.0".to_string();
        config.logging.level = "info".to_string();
        config.auth.server.trusted_proxies = Vec::new();

        let error = config.validate().unwrap_err();
        assert!(
            error.to_string().contains("auth.server.trusted_proxies"),
            "unexpected error: {error}"
        );

        config.auth.server.trusted_proxies = vec!["10.0.0.0/16".to_string()];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_trusted_proxy_stops_loading_without_showing_it() {
        let error = load_development_example(&[(
            "APP__AUTH__SERVER__TRUSTED_PROXIES",
            "10.0.0.0/8,proxy.internal",
        )])
        .unwrap_err()
        .to_string();

        assert_eq!(
            error,
            "Configuration validation failed: auth.server.trusted_proxies: entry 2 is not an IP \
             address or CIDR range"
        );
    }

    #[test]
    fn test_missing_configuration_file_is_an_error_that_names_it() {
        let config_dir = tempfile::tempdir().unwrap();
        let file = config_dir.path().join("production.toml");

        // Settings live in the file: `APP__` variables only override it and cannot replace it.
        let error = Config::load_with(
            Environment::Production,
            Some(config_dir.path().to_path_buf()),
            Config::app_overrides([("APP__ENVIRONMENT".into(), "production".into())]).unwrap(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "Failed to load configuration: configuration file \"{}\" not found",
                file.display()
            )
        );
    }

    #[test]
    fn test_unsupported_oauth_provider_stops_loading_and_is_named() {
        let (result, _) = load_edited_example(
            "providers = {}",
            r#"providers = { gitlab = { client_id = "id", client_secret = "secret", auth_url = "https://gitlab.example.com/auth", token_url = "https://gitlab.example.com/token", user_info_url = "https://gitlab.example.com/user", scopes = ["read_user"], response_type = "code" } }"#,
        );

        assert_eq!(
            result.unwrap_err().to_string(),
            "Configuration validation failed: auth.oauth.providers: unsupported provider gitlab \
             (supported: google, github, twitter)"
        );
    }

    #[test]
    fn test_development_example_loads() {
        let config = development_config();
        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8081);
        assert_eq!(config.logging.level, "debug");
        assert!(config.logging.colored);
        assert!(!config.logging.structured);
        // trusted_proxies is optional: absent, development trusts no proxy.
        assert!(config.auth.server.trusted_proxies.is_empty());
    }

    #[test]
    fn test_app_overrides_reach_keys_with_underscores() {
        let config = load_development_example(&[
            ("APP__SERVER__PORT", "9090"),
            (
                "APP__AUTH__SERVER__TRUSTED_PROXIES",
                "10.0.1.0/24, 10.0.2.0/24",
            ),
            ("APP__AUTH__JWT__ACCESS_TOKEN_TTL_MINUTES", "5"),
            ("APP__AUTH__JWT__ISSUER", "0123"),
            ("APP__LOGGING__FILE_PATH", "/var/log/vex-gateway.log"),
            // A map entry, with a list below it.
            ("APP__AUTH__OAUTH__PROVIDERS__GOOGLE__CLIENT_ID", "id"),
            (
                "APP__AUTH__OAUTH__PROVIDERS__GOOGLE__CLIENT_SECRET",
                "secret",
            ),
            (
                "APP__AUTH__OAUTH__PROVIDERS__GOOGLE__AUTH_URL",
                "https://accounts.example.com/auth",
            ),
            (
                "APP__AUTH__OAUTH__PROVIDERS__GOOGLE__TOKEN_URL",
                "https://accounts.example.com/token",
            ),
            (
                "APP__AUTH__OAUTH__PROVIDERS__GOOGLE__USER_INFO_URL",
                "https://accounts.example.com/userinfo",
            ),
            (
                "APP__AUTH__OAUTH__PROVIDERS__GOOGLE__SCOPES",
                "openid,email",
            ),
            ("APP__AUTH__OAUTH__PROVIDERS__GOOGLE__RESPONSE_TYPE", "code"),
        ])
        .unwrap();

        assert_eq!(config.server.port, 9090);
        assert_eq!(
            config.auth.server.trusted_proxies,
            ["10.0.1.0/24", "10.0.2.0/24"]
        );
        assert_eq!(config.auth.jwt.access_token_ttl_minutes, 5);
        // A text setting is taken verbatim, not read as the number 123.
        assert_eq!(config.auth.jwt.issuer, "0123");
        assert_eq!(
            config.logging.file_path.as_deref(),
            Some(Path::new("/var/log/vex-gateway.log"))
        );
        assert_eq!(
            config.auth.oauth.providers["google"].scopes,
            ["openid", "email"]
        );
    }

    #[test]
    fn test_unknown_settings_are_errors_that_name_their_source() {
        // An `APP__` variable that names no key, at any depth, is named in the error.
        for name in [
            "APP__FOO__BAR",
            "APP__SERVER__HOTS",
            "APP__AUTH__SERVER__TRUSTED_PROXY",
            // Below a setting, including a list index.
            "APP__SERVER__PORT__NUMBER",
            "APP__AUTH__SERVER__TRUSTED_PROXIES__0",
        ] {
            let error = load_development_example(&[(name, "1")]).unwrap_err();
            assert!(
                matches!(error, ConfigError::UnknownSettings(_))
                    && error.to_string() == format!("Unknown configuration settings: {name}"),
                "{name}: unexpected error: {error}"
            );
        }

        // So is a file key that no setting reads, with its file, also when renaming it leaves a
        // required key missing.
        let config_dir = tempfile::tempdir().unwrap();
        let example = include_str!("../../../../config/development.toml.example");
        assert_eq!(example.matches("[server]\nhost = ").count(), 1);
        fs::write(
            config_dir.path().join("development.toml"),
            example.replace("[server]\nhost = ", "[server]\nhots = "),
        )
        .unwrap();
        let error = Config::load_with(
            Environment::Development,
            Some(config_dir.path().to_path_buf()),
            Config::app_overrides([]).unwrap(),
        )
        .unwrap_err();
        let file = config_dir.path().join("development.toml");
        assert_eq!(
            error.to_string(),
            format!(
                "Unknown configuration settings: server.hots (in {})",
                file.display()
            )
        );
    }

    #[test]
    fn test_bootstrap_variables_are_not_overrides() {
        // A single `_`: the `APP__` layer reads neither, so neither is an unknown key.
        let config = load_development_example(&[
            ("APP_ENV", "production"),
            ("APP_CONFIG_DIR", "/nonexistent"),
        ])
        .unwrap();
        assert_eq!(config.environment, Environment::Development);
    }

    #[test]
    fn test_environment_must_match_the_selected_one() {
        let error = load_development_example(&[("APP__ENVIRONMENT", "production")]).unwrap_err();
        assert!(
            matches!(error, ConfigError::ValidationError(_))
                && error.to_string().contains("APP_ENV"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_load_errors_name_the_setting_but_never_its_value() {
        let secret = "Sup3r-S3cret";
        let mut errors = Vec::new();
        let mut expect = |error: ConfigError, expected: &str| {
            assert_eq!(error.to_string(), expected);
            errors.push(error);
        };

        // A value of the wrong type names its variable, or its key and the file, and what the
        // setting expects; so does an unknown environment.
        expect(
            load_development_example(&[("APP__SERVER__PORT", secret)]).unwrap_err(),
            "Failed to load configuration: APP__SERVER__PORT must be an integer",
        );
        expect(
            load_development_example(&[("APP__SERVER__PORT", "65536")]).unwrap_err(),
            "Failed to load configuration: APP__SERVER__PORT must be an unsigned 16 bit integer",
        );
        let (result, file) =
            load_edited_example("otp_length = 6", &format!("otp_length = \"{secret}\""));
        expect(
            result.unwrap_err(),
            &format!(
                "Failed to load configuration: auth.otp.otp_length (in {}) must be an integer",
                file.display()
            ),
        );
        expect(
            load_development_example(&[("APP__ENVIRONMENT", secret)]).unwrap_err(),
            "Failed to load configuration: APP__ENVIRONMENT must be one of: development, testing, \
             production",
        );
        let (result, file) = load_edited_example(
            "environment = \"development\"",
            &format!("environment = \"{secret}\""),
        );
        expect(
            result.unwrap_err(),
            &format!(
                "Failed to load configuration: environment (in {}) must be one of: development, \
                 testing, production",
                file.display()
            ),
        );

        // A section given a single value (the variable named the section, not its `url`) is named
        // by what it expects.
        expect(
            load_development_example(&[(
                "APP__AUTH__DATABASE",
                &format!("postgres://vex:{secret}@db.internal/vex"),
            )])
            .unwrap_err(),
            "Failed to load configuration: a section or list has a value of the wrong type: \
             expected struct DatabaseConfig",
        );

        // A line that is not TOML: the file and the position, not the line.
        let (result, file) = load_edited_example(
            "smtp_password = \"password\"",
            &format!("smtp_password = \"{secret}"),
        );
        let syntax = result.unwrap_err();
        assert!(
            syntax.to_string().starts_with(&format!(
                "Failed to load configuration: {}: TOML parse error at line ",
                file.display()
            )),
            "unexpected error: {syntax}"
        );

        // No value in any of these, an unknown `APP_ENV`, an invalid log level or an address that
        // does not resolve, whether displayed or debug-printed.
        let unresolvable = ServerConfig {
            host: format!("{secret}\0"),
            port: 8081,
        };
        for error in errors.into_iter().chain([
            syntax,
            Config::load_selected(Some(secret), None).unwrap_err(),
            load_development_example(&[("APP__LOGGING__LEVEL", secret)]).unwrap_err(),
            unresolvable.socket_addr().unwrap_err(),
        ]) {
            let text = format!("{error} {error:?}").to_lowercase();
            assert!(!text.contains(&secret.to_lowercase()), "{text}");
        }
    }

    #[test]
    fn test_lenient_conversions_documented_in_the_runbook() {
        let config = load_development_example(&[
            ("APP__SERVER__PORT", "yes"),
            ("APP__LOGGING__STRUCTURED", "on"),
        ])
        .unwrap();
        assert_eq!(config.server.port, 1);
        assert!(config.logging.structured);

        // A decimal is rounded in the file, and rejected in a variable.
        let config_dir = tempfile::tempdir().unwrap();
        let example = include_str!("../../../../config/development.toml.example");
        assert_eq!(example.matches("port = 8081\n").count(), 2);
        fs::write(
            config_dir.path().join("development.toml"),
            example.replacen("port = 8081\n", "port = 8080.6\n", 1),
        )
        .unwrap();
        let config = Config::load_with(
            Environment::Development,
            Some(config_dir.path().to_path_buf()),
            Config::app_overrides([]).unwrap(),
        )
        .unwrap();
        assert_eq!(config.server.port, 8081);
        assert!(matches!(
            load_development_example(&[("APP__SERVER__PORT", "8080.6")]),
            Err(ConfigError::LoadError(_))
        ));
    }

    #[test]
    fn test_server_address_parsing() {
        let config = development_config();
        let addr = config.server_address().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_eq!(addr.port(), 8081);
    }

    #[test]
    fn test_port_0_validation() {
        // Test that port 0 is allowed in testing environment
        let mut testing_config = development_config();
        testing_config.environment = Environment::Testing;
        testing_config.server.port = 0;
        assert!(testing_config.validate().is_ok());

        // Test that port 0 is rejected in production environment
        let mut production_config = development_config();
        production_config.environment = Environment::Production;
        production_config.auth.server.trusted_proxies = vec!["10.0.0.0/16".to_string()];
        production_config.server.port = 0;
        production_config.server.host = "0.0.0.0".to_string();
        production_config.logging.level = "info".to_string();
        assert!(production_config.validate().is_err());

        // Test that port 0 is allowed in development environment
        let mut dev_config = development_config();
        dev_config.server.port = 0;
        assert!(dev_config.validate().is_ok());
    }
}
