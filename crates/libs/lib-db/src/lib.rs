//! # Database Library
//! 
//! This crate provides a unified database abstraction layer.
//! It supports multiple database backends through feature flags and provides a consistent
//! interface for database operations.
//! 
//! ## Features
//! 
//! - **postgres**: Enables PostgreSQL support
//! - **redis**: Enables Redis support for caching and key-value operations
//! 
//! ## Usage
//! 
//! ```rust
//! use lib_db::{Result, DbError};
//! 
//! #[cfg(feature = "postgres")]
//! use lib_db::backends::sql::postgres;
//! 
//! #[cfg(feature = "redis")]
//! use lib_db::backends::kv::{redis, memory_kv};
//! ```
//! 
//! ## Architecture
//! 
//! This library follows the Escape Hatch pattern for database interactions, providing
//! both high-level abstractions and low-level access when needed.

/// Core error types and result aliases for database operations
pub mod error;

/// Database trait definitions and abstractions
pub mod traits;

/// Database backend implementations
pub mod backends;

/// Re-export common error types and result aliases
pub use error::{DbError, Result};

// Re-export backend modules for convenience
#[cfg(feature = "postgres")]
pub use backends::sql::postgres;

#[cfg(feature = "redis")]
pub use backends::kv::{redis, memory_kv};