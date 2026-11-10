//! lib-config/src/metrics.rs

use crate::ConfigError;
use serde::Deserialize;
use std::net::SocketAddr;

/// The internal listener that serves the Prometheus metrics: the `[metrics]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    /// The IP address and port it binds, such as `127.0.0.1:9091`: reachable by the metrics
    /// scraper only, never through the load balancer. Production requires a port of its own.
    pub bind: String,
}

impl MetricsConfig {
    /// `bind` as a socket address. The error never shows the value.
    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.bind.trim().parse().map_err(|_| {
            ConfigError::ValidationError(
                "metrics.bind must be an IP address and a port, such as 127.0.0.1:9091".to_string(),
            )
        })
    }
}
