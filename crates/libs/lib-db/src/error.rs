use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    /// Connection-related errors (pool exhausted, network issues, auth failures)
    #[error("connection error: {message}")]
    Connection {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Query execution errors (syntax errors, constraint violations)
    #[error("query execution failed: {message}")]
    QueryExecution {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Data not found (useful for 404 responses)
    #[error("entity not found: {entity}")]
    NotFound { entity: String },

    /// Serialization/Deserialization errors
    #[error("serialization error: {message}")]
    Serialization {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Transaction-related errors
    #[error("transaction failed: {message}")]
    Transaction {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Timeout errors (connection or query timeout)
    #[error("operation timed out after {duration_ms}ms")]
    Timeout { duration_ms: u64 },

    /// Pool errors (exhausted, closed)
    #[error("connection pool error: {message}")]
    Pool { message: String },

    /// Configuration errors
    #[error("configuration error: {message}")]
    Configuration { message: String },

    /// Feature not enabled
    #[error("feature not enabled: {feature} (enable it in Cargo.toml)")]
    FeatureNotEnabled { feature: &'static str },

    /// Rate limiting
    #[error("rate limit exceeded: {message}")]
    RateLimited {
        message: String,
        retry_after_ms: Option<u64>,
    },

    /// Generic internal error (for unexpected cases)
    #[error("internal database error")]
    Internal {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Result type alias
pub type Result<T> = std::result::Result<T, DbError>;

// ============================================================================
// POSTGRESQL ERROR CONVERSION
// ============================================================================

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        use sqlx::Error;

        match err {
            // Not found errors -> 404
            Error::RowNotFound => DbError::NotFound {
                entity: "row".to_string(),
            },

            // Connection/Pool errors
            Error::PoolTimedOut => DbError::Timeout {
                duration_ms: 30000, // Default pool timeout
            },

            Error::PoolClosed => DbError::Pool {
                message: "connection pool is closed".to_string(),
            },

            Error::Configuration(msg) => DbError::Configuration {
                message: msg.to_string(),
            },

            // Database errors (constraint violations, etc.)
            Error::Database(db_err) => {
                // Check for specific database error codes
                if let Some(code) = db_err.code() {
                    match code.as_ref() {
                        // PostgreSQL error codes
                        "23505" => DbError::QueryExecution {
                            message: format!("unique constraint violation: {}", db_err.message()),
                            source: Some(Box::new(db_err)),
                        },
                        "23503" => DbError::QueryExecution {
                            message: format!("foreign key violation: {}", db_err.message()),
                            source: Some(Box::new(db_err)),
                        },
                        "23502" => DbError::QueryExecution {
                            message: format!("not null violation: {}", db_err.message()),
                            source: Some(Box::new(db_err)),
                        },
                        "23514" => DbError::QueryExecution {
                            message: format!("check constraint violation: {}", db_err.message()),
                            source: Some(Box::new(db_err)),
                        },
                        _ => DbError::QueryExecution {
                            message: db_err.message().to_string(),
                            source: Some(Box::new(db_err)),
                        },
                    }
                } else {
                    DbError::QueryExecution {
                        message: db_err.message().to_string(),
                        source: Some(Box::new(db_err)),
                    }
                }
            }

            // IO errors usually mean connection issues
            Error::Io(io_err) => DbError::Connection {
                message: format!("IO error: {}", io_err),
                source: Some(Box::new(io_err)),
            },

            // TLS errors
            Error::Tls(tls_err) => DbError::Connection {
                message: format!("TLS error: {}", tls_err),
                source: Some(tls_err),
            },

            // Protocol errors
            Error::Protocol(msg) => DbError::Connection {
                message: format!("protocol error: {}", msg),
                source: None,
            },

            // Column decode errors
            Error::ColumnDecode { index, source } => DbError::Serialization {
                message: format!("failed to decode column {}: {}", index, source),
                source: Some(source),
            },

            // Type not found
            Error::TypeNotFound { type_name } => DbError::Configuration {
                message: format!("type '{}' not found in database", type_name),
            },

            // Column not found
            Error::ColumnNotFound(column) => DbError::QueryExecution {
                message: format!("column '{}' not found", column),
                source: None,
            },

            // Column index out of bounds
            Error::ColumnIndexOutOfBounds { index, len } => DbError::QueryExecution {
                message: format!(
                    "column index {} out of bounds (row has {} columns)",
                    index, len
                ),
                source: None,
            },

            // Decode errors
            Error::Decode(decode_err) => DbError::Serialization {
                message: format!("decode error: {}", decode_err),
                source: Some(decode_err),
            },

            // Migration errors (if using)
            Error::Migrate(migrate_err) => DbError::Configuration {
                message: format!("migration error: {}", migrate_err),
            },

            // Any other errors
            _ => DbError::Internal {
                source: Box::new(err),
            },
        }
    }
}

// ============================================================================
// REDIS ERROR CONVERSION
// ============================================================================

#[cfg(feature = "redis")]
impl From<redis::RedisError> for DbError {
    fn from(err: redis::RedisError) -> Self {
        use redis::ErrorKind;

        match err.kind() {
            // Connection errors
            ErrorKind::IoError | ErrorKind::ClientError => DbError::Connection {
                message: err.to_string(),
                source: Some(Box::new(err)),
            },

            // Authentication errors
            ErrorKind::AuthenticationFailed => DbError::Connection {
                message: "authentication failed".to_string(),
                source: Some(Box::new(err)),
            },

            // Type errors (wrong data type for operation)
            ErrorKind::TypeError => DbError::QueryExecution {
                message: format!("type error: {}", err),
                source: Some(Box::new(err)),
            },

            // Command execution errors
            ErrorKind::ExecAbortError => DbError::Transaction {
                message: "transaction aborted".to_string(),
                source: Some(Box::new(err)),
            },

            // Busy/Loading errors
            ErrorKind::BusyLoadingError => DbError::QueryExecution {
                message: "server is busy loading dataset".to_string(),
                source: Some(Box::new(err)),
            },

            // Read-only errors
            ErrorKind::ReadOnly => DbError::QueryExecution {
                message: "server is in read-only mode".to_string(),
                source: Some(Box::new(err)),
            },

            // Cluster errors
            ErrorKind::ClusterDown | ErrorKind::CrossSlot | ErrorKind::TryAgain => {
                DbError::Connection {
                    message: format!("cluster error: {}", err),
                    source: Some(Box::new(err)),
                }
            }

            // Master down error
            ErrorKind::MasterDown => DbError::Connection {
                message: "master is down".to_string(),
                source: Some(Box::new(err)),
            },

            // Extension errors or unknown
            _ => DbError::Internal {
                source: Box::new(err),
            },
        }
    }
}

// ============================================================================
// BINCODE ERROR CONVERSION (for Redis serialization)
// ============================================================================

#[cfg(all(feature = "redis"))]
impl From<bincode::Error> for DbError {
    fn from(err: bincode::Error) -> Self {
        DbError::Serialization {
            message: format!("bincode error: {}", err),
            source: Some(Box::new(err)),
        }
    }
}

// ============================================================================
// BB8 POOL ERROR CONVERSION
// ============================================================================

#[cfg(feature = "redis")]
impl<E> From<bb8::RunError<E>> for DbError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: bb8::RunError<E>) -> Self {
        use bb8::RunError;

        match err {
            RunError::User(user_err) => DbError::Internal {
                source: Box::new(user_err),
            },
            RunError::TimedOut => DbError::Timeout { duration_ms: 30000 },
        }
    }
}

// ============================================================================
// HELPER IMPLEMENTATIONS
// ============================================================================

impl DbError {
    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            DbError::Timeout { .. }
                | DbError::Connection { .. }
                | DbError::Pool { .. }
                | DbError::RateLimited { .. }
        )
    }

    /// Check if this is a not found error (useful for 404 responses)
    pub fn is_not_found(&self) -> bool {
        matches!(self, DbError::NotFound { .. })
    }

    /// Check if this is a client error (4xx class)
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            DbError::NotFound { .. }
                | DbError::QueryExecution { .. }
                | DbError::Serialization { .. }
                | DbError::FeatureNotEnabled { .. }
        )
    }

    /// Check if this is a server error (5xx class)
    pub fn is_server_error(&self) -> bool {
        !self.is_client_error()
    }

    /// Get suggested HTTP status code for this error
    pub fn http_status_code(&self) -> u16 {
        match self {
            DbError::NotFound { .. } => 404,
            DbError::RateLimited { .. } => 429,
            DbError::Timeout { .. } => 408,
            DbError::Configuration { .. } | DbError::FeatureNotEnabled { .. } => 500,
            DbError::QueryExecution { .. } | DbError::Serialization { .. } => 400,
            DbError::Connection { .. } | DbError::Pool { .. } => 503,
            DbError::Transaction { .. } => 409,
            DbError::Internal { .. } => 500,
        }
    }

    /// Get retry after duration in milliseconds
    pub fn retry_after_ms(&self) -> Option<u64> {
        match self {
            DbError::RateLimited { retry_after_ms, .. } => *retry_after_ms,
            DbError::Timeout { .. } => Some(1000), // Retry after 1 second
            DbError::Pool { .. } => Some(100),     // Retry quickly for pool issues
            _ => None,
        }
    }
}
