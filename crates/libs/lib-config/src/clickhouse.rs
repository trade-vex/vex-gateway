//! lib-config/src/clickhouse.rs

use crate::ConfigError;
use secrecy::SecretString;
use serde::Deserialize;

/// ClickHouse, which serves the portfolio and balance-chart reads: the `[clickhouse]` section.
/// `APP__CLICKHOUSE__<KEY>` overrides a key, such as `APP__CLICKHOUSE__PASSWORD`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClickHouseConfig {
    /// The HTTP endpoint, such as `http://clickhouse.internal:8123`.
    pub url: String,
    /// The user the gateway reads as.
    pub user: String,
    /// That user's password, never logged. Empty only for a user without one, such as a local
    /// server's `default`; production requires it.
    pub password: SecretString,
    /// The database holding the portfolio tables. Every query names it before the table.
    pub database: String,
}

impl ClickHouseConfig {
    /// `user` cannot be empty, and `database` must be a plain identifier (ASCII letters, digits and
    /// underscores, not starting with a digit): the queries name it unquoted.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.user.trim().is_empty() {
            return Err(ConfigError::ValidationError(
                "clickhouse.user cannot be empty".to_string(),
            ));
        }
        let mut database = self.database.chars();
        let plain_identifier = database
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && database.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !plain_identifier {
            return Err(ConfigError::ValidationError(
                "clickhouse.database must be a plain identifier: ASCII letters, digits and \
                 underscores, not starting with a digit"
                    .to_string(),
            ));
        }
        Ok(())
    }
}
