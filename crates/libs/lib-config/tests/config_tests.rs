//! tests for lib-config

use lib_config::{Config, Environment, ServerConfig, LoggingConfig};
use std::str::FromStr;

fn create_test_server_config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 8081,
        max_connections: 1000,
        connection_timeout: 30,
        keep_alive_timeout: 60,
    }
}

fn create_test_logging_config() -> LoggingConfig {
    LoggingConfig {
        level: "debug".to_string(),
        structured: false,
        colored: true,
        file_path: None,
    }
}

#[test]
fn test_environment_from_str_comprehensive() {
    // test all the valid environment strings we support
    assert_eq!(Environment::from_str("development").unwrap(), Environment::Development);
    assert_eq!(Environment::from_str("dev").unwrap(), Environment::Development);
    assert_eq!(Environment::from_str("Development").unwrap(), Environment::Development);
    assert_eq!(Environment::from_str("DEVELOPMENT").unwrap(), Environment::Development);
    
    assert_eq!(Environment::from_str("production").unwrap(), Environment::Production);
    assert_eq!(Environment::from_str("prod").unwrap(), Environment::Production);
    assert_eq!(Environment::from_str("Production").unwrap(), Environment::Production);
    assert_eq!(Environment::from_str("PRODUCTION").unwrap(), Environment::Production);
    
    assert_eq!(Environment::from_str("testing").unwrap(), Environment::Testing);
    assert_eq!(Environment::from_str("test").unwrap(), Environment::Testing);
    assert_eq!(Environment::from_str("Testing").unwrap(), Environment::Testing);
    assert_eq!(Environment::from_str("TESTING").unwrap(), Environment::Testing);
    
    // make sure invalid strings fail properly
    assert!(Environment::from_str("invalid").is_err());
    assert!(Environment::from_str("").is_err());
    assert!(Environment::from_str("staging").is_err());
    assert!(Environment::from_str("prod ").is_err()); // trailing space
}

#[test]
fn test_environment_serialization() {
    // check that environment serializes correctly
    let dev_env = Environment::Development;
    let prod_env = Environment::Production;
    let test_env = Environment::Testing;
    
    // should serialize to lowercase due to rename_all = "lowercase"
    let dev_json = serde_json::to_string(&dev_env).unwrap();
    assert_eq!(dev_json, "\"development\"");
    
    let prod_json = serde_json::to_string(&prod_env).unwrap();
    assert_eq!(prod_json, "\"production\"");
    
    let test_json = serde_json::to_string(&test_env).unwrap();
    assert_eq!(test_json, "\"testing\"");
}

#[test]
fn test_environment_config_file_name() {
    assert_eq!(Environment::Development.config_file_name(), "development.toml");
    assert_eq!(Environment::Production.config_file_name(), "production.toml");
    assert_eq!(Environment::Testing.config_file_name(), "testing.toml");
}

#[test]
fn test_environment_helper_methods() {
    let dev_env = Environment::Development;
    let prod_env = Environment::Production;
    let test_env = Environment::Testing;
    
    assert!(dev_env.is_development());
    assert!(!dev_env.is_production());
    
    assert!(!prod_env.is_development());
    assert!(prod_env.is_production());
    
    assert!(!test_env.is_development());
    assert!(!test_env.is_production());
}

#[test]
fn test_server_config_validation_comprehensive() {
    // basic valid config should work
    let valid_config = create_test_server_config();
    assert!(valid_config.validate().is_ok());
    
    // try breaking it in different ways
    let mut invalid_config = create_test_server_config();
    
    // empty host should fail
    invalid_config.host = "".to_string();
    assert!(invalid_config.validate().is_err());
    
    // zero port should fail
    invalid_config = create_test_server_config();
    invalid_config.port = 0;
    assert!(invalid_config.validate().is_err());
    
    // zero max connections should fail
    invalid_config = create_test_server_config();
    invalid_config.max_connections = 0;
    assert!(invalid_config.validate().is_err());
    
    // zero connection timeout should fail
    invalid_config = create_test_server_config();
    invalid_config.connection_timeout = 0;
    assert!(invalid_config.validate().is_err());
    
    // keep_alive_timeout = 0 is actually allowed
}

#[test]
fn test_server_config_socket_addr() {
    let config = create_test_server_config();
    let addr = config.socket_addr().unwrap();
    
    assert_eq!(addr.ip().to_string(), "127.0.0.1");
    assert_eq!(addr.port(), 8081);
    
    // try with different host
    let mut config = create_test_server_config();
    config.host = "0.0.0.0".to_string();
    let addr = config.socket_addr().unwrap();
    assert_eq!(addr.ip().to_string(), "0.0.0.0");
}

