//! Session-based Authentication Middleware (Wrapper Layer)
//!
//! This middleware acts as the top layer that:
//! 1. Checks for session_id cookie in every request
//! 2. Retrieves session data from Redis
//! 3. Extracts access_token and refresh_token from session data
//! 4. Adds the tokens to Authorization header for underlying JWT/API Key auth middleware
//! 5. Passes control to CombinedAuthMiddleware

use actix_web::{
    Error,
    body::{BoxBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    http::header::{HeaderName, HeaderValue},
};
use deadpool_redis::Pool;
use deadpool_redis::redis::AsyncCommands;
use futures_util::future::LocalBoxFuture;
use serde_json;
use std::future::{Ready, ready};
use std::rc::Rc;
use std::sync::Arc;

use crate::dto::session_dto::SessionData;

#[derive(Clone)]
pub struct SessionAuthentication {
    redis_pool: Arc<Pool>,
}

impl SessionAuthentication {
    pub fn new(redis_pool: Arc<Pool>) -> Self {
        Self { redis_pool }
    }
}

impl<S, B> Transform<S, ServiceRequest> for SessionAuthentication
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type InitError = ();
    type Transform = SessionAuthenticationMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(SessionAuthenticationMiddleware {
            service: Rc::new(service),
            redis_pool: self.redis_pool.clone(),
        }))
    }
}

pub struct SessionAuthenticationMiddleware<S> {
    service: Rc<S>,
    redis_pool: Arc<Pool>,
}

impl<S, B> Service<ServiceRequest> for SessionAuthenticationMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let redis_pool = self.redis_pool.clone();

        Box::pin(async move {
            // Check for session cookie
            if let Some(cookie) = req.cookie("session_id") {
                let session_id = cookie.value();

                tracing::debug!("Session cookie found: {}", session_id);

                // Get session from Redis
                match redis_pool.get().await {
                    Ok(mut conn) => {
                        // Try to get session data from Redis
                        if let Ok(Some(json)) = conn.get::<&str, Option<String>>(session_id).await {
                            match serde_json::from_str::<SessionData>(&json) {
                                Ok(session_data) => {
                                    // Extract access_token from session
                                    let access_token = session_data.access_token.clone();
                                    
                                    tracing::debug!("Session data retrieved, adding access_token to Authorization header");
                                    
                                    // Add the access_token to Authorization header
                                    // This allows the underlying JWT middleware to validate it
                                    if let Ok(auth_value) = format!("Bearer {}", access_token).parse::<HeaderValue>() {
                                        req.headers_mut().insert(HeaderName::from_static("authorization"), auth_value);
                                        tracing::debug!("Added token to Authorization header");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to deserialize session data: {}", e);
                                }
                            }
                        } else {
                            tracing::debug!("No session found in Redis for: {}", session_id);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to get Redis connection: {}", e);
                        // Continue anyway - let underlying middleware handle auth
                    }
                }
            }

            // Proceed with the request (with or without session augmentation)
            // The underlying CombinedAuthMiddleware will handle JWT validation
            let res = service.call(req).await?;
            Ok(res.map_into_boxed_body())
        })
    }
}
