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
//! use vex_gateway_lib_db::{Result, DbError};
//! 
//! #[cfg(feature = "postgres")]
//! use vex_gateway_lib_db::postgres;
//! 
//! #[cfg(feature = "redis")]
//! use vex_gateway_lib_db::{redis, memory_kv};
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

/// PostgreSQL database implementation
#[cfg(feature = "postgres")]
pub mod postgres;

/// Redis database implementation for caching and key-value operations
#[cfg(feature = "redis")]
pub mod redis;

/// In-memory key-value store implementation
#[cfg(feature = "redis")]
pub mod memory_kv;

/// Re-export common error types and result aliases
pub use error::{DbError, Result};