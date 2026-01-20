//! Utility functions and extractors for the VEX Gateway.
//!
//! Provides common utilities used across the gateway crates:
//!
//! - **Auth Extractors**: Actix-web extractors for authenticated user context
//!   - `AuthenticatedUserId`: Extracts user ID from JWT claims
//!   - `AuthContext`: Full authentication context with user metadata
//! - **Validation Extractors**: Automatic request validation
//!   - `ValidatedJson`: JSON extractor with automatic validation

pub mod auth_extractors;
pub mod validated_json;

pub use auth_extractors::{
    extract_user_id_from_query, extract_user_id_from_request, AuthContext, AuthenticatedUserId,
};
pub use validated_json::{ValidatedJson, ValidationError};
