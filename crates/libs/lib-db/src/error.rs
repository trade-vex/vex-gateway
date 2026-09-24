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

    /// Operation not supported by this store
    #[error("operation not supported: {operation}")]
    Unsupported { operation: String },
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
            // Connection/IO errors
            ErrorKind::Io | ErrorKind::Client => DbError::Connection {
                message: err.to_string(),
                source: Some(Box::new(err)),
            },

            // Authentication errors
            ErrorKind::AuthenticationFailed => DbError::Connection {
                message: "authentication failed".to_string(),
                source: Some(Box::new(err)),
            },

            // Type errors (wrong data type for operation)
            ErrorKind::UnexpectedReturnType => DbError::QueryExecution {
                message: format!("type error: {}", err),
                source: Some(Box::new(err)),
            },

            // Parse errors
            ErrorKind::Parse => DbError::Serialization {
                message: format!("parse error: {}", err),
                source: Some(Box::new(err)),
            },

            // Invalid client config
            ErrorKind::InvalidClientConfig => DbError::Configuration {
                message: err.to_string(),
            },

            // Server errors (covers BusyLoading, TryAgain, ClusterDown, etc.)
            ErrorKind::Server(_) => DbError::Connection {
                message: format!("server error: {}", err),
                source: Some(Box::new(err)),
            },

            // Cluster connection errors
            ErrorKind::ClusterConnectionNotFound => DbError::Connection {
                message: "cluster connection not found".to_string(),
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
                | DbError::RateLimited { .. }
        )
    }

    /// Check if this is a server error (5xx class)
    pub fn is_server_error(&self) -> bool {
        !self.is_client_error()
    }

    /// Get suggested HTTP status code for this error
    pub fn http_status_code(&self) -> u16 {
        match self {
            DbError::NotFound { .. } => 404,    // Not Found
            DbError::RateLimited { .. } => 429, // Too Many Requests
            DbError::Timeout { .. } => 408,     // Request Timeout
            DbError::Configuration { .. } | DbError::FeatureNotEnabled { .. } => 500, // Internal server error
            DbError::QueryExecution { .. } | DbError::Serialization { .. } => 400,    // Bad Request
            DbError::Connection { .. } | DbError::Pool { .. } => 503, // Service Unavailable
            DbError::Transaction { .. } => 409,                       // Conflict
            DbError::Internal { .. } => 500,                          // Internal server error
            DbError::Unsupported { .. } => 501,                       // Not Implemented
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
