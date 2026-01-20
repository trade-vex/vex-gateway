//! Validated JSON extractor for automatic request validation.
//!
//! This module provides a `ValidatedJson<T>` extractor that combines JSON deserialization
//! with automatic validation using the `validator` crate. It eliminates boilerplate
//! validation code from handlers.
//!
//! # Example
//!
//! ```ignore
//! use lib_utils::ValidatedJson;
//! use validator::Validate;
//!
//! #[derive(Deserialize, Validate)]
//! struct CreateUserRequest {
//!     #[validate(email)]
//!     email: String,
//!     #[validate(length(min = 8))]
//!     password: String,
//! }
//!
//! async fn create_user(
//!     ValidatedJson(req): ValidatedJson<CreateUserRequest>,
//! ) -> HttpResponse {
//!     // req is already validated - no manual validation needed
//!     // ...
//! }
//! ```

use actix_web::{
    dev::Payload, http::StatusCode, web::Json, FromRequest, HttpRequest, HttpResponse,
    ResponseError,
};
use futures_util::future::LocalBoxFuture;
use serde::de::DeserializeOwned;
use std::fmt;
use std::ops::Deref;
use validator::Validate;

/// Error type for validation failures.
#[derive(Debug)]
pub struct ValidationError {
    message: String,
    details: Option<serde_json::Value>,
}

impl ValidationError {
    /// Create a new validation error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: None,
        }
    }

    /// Create a validation error with field-level details.
    pub fn with_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            message: message.into(),
            details: Some(details),
        }
    }

    /// Convert validator errors to structured JSON details.
    fn from_validator_errors(errors: validator::ValidationErrors) -> Self {
        let field_errors: serde_json::Map<String, serde_json::Value> = errors
            .field_errors()
            .iter()
            .map(|(field, errs)| {
                let messages: Vec<String> = errs
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| format!("validation failed: {:?}", e.code))
                    })
                    .collect();
                (field.to_string(), serde_json::json!(messages))
            })
            .collect();

        Self {
            message: "Request validation failed".to_string(),
            details: Some(serde_json::Value::Object(field_errors)),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl ResponseError for ValidationError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }

    fn error_response(&self) -> HttpResponse {
        let mut body = serde_json::json!({
            "code": "VALIDATION_ERROR",
            "message": self.message,
        });

        if let Some(details) = &self.details {
            body["details"] = details.clone();
        }

        HttpResponse::BadRequest().json(body)
    }
}

/// A validated JSON extractor that automatically validates request bodies.
///
/// This extractor wraps `Json<T>` and adds automatic validation using the `validator` crate.
/// The inner type `T` must implement both `DeserializeOwned` and `Validate`.
///
/// If deserialization fails, returns 400 with a JSON parsing error.
/// If validation fails, returns 400 with field-level validation errors.
#[derive(Debug)]
pub struct ValidatedJson<T>(pub T);

impl<T> ValidatedJson<T> {
    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for ValidatedJson<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> FromRequest for ValidatedJson<T>
where
    T: DeserializeOwned + Validate + 'static,
{
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let json_fut = Json::<T>::from_request(req, payload);

        Box::pin(async move {
            // First, deserialize the JSON
            let json = json_fut
                .await
                .map_err(|e| ValidationError::new(format!("Invalid JSON: {}", e)))?;

            // Then validate the deserialized data
            json.validate()
                .map_err(ValidationError::from_validator_errors)?;

            Ok(ValidatedJson(json.into_inner()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use validator::Validate;

    #[derive(Debug, Deserialize, Validate)]
    struct TestRequest {
        #[validate(email)]
        email: String,
        #[validate(length(min = 3, max = 50))]
        username: String,
    }

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::new("Test error");
        assert_eq!(err.to_string(), "Test error");
    }

    #[test]
    fn test_validation_error_status_code() {
        let err = ValidationError::new("Test error");
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validation_error_response_without_details() {
        let err = ValidationError::new("Test error");
        let response = err.error_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validation_error_response_with_details() {
        let details = serde_json::json!({
            "email": ["must be a valid email"]
        });
        let err = ValidationError::with_details("Validation failed", details);
        let response = err.error_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_from_validator_errors() {
        // Create a test struct with invalid data
        let req = TestRequest {
            email: "not-an-email".to_string(),
            username: "ab".to_string(), // too short
        };

        let result = req.validate();
        assert!(result.is_err());

        let validation_error = ValidationError::from_validator_errors(result.unwrap_err());
        assert_eq!(validation_error.message, "Request validation failed");
        assert!(validation_error.details.is_some());

        let details = validation_error.details.unwrap();
        assert!(details.get("email").is_some());
        assert!(details.get("username").is_some());
    }

    #[test]
    fn test_validated_json_deref() {
        let inner = TestRequest {
            email: "test@example.com".to_string(),
            username: "testuser".to_string(),
        };
        let validated = ValidatedJson(inner);

        // Test Deref
        assert_eq!(validated.email, "test@example.com");
        assert_eq!(validated.username, "testuser");
    }

    #[test]
    fn test_validated_json_into_inner() {
        let inner = TestRequest {
            email: "test@example.com".to_string(),
            username: "testuser".to_string(),
        };
        let validated = ValidatedJson(inner);

        let unwrapped = validated.into_inner();
        assert_eq!(unwrapped.email, "test@example.com");
    }
}
