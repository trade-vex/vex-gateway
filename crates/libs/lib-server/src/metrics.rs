//! Prometheus Metrics Configuration
//!
//! Provides Prometheus metrics middleware and custom business metrics for the API server.
//!
//! # Endpoints
//!
//! - `/metrics` - Prometheus metrics scrape endpoint
//!
//! # Configuration
//!
//! Metrics can be configured via environment variables:
//! - `METRICS_ENABLED`: Enable/disable metrics collection (default: true)
//! - `METRICS_PATH`: Path for metrics endpoint (default: /metrics)
//! - `METRICS_NAMESPACE`: Prometheus namespace for metrics (default: vex_gateway)
//!
//! # Metrics Collected
//!
//! ## HTTP Metrics (via actix-web-prom)
//! - `http_requests_total` - Total HTTP requests by method, path, status
//! - `http_request_duration_seconds` - Request duration histogram
//!
//! ## Custom Business Metrics
//! - `orders_submitted_total` - Total orders submitted
//! - `orders_cancelled_total` - Total orders cancelled
//! - `active_sessions` - Current active session count
//! - `api_key_cache_hits_total` - API key cache hits
//! - `api_key_cache_misses_total` - API key cache misses

use actix_web_prom::{PrometheusMetrics, PrometheusMetricsBuilder};
use prometheus::{Counter, Gauge, Registry, opts};
use std::sync::Arc;
use tracing::info;

/// Metrics configuration
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Whether metrics are enabled
    pub enabled: bool,
    /// Path for metrics endpoint
    pub path: String,
    /// Prometheus namespace
    pub namespace: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "/metrics".to_string(),
            namespace: "vex_gateway".to_string(),
        }
    }
}

impl MetricsConfig {
    /// Create MetricsConfig from environment variables
    pub fn from_env() -> Self {
        let enabled = std::env::var("METRICS_ENABLED")
            .map(|s| s.to_lowercase() != "false")
            .unwrap_or(true);

        let path = std::env::var("METRICS_PATH")
            .unwrap_or_else(|_| "/metrics".to_string());

        let namespace = std::env::var("METRICS_NAMESPACE")
            .unwrap_or_else(|_| "vex_gateway".to_string());

        Self {
            enabled,
            path,
            namespace,
        }
    }
}

/// Custom business metrics for the gateway
#[derive(Clone)]
pub struct BusinessMetrics {
    /// Total orders submitted
    pub orders_submitted: Counter,
    /// Total orders cancelled
    pub orders_cancelled: Counter,
    /// Total orders rejected
    pub orders_rejected: Counter,
    /// Current active sessions
    pub active_sessions: Gauge,
    /// API key cache hits
    pub cache_hits: Counter,
    /// API key cache misses
    pub cache_misses: Counter,
    /// Authentication failures
    pub auth_failures: Counter,
    /// Rate limit hits
    pub rate_limit_hits: Counter,
}

impl BusinessMetrics {
    /// Create new BusinessMetrics and register with the given registry
    pub fn new(registry: &Registry, namespace: &str) -> Result<Self, prometheus::Error> {
        let orders_submitted = Counter::with_opts(
            opts!("orders_submitted_total", "Total orders submitted")
                .namespace(namespace),
        )?;
        registry.register(Box::new(orders_submitted.clone()))?;

        let orders_cancelled = Counter::with_opts(
            opts!("orders_cancelled_total", "Total orders cancelled")
                .namespace(namespace),
        )?;
        registry.register(Box::new(orders_cancelled.clone()))?;

        let orders_rejected = Counter::with_opts(
            opts!("orders_rejected_total", "Total orders rejected")
                .namespace(namespace),
        )?;
        registry.register(Box::new(orders_rejected.clone()))?;

        let active_sessions = Gauge::with_opts(
            opts!("active_sessions", "Current number of active sessions")
                .namespace(namespace),
        )?;
        registry.register(Box::new(active_sessions.clone()))?;

        let cache_hits = Counter::with_opts(
            opts!("api_key_cache_hits_total", "API key cache hits")
                .namespace(namespace),
        )?;
        registry.register(Box::new(cache_hits.clone()))?;

        let cache_misses = Counter::with_opts(
            opts!("api_key_cache_misses_total", "API key cache misses")
                .namespace(namespace),
        )?;
        registry.register(Box::new(cache_misses.clone()))?;

        let auth_failures = Counter::with_opts(
            opts!("auth_failures_total", "Authentication failures")
                .namespace(namespace),
        )?;
        registry.register(Box::new(auth_failures.clone()))?;

        let rate_limit_hits = Counter::with_opts(
            opts!("rate_limit_hits_total", "Rate limit exceeded events")
                .namespace(namespace),
        )?;
        registry.register(Box::new(rate_limit_hits.clone()))?;

        Ok(Self {
            orders_submitted,
            orders_cancelled,
            orders_rejected,
            active_sessions,
            cache_hits,
            cache_misses,
            auth_failures,
            rate_limit_hits,
        })
    }

