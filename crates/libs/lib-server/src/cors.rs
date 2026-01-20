//! CORS (Cross-Origin Resource Sharing) Configuration
//!
//! Provides configurable CORS middleware for the API server.
//!
//! # Configuration
//!
//! CORS can be configured via environment variables:
//! - `CORS_ALLOWED_ORIGINS`: Comma-separated list of allowed origins (default: uses FRONTEND_URL)
//! - `CORS_ALLOW_CREDENTIALS`: Whether to allow credentials (default: true)
//! - `CORS_MAX_AGE_SECS`: Preflight cache duration in seconds (default: 3600)
//!
//! # Security Considerations
//!
//! - In production, always specify explicit origins (never use wildcard with credentials)
//! - The default configuration uses FRONTEND_URL which should be set appropriately per environment
//! - Credentials are enabled by default for cookie-based authentication

use actix_cors::Cors;
use actix_web::http::{header, Method};
use tracing::info;

/// CORS configuration settings
#[derive(Debug, Clone)]
pub struct CorsConfig {
    /// Allowed origins (empty = use FRONTEND_URL)
    pub allowed_origins: Vec<String>,
    /// Whether to allow credentials (cookies, authorization headers)
    pub allow_credentials: bool,
    /// Max age for preflight cache (seconds)
    pub max_age_secs: u64,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allow_credentials: true,
            max_age_secs: 3600, // 1 hour
        }
    }
}

impl CorsConfig {
    /// Create CorsConfig from environment variables
    ///
    /// Environment variables:
    /// - `CORS_ALLOWED_ORIGINS`: Comma-separated list of allowed origins
    /// - `CORS_ALLOW_CREDENTIALS`: "true" or "false" (default: true)
    /// - `CORS_MAX_AGE_SECS`: Preflight cache duration (default: 3600)
    pub fn from_env() -> Self {
        let allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|o| o.trim().to_string())
                    .filter(|o| !o.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let allow_credentials = std::env::var("CORS_ALLOW_CREDENTIALS")
            .map(|s| s.to_lowercase() != "false")
            .unwrap_or(true);

        let max_age_secs = std::env::var("CORS_MAX_AGE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);

        Self {
            allowed_origins,
            allow_credentials,
            max_age_secs,
        }
    }

    /// Create a permissive CORS config for development
    pub fn permissive() -> Self {
        Self {
            allowed_origins: Vec::new(), // Will use permissive() in build
            allow_credentials: true,
            max_age_secs: 3600,
        }
    }
}

/// Build CORS middleware from configuration
///
/// If no allowed origins are specified, uses FRONTEND_URL environment variable.
/// Falls back to localhost:8081 for development.
pub fn build_cors(config: &CorsConfig) -> Cors {
    let frontend_url = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string());

    // Determine allowed origins
    let origins: Vec<String> = if config.allowed_origins.is_empty() {
        vec![frontend_url.clone()]
    } else {
        config.allowed_origins.clone()
    };

    info!(
        origins = ?origins,
        allow_credentials = config.allow_credentials,
        max_age_secs = config.max_age_secs,
        "Configuring CORS middleware"
    );

    let mut cors = Cors::default()
        // Allowed HTTP methods
        .allowed_methods(vec![
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        // Allowed headers
        .allowed_headers(vec![
            header::AUTHORIZATION,
            header::ACCEPT,
            header::CONTENT_TYPE,
            header::ORIGIN,
            header::HeaderName::from_static("x-request-id"),
            header::HeaderName::from_static("x-api-key"),
            header::HeaderName::from_static("x-signature"),
            header::HeaderName::from_static("x-timestamp"),
        ])
        // Expose headers to client
        .expose_headers(vec![
            header::HeaderName::from_static("x-request-id"),
            header::HeaderName::from_static("x-ratelimit-limit"),
            header::HeaderName::from_static("x-ratelimit-remaining"),
            header::HeaderName::from_static("retry-after"),
        ])
        // Preflight cache duration
        .max_age(config.max_age_secs as usize);

    // Add allowed origins
    for origin in &origins {
        cors = cors.allowed_origin(origin);
    }

    // Set credentials support
    if config.allow_credentials {
        cors = cors.supports_credentials();
    }

    cors
}

/// Build a permissive CORS middleware for development
///
/// WARNING: This should only be used in development environments.
/// It allows all origins which is insecure for production.
pub fn build_permissive_cors() -> Cors {
    info!("Building permissive CORS middleware (development only)");

    Cors::permissive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CorsConfig::default();
        assert!(config.allowed_origins.is_empty());
        assert!(config.allow_credentials);
        assert_eq!(config.max_age_secs, 3600);
    }

    #[test]
    fn test_permissive_config() {
        let config = CorsConfig::permissive();
        assert!(config.allowed_origins.is_empty());
        assert!(config.allow_credentials);
    }

    #[test]
    fn test_build_cors() {
        let config = CorsConfig {
            allowed_origins: vec!["https://example.com".to_string()],
            allow_credentials: true,
            max_age_secs: 7200,
        };

        // Should not panic
        let _cors = build_cors(&config);
    }
}
