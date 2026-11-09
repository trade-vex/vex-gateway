//! lib-config/src/clickhouse.rs

use serde::Deserialize;

/// ClickHouse, which serves the portfolio and balance-chart reads: the `[clickhouse]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct ClickHouseConfig {
    /// The HTTP endpoint, such as `http://clickhouse.internal:8123`.
    pub url: String,
}