#[test]
fn test_logging_config_validation() {
    // basic valid config should work
    let valid_config = create_test_logging_config();
    assert!(valid_config.validate().is_ok());
    
    // try breaking it
    let mut invalid_config = create_test_logging_config();
    
    // empty log level should fail
    invalid_config.level = "".to_string();
    assert!(invalid_config.validate().is_err());
    
    // invalid log level should fail
    invalid_config.level = "invalid_level".to_string();
    assert!(invalid_config.validate().is_err());
}

#[test]
fn test_config_load_development_defaults() {
    let config = Config::load_development_defaults();
    
    assert_eq!(config.environment, Environment::Development);
    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.port, 8081);
    assert_eq!(config.logging.level, "debug");
    assert!(config.logging.colored);
    assert!(!config.logging.structured);
}

#[test]
fn test_config_server_address() {
    let config = Config::load_development_defaults();
    let addr = config.server_address().unwrap();
    
    assert_eq!(addr.ip().to_string(), "127.0.0.1");
    assert_eq!(addr.port(), 8081);
}

#[test]
fn test_config_validation_comprehensive() {
    let mut config = Config::load_development_defaults();
    
    // development should be fine
    assert!(config.validate().is_ok());
    
    // switch to production and test the rules
    config.environment = Environment::Production;
    
    // should fail with localhost
    config.server.host = "127.0.0.1".to_string();
    assert!(config.validate().is_err());
    
    // should also fail with localhost (lowercase)
    config.server.host = "localhost".to_string();
    assert!(config.validate().is_err());
    
    // should pass with 0.0.0.0 and info logging
    config.server.host = "0.0.0.0".to_string();
    config.logging.level = "info".to_string();
    assert!(config.validate().is_ok());
    
    // should fail with debug logging in production
    config.logging.level = "debug".to_string();
    assert!(config.validate().is_err());
    
    // should fail with trace logging in production
    config.logging.level = "trace".to_string();
    assert!(config.validate().is_err());
    
    // should pass with info logging in production
    config.logging.level = "info".to_string();
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_validation_testing_environment() {
    let mut config = Config::load_development_defaults();
    config.environment = Environment::Testing;
    
    // should fail with privileged port
    config.server.port = 80;
    assert!(config.validate().is_err());
    
    // should pass with non-privileged port
    config.server.port = 8081;
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_validation_development_environment() {
    let mut config = Config::load_development_defaults();
    
    // should fail with too many connections
    config.server.max_connections = 15000;
    assert!(config.validate().is_err());
    
    // should pass with reasonable connections
    config.server.max_connections = 5000;
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_helper_methods() {
    let mut config = Config::load_development_defaults();
    
    assert!(config.is_development());
    assert!(!config.is_production());
    
    config.environment = Environment::Production;
    assert!(!config.is_development());
    assert!(config.is_production());
}

#[test]
fn test_config_description() {
    let config = Config::load_development_defaults();
    let description = config.description();
    
    assert!(description.contains("VEX Gateway Service Configuration"));
    assert!(description.contains("Environment: Development"));
    assert!(description.contains("Server: 127.0.0.1:8081"));
    assert!(description.contains("Log Level: debug"));
}

#[test]
fn test_config_default_implementation() {
    let config = Config::default();
    
    // default should be development defaults
    assert_eq!(config.environment, Environment::Development);
    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.port, 8081);
}

#[test]
fn test_config_clone() {
    let config = Config::load_development_defaults();
    let cloned = config.clone();
    
    assert_eq!(config.environment, cloned.environment);
    assert_eq!(config.server.host, cloned.server.host);
    assert_eq!(config.server.port, cloned.server.port);
    assert_eq!(config.logging.level, cloned.logging.level);
}

#[test]
fn test_config_debug() {
    let config = Config::load_development_defaults();
    let debug_str = format!("{:?}", config);
    
    assert!(!debug_str.is_empty());
    assert!(debug_str.len() > 50);
}

#[test]
fn test_server_config_clone() {
    let config = create_test_server_config();
    let cloned = config.clone();
    
    assert_eq!(config.host, cloned.host);
    assert_eq!(config.port, cloned.port);
    assert_eq!(config.max_connections, cloned.max_connections);
}

#[test]
fn test_logging_config_clone() {
    let config = create_test_logging_config();
    let cloned = config.clone();
    
    assert_eq!(config.level, cloned.level);
    assert_eq!(config.structured, cloned.structured);
    assert_eq!(config.colored, cloned.colored);
}

#[test]
fn test_environment_clone() {
    let env = Environment::Development;
    let cloned = env.clone();
    
    assert_eq!(env, cloned);
}

#[test]
fn test_environment_debug() {
    let env = Environment::Production;
    let debug_str = format!("{:?}", env);
    
    assert!(!debug_str.is_empty());
    assert!(debug_str.len() > 5);
}
