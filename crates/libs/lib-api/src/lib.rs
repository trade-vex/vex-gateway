//! lib-api/src/lib.rs

pub mod handlers;
pub mod routes;

// Re-export handlers for easy access
pub use handlers::{trading, user, health_check};

// Re-export route configurations
pub use routes::{trading_routes, user_routes, health_check_route};
