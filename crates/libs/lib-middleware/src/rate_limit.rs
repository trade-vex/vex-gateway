//! Rate Limiting Middleware
//!
//! Provides configurable rate limiting for API endpoints using the token bucket algorithm.
//! Supports both IP-based and user-based rate limiting with different limits for different
//! endpoint categories.
//!
//! # Features
//!
//! - IP-based rate limiting for unauthenticated requests
//! - User-based rate limiting for authenticated requests
//! - Configurable limits per endpoint category (auth, trading, general)
//! - Proper 429 Too Many Requests responses with Retry-After header
//!
//! # Usage
//!
//! ```rust,ignore
//! use lib_middleware::rate_limit::{RateLimitConfig, create_rate_limiter};
//!
//! // Create rate limiter with custom config
//! let config = RateLimitConfig::default();
//! let rate_limiter = create_rate_limiter(config);
//!
//! App::new()
//!     .wrap(rate_limiter)
//!     // ... routes
//! ```

use actix_governor::{Governor, GovernorConfigBuilder, KeyExtractor, SimpleKeyExtractionError};
use actix_web::dev::ServiceRequest;
use governor::clock::QuantaInstant;
use governor::middleware::NoOpMiddleware;
use std::net::IpAddr;
use std::time::Duration;

/// Rate limit configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Requests per second for general endpoints
    pub general_rps: u32,
    /// Burst size for general endpoints
    pub general_burst: u32,
    /// Requests per second for authentication endpoints (more strict)
    pub auth_rps: u32,
    /// Burst size for authentication endpoints
    pub auth_burst: u32,
    /// Requests per second for trading endpoints
    pub trading_rps: u32,
    /// Burst size for trading endpoints
    pub trading_burst: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            // General API: 100 requests per second with burst of 200
            general_rps: 100,
            general_burst: 200,
            // Auth endpoints: 10 requests per second with burst of 20
            // More restrictive to prevent brute force
            auth_rps: 10,
            auth_burst: 20,
            // Trading: 50 requests per second with burst of 100
            trading_rps: 50,
            trading_burst: 100,
        }
    }
}

impl RateLimitConfig {
    /// Create a strict rate limit config for high-security scenarios
    pub fn strict() -> Self {
        Self {
            general_rps: 50,
            general_burst: 100,
            auth_rps: 5,
            auth_burst: 10,
            trading_rps: 25,
            trading_burst: 50,
        }
    }

    /// Create a permissive rate limit config for development/testing
    pub fn permissive() -> Self {
        Self {
            general_rps: 500,
            general_burst: 1000,
            auth_rps: 50,
            auth_burst: 100,
            trading_rps: 200,
            trading_burst: 500,
        }
    }

