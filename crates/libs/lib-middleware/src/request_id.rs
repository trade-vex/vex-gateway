//! Request ID middleware for distributed tracing
//!
//! This middleware handles X-Request-ID header propagation:
//! - Extracts existing X-Request-ID from incoming requests
//! - Generates a new UUID if not present
//! - Adds the request ID to request extensions for handler access
//! - Adds X-Request-ID to response headers
//! - Integrates with tracing spans for log correlation

use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    http::header::{HeaderName, HeaderValue},
    Error, HttpMessage,
};
use futures_util::future::LocalBoxFuture;
use std::{
    future::{ready, Ready},
    rc::Rc,
};
use tracing::{info_span, Instrument};
use uuid::Uuid;

/// Header name for request ID
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Request ID stored in request extensions
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl RequestId {
    /// Create a new random request ID
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create a request ID from an existing string
    pub fn from_string(id: String) -> Self {
        Self(id)
    }

    /// Get the request ID as a string slice
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Middleware factory for request ID handling
pub struct RequestIdMiddleware;

impl RequestIdMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RequestIdMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, B> Transform<S, ServiceRequest> for RequestIdMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = RequestIdMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RequestIdMiddlewareService {
            service: Rc::new(service),
        }))
    }
}

pub struct RequestIdMiddlewareService<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for RequestIdMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);

        Box::pin(async move {
            // Extract or generate request ID
            let request_id = req
                .headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(|s| RequestId::from_string(s.to_string()))
                .unwrap_or_default();

            let request_id_str = request_id.0.clone();

            // Store in request extensions for handlers to access
            req.extensions_mut().insert(request_id);

            // Create a tracing span with the request ID
            let span = info_span!(
                "request",
                request_id = %request_id_str,
                method = %req.method(),
                path = %req.path(),
            );

            // Call the service within the span
            let fut = service.call(req);
            let res = fut.instrument(span).await?;

            // Add request ID to response headers
            let header_name = HeaderName::from_static(REQUEST_ID_HEADER);
            let header_value = HeaderValue::from_str(&request_id_str)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid"));

            // Get mutable access to response headers
            let (req, mut res) = res.into_parts();
            res.headers_mut().insert(header_name, header_value);

            Ok(ServiceResponse::new(req, res))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_generation() {
        let id1 = RequestId::new();
        let id2 = RequestId::new();
        // Should generate different UUIDs
        assert_ne!(id1.as_str(), id2.as_str());
        // Should be valid UUID format (36 chars with hyphens)
        assert_eq!(id1.as_str().len(), 36);
    }

    #[test]
    fn test_request_id_from_string() {
        let custom_id = "my-custom-request-id-123";
        let id = RequestId::from_string(custom_id.to_string());
        assert_eq!(id.as_str(), custom_id);
    }

    #[test]
    fn test_request_id_display() {
        let id = RequestId::from_string("test-id".to_string());
        assert_eq!(format!("{}", id), "test-id");
    }
}
