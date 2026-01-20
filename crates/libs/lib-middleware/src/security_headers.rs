//! Security Headers Middleware
//!
//! Adds security-related HTTP headers to all responses to protect against
//! common web vulnerabilities.
//!
//! # Headers Added
//!
//! - `X-Content-Type-Options: nosniff` - Prevents MIME type sniffing
//! - `X-Frame-Options: DENY` - Prevents clickjacking via iframes
//! - `X-XSS-Protection: 1; mode=block` - Legacy XSS filter (for older browsers)
//! - `Referrer-Policy: strict-origin-when-cross-origin` - Controls referrer information
//! - `Cache-Control: no-store` - Prevents caching of sensitive API responses
//! - `Permissions-Policy` - Restricts browser features
//!
//! # Usage
//!
//! ```rust,ignore
//! use lib_middleware::SecurityHeadersMiddleware;
//!
//! App::new()
//!     .wrap(SecurityHeadersMiddleware::default())
//!     // ... routes
//! ```

use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    http::header::{HeaderName, HeaderValue},
    Error,
};
use futures_util::future::LocalBoxFuture;
use std::{
    future::{ready, Ready},
    rc::Rc,
};

/// Configuration for security headers
#[derive(Debug, Clone)]
pub struct SecurityHeadersConfig {
    /// X-Content-Type-Options header value (default: "nosniff")
    pub content_type_options: Option<&'static str>,
    /// X-Frame-Options header value (default: "DENY")
    pub frame_options: Option<&'static str>,
    /// X-XSS-Protection header value (default: "1; mode=block")
    pub xss_protection: Option<&'static str>,
    /// Referrer-Policy header value (default: "strict-origin-when-cross-origin")
    pub referrer_policy: Option<&'static str>,
    /// Cache-Control header value (default: "no-store" for API responses)
    pub cache_control: Option<&'static str>,
    /// Permissions-Policy header value
    pub permissions_policy: Option<&'static str>,
    /// Content-Security-Policy header value (optional, set for specific needs)
    pub content_security_policy: Option<String>,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            content_type_options: Some("nosniff"),
            frame_options: Some("DENY"),
            xss_protection: Some("1; mode=block"),
            referrer_policy: Some("strict-origin-when-cross-origin"),
            cache_control: Some("no-store"),
            permissions_policy: Some("geolocation=(), microphone=(), camera=()"),
            content_security_policy: None,
        }
    }
}

impl SecurityHeadersConfig {
    /// Create a new config with all headers disabled
    pub fn none() -> Self {
        Self {
            content_type_options: None,
            frame_options: None,
            xss_protection: None,
            referrer_policy: None,
            cache_control: None,
            permissions_policy: None,
            content_security_policy: None,
        }
    }

    /// Set Content-Security-Policy header
    pub fn with_csp(mut self, csp: impl Into<String>) -> Self {
        self.content_security_policy = Some(csp.into());
        self
    }

    /// Disable cache control header (useful if you want to allow caching)
    pub fn allow_caching(mut self) -> Self {
        self.cache_control = None;
        self
    }

    /// Set frame options to SAMEORIGIN instead of DENY
    pub fn allow_same_origin_frames(mut self) -> Self {
        self.frame_options = Some("SAMEORIGIN");
        self
    }
}

/// Security headers middleware
pub struct SecurityHeadersMiddleware {
    config: SecurityHeadersConfig,
}

impl SecurityHeadersMiddleware {
    pub fn new(config: SecurityHeadersConfig) -> Self {
        Self { config }
    }
}

impl Default for SecurityHeadersMiddleware {
    fn default() -> Self {
        Self::new(SecurityHeadersConfig::default())
    }
}

impl<S, B> Transform<S, ServiceRequest> for SecurityHeadersMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = SecurityHeadersService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(SecurityHeadersService {
            service: Rc::new(service),
            config: self.config.clone(),
        }))
    }
}

pub struct SecurityHeadersService<S> {
    service: Rc<S>,
    config: SecurityHeadersConfig,
}

impl<S, B> Service<ServiceRequest> for SecurityHeadersService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);
        let config = self.config.clone();

        Box::pin(async move {
            let res = service.call(req).await?;

            // Add security headers to response
            let (req, mut res) = res.into_parts();
            let headers = res.headers_mut();

            // X-Content-Type-Options
            if let Some(value) = config.content_type_options {
                headers.insert(
                    HeaderName::from_static("x-content-type-options"),
                    HeaderValue::from_static(value),
                );
            }

            // X-Frame-Options
            if let Some(value) = config.frame_options {
                headers.insert(
                    HeaderName::from_static("x-frame-options"),
                    HeaderValue::from_static(value),
                );
            }

            // X-XSS-Protection
            if let Some(value) = config.xss_protection {
                headers.insert(
                    HeaderName::from_static("x-xss-protection"),
                    HeaderValue::from_static(value),
                );
            }

            // Referrer-Policy
            if let Some(value) = config.referrer_policy {
                headers.insert(
                    HeaderName::from_static("referrer-policy"),
                    HeaderValue::from_static(value),
                );
            }

            // Cache-Control
            if let Some(value) = config.cache_control {
                headers.insert(
                    HeaderName::from_static("cache-control"),
                    HeaderValue::from_static(value),
                );
            }

            // Permissions-Policy
            if let Some(value) = config.permissions_policy {
                headers.insert(
                    HeaderName::from_static("permissions-policy"),
                    HeaderValue::from_static(value),
                );
            }

            // Content-Security-Policy (if configured)
            if let Some(ref value) = config.content_security_policy {
                if let Ok(header_value) = HeaderValue::from_str(value) {
                    headers.insert(
                        HeaderName::from_static("content-security-policy"),
                        header_value,
                    );
                }
            }

            Ok(ServiceResponse::new(req, res))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SecurityHeadersConfig::default();
        assert_eq!(config.content_type_options, Some("nosniff"));
        assert_eq!(config.frame_options, Some("DENY"));
        assert_eq!(config.xss_protection, Some("1; mode=block"));
        assert_eq!(
            config.referrer_policy,
            Some("strict-origin-when-cross-origin")
        );
        assert_eq!(config.cache_control, Some("no-store"));
        assert!(config.permissions_policy.is_some());
        assert!(config.content_security_policy.is_none());
    }

    #[test]
    fn test_none_config() {
        let config = SecurityHeadersConfig::none();
        assert!(config.content_type_options.is_none());
        assert!(config.frame_options.is_none());
        assert!(config.xss_protection.is_none());
        assert!(config.referrer_policy.is_none());
        assert!(config.cache_control.is_none());
        assert!(config.permissions_policy.is_none());
        assert!(config.content_security_policy.is_none());
    }

    #[test]
    fn test_config_with_csp() {
        let config = SecurityHeadersConfig::default().with_csp("default-src 'self'");
        assert_eq!(
            config.content_security_policy,
            Some("default-src 'self'".to_string())
        );
    }

    #[test]
    fn test_allow_caching() {
        let config = SecurityHeadersConfig::default().allow_caching();
        assert!(config.cache_control.is_none());
    }

    #[test]
    fn test_allow_same_origin_frames() {
        let config = SecurityHeadersConfig::default().allow_same_origin_frames();
        assert_eq!(config.frame_options, Some("SAMEORIGIN"));
    }
}