    /// Record an order submission
    pub fn record_order_submitted(&self) {
        self.orders_submitted.inc();
    }

    /// Record an order cancellation
    pub fn record_order_cancelled(&self) {
        self.orders_cancelled.inc();
    }

    /// Record an order rejection
    pub fn record_order_rejected(&self) {
        self.orders_rejected.inc();
    }

    /// Update active session count
    pub fn set_active_sessions(&self, count: i64) {
        self.active_sessions.set(count as f64);
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self) {
        self.cache_hits.inc();
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self) {
        self.cache_misses.inc();
    }

    /// Record an authentication failure
    pub fn record_auth_failure(&self) {
        self.auth_failures.inc();
    }

    /// Record a rate limit hit
    pub fn record_rate_limit_hit(&self) {
        self.rate_limit_hits.inc();
    }
}

/// Build Prometheus metrics middleware
///
/// Returns the PrometheusMetrics middleware and BusinessMetrics for custom instrumentation
pub fn build_metrics(
    config: &MetricsConfig,
) -> Result<(PrometheusMetrics, Arc<BusinessMetrics>), Box<dyn std::error::Error>> {
    if !config.enabled {
        info!("Metrics collection disabled");
        // Return a minimal setup that won't collect metrics
        let registry = Registry::new();
        let prometheus = PrometheusMetricsBuilder::new(&config.namespace)
            .registry(registry.clone())
            .endpoint(&config.path)
            .build()
            .map_err(|e| format!("Failed to build prometheus metrics: {}", e))?;

        let business_metrics = BusinessMetrics::new(&registry, &config.namespace)?;
        return Ok((prometheus, Arc::new(business_metrics)));
    }

    info!(
        enabled = config.enabled,
        path = %config.path,
        namespace = %config.namespace,
        "Configuring Prometheus metrics"
    );

    // Create a custom registry
    let registry = Registry::new();

    // Create business metrics
    let business_metrics = BusinessMetrics::new(&registry, &config.namespace)?;

    // Build the Prometheus middleware with custom registry
    let prometheus = PrometheusMetricsBuilder::new(&config.namespace)
        .registry(registry)
        .endpoint(&config.path)
        .build()
        .map_err(|e| format!("Failed to build prometheus metrics: {}", e))?;

    Ok((prometheus, Arc::new(business_metrics)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MetricsConfig::default();
        assert!(config.enabled);
        assert_eq!(config.path, "/metrics");
        assert_eq!(config.namespace, "vex_gateway");
    }

    #[test]
    fn test_business_metrics_creation() {
        let registry = Registry::new();
        let metrics = BusinessMetrics::new(&registry, "test").unwrap();

        // Test counter increments
        metrics.record_order_submitted();
        metrics.record_order_cancelled();
        metrics.record_cache_hit();

        // Verify counters were incremented
        assert_eq!(metrics.orders_submitted.get(), 1.0);
        assert_eq!(metrics.orders_cancelled.get(), 1.0);
        assert_eq!(metrics.cache_hits.get(), 1.0);
    }

    #[test]
    fn test_build_metrics() {
        let config = MetricsConfig::default();
        let result = build_metrics(&config);
        assert!(result.is_ok());
    }
}