    /// Load rate limit configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RATE_LIMIT_GENERAL_RPS` - Requests per second for general endpoints (default: 100)
    /// - `RATE_LIMIT_GENERAL_BURST` - Burst size for general endpoints (default: 200)
    /// - `RATE_LIMIT_AUTH_RPS` - Requests per second for auth endpoints (default: 10)
    /// - `RATE_LIMIT_AUTH_BURST` - Burst size for auth endpoints (default: 20)
    /// - `RATE_LIMIT_TRADING_RPS` - Requests per second for trading endpoints (default: 50)
    /// - `RATE_LIMIT_TRADING_BURST` - Burst size for trading endpoints (default: 100)
    ///
    /// # Example
    /// ```bash
    /// export RATE_LIMIT_GENERAL_RPS=200
    /// export RATE_LIMIT_GENERAL_BURST=400
    /// ```
    pub fn from_env() -> Self {
        let default = Self::default();

        Self {
            general_rps: std::env::var("RATE_LIMIT_GENERAL_RPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.general_rps),
            general_burst: std::env::var("RATE_LIMIT_GENERAL_BURST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.general_burst),
            auth_rps: std::env::var("RATE_LIMIT_AUTH_RPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.auth_rps),
            auth_burst: std::env::var("RATE_LIMIT_AUTH_BURST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.auth_burst),
            trading_rps: std::env::var("RATE_LIMIT_TRADING_RPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.trading_rps),
            trading_burst: std::env::var("RATE_LIMIT_TRADING_BURST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.trading_burst),
        }
    }

    /// Load rate limit configuration based on environment name.
    ///
    /// - "production" -> default (balanced)
    /// - "development" or "testing" -> permissive
    /// - other -> strict
    ///
    /// Environment variables always override these defaults.
    pub fn for_environment(env_name: &str) -> Self {
        let base = match env_name.to_lowercase().as_str() {
            "production" => Self::default(),
            "development" | "testing" | "test" => Self::permissive(),
            _ => Self::strict(),
        };

        // Allow environment variables to override
        Self {
            general_rps: std::env::var("RATE_LIMIT_GENERAL_RPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.general_rps),
            general_burst: std::env::var("RATE_LIMIT_GENERAL_BURST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.general_burst),
            auth_rps: std::env::var("RATE_LIMIT_AUTH_RPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.auth_rps),
            auth_burst: std::env::var("RATE_LIMIT_AUTH_BURST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.auth_burst),
            trading_rps: std::env::var("RATE_LIMIT_TRADING_RPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.trading_rps),
            trading_burst: std::env::var("RATE_LIMIT_TRADING_BURST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.trading_burst),
        }
    }
}

/// Key extractor that uses client IP address
/// Falls back to 127.0.0.1 if IP cannot be determined
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpKeyExtractor;

impl KeyExtractor for IpKeyExtractor {
    type Key = IpAddr;
    type KeyExtractionError = SimpleKeyExtractionError<&'static str>;

    fn extract(&self, req: &ServiceRequest) -> Result<Self::Key, Self::KeyExtractionError> {
        // Try to get real IP from X-Forwarded-For header first (for proxied requests)
        let ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
            // Fall back to X-Real-IP header
            .or_else(|| {
                req.headers()
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<IpAddr>().ok())
            })
            // Fall back to peer address
            .or_else(|| {
                req.connection_info()
                    .peer_addr()
                    .and_then(|s| s.parse::<IpAddr>().ok())
            })
            // Final fallback
            .unwrap_or_else(|| "127.0.0.1".parse().unwrap());

        Ok(ip)
    }
}

/// Create a rate limiter governor for general API endpoints
pub fn create_rate_limiter(
    config: &RateLimitConfig,
) -> Governor<IpKeyExtractor, NoOpMiddleware<QuantaInstant>> {
    // Use seconds_per_request instead of deprecated per_second
    // For 100 RPS, that's 1/100 = 0.01 seconds per request = 10ms
    let seconds_per_request = 1.0 / config.general_rps as f64;
    let period = Duration::from_secs_f64(seconds_per_request);

    let governor_config = GovernorConfigBuilder::default()
        .period(period)
        .burst_size(config.general_burst)
        .key_extractor(IpKeyExtractor)
        .finish()
        .expect("Failed to build governor config");

    Governor::new(&governor_config)
}

/// Create a strict rate limiter for authentication endpoints
pub fn create_auth_rate_limiter(
    config: &RateLimitConfig,
) -> Governor<IpKeyExtractor, NoOpMiddleware<QuantaInstant>> {
    let seconds_per_request = 1.0 / config.auth_rps as f64;
    let period = Duration::from_secs_f64(seconds_per_request);

    let governor_config = GovernorConfigBuilder::default()
        .period(period)
        .burst_size(config.auth_burst)
        .key_extractor(IpKeyExtractor)
        .finish()
        .expect("Failed to build auth governor config");

    Governor::new(&governor_config)
}

/// Create a rate limiter for trading endpoints
pub fn create_trading_rate_limiter(
    config: &RateLimitConfig,
) -> Governor<IpKeyExtractor, NoOpMiddleware<QuantaInstant>> {
    let seconds_per_request = 1.0 / config.trading_rps as f64;
    let period = Duration::from_secs_f64(seconds_per_request);

    let governor_config = GovernorConfigBuilder::default()
        .period(period)
        .burst_size(config.trading_burst)
        .key_extractor(IpKeyExtractor)
        .finish()
        .expect("Failed to build trading governor config");

    Governor::new(&governor_config)
}

/// Create a rate limiter with custom replenish interval
/// Useful for very strict rate limits (e.g., 1 request per minute)
pub fn create_custom_rate_limiter(
    replenish_interval: Duration,
    burst_size: u32,
) -> Governor<IpKeyExtractor, NoOpMiddleware<QuantaInstant>> {
    let governor_config = GovernorConfigBuilder::default()
        .period(replenish_interval)
        .burst_size(burst_size)
        .key_extractor(IpKeyExtractor)
        .finish()
        .expect("Failed to build custom governor config");

    Governor::new(&governor_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.general_rps, 100);
        assert_eq!(config.general_burst, 200);
        assert_eq!(config.auth_rps, 10);
        assert_eq!(config.auth_burst, 20);
    }

    #[test]
    fn test_strict_config() {
        let config = RateLimitConfig::strict();
        assert_eq!(config.general_rps, 50);
        assert_eq!(config.auth_rps, 5);
    }

    #[test]
    fn test_permissive_config() {
        let config = RateLimitConfig::permissive();
        assert_eq!(config.general_rps, 500);
        assert_eq!(config.auth_rps, 50);
    }

    #[test]
    fn test_create_rate_limiter() {
        let config = RateLimitConfig::default();
        // Should not panic
        let _limiter = create_rate_limiter(&config);
    }

    #[test]
    fn test_create_auth_rate_limiter() {
        let config = RateLimitConfig::default();
        // Should not panic
        let _limiter = create_auth_rate_limiter(&config);
    }

    #[test]
    fn test_create_trading_rate_limiter() {
        let config = RateLimitConfig::default();
        // Should not panic
        let _limiter = create_trading_rate_limiter(&config);
    }

    #[test]
    fn test_custom_rate_limiter() {
        // 1 request per minute
        let _limiter = create_custom_rate_limiter(Duration::from_secs(60), 1);
    }

    #[test]
    fn test_from_env_defaults() {
        // Clear any existing env vars to test defaults
        std::env::remove_var("RATE_LIMIT_GENERAL_RPS");
        std::env::remove_var("RATE_LIMIT_GENERAL_BURST");

        let config = RateLimitConfig::from_env();
        let default = RateLimitConfig::default();

        assert_eq!(config.general_rps, default.general_rps);
        assert_eq!(config.general_burst, default.general_burst);
    }

    #[test]
    fn test_for_environment_production() {
        let config = RateLimitConfig::for_environment("production");
        let default = RateLimitConfig::default();
        assert_eq!(config.general_rps, default.general_rps);
    }

    #[test]
    fn test_for_environment_development() {
        let config = RateLimitConfig::for_environment("development");
        let permissive = RateLimitConfig::permissive();
        assert_eq!(config.general_rps, permissive.general_rps);
    }

    #[test]
    fn test_for_environment_testing() {
        let config = RateLimitConfig::for_environment("testing");
        let permissive = RateLimitConfig::permissive();
        assert_eq!(config.general_rps, permissive.general_rps);
    }

    #[test]
    fn test_for_environment_unknown() {
        let config = RateLimitConfig::for_environment("unknown");
        let strict = RateLimitConfig::strict();
        assert_eq!(config.general_rps, strict.general_rps);
    }
}
