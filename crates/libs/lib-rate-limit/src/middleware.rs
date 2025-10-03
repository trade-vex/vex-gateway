use crate::config::RateLimitConfig;
use crate::error::RateLimitError;
use crate::rate_limiter::{RateLimiter, FixedWindowRateLimiter};
use crate::redis_store::RedisRateLimitStore;
use actix_web::{
    body::{BoxBody, MessageBody},
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpMessage, HttpResponse,
};
use futures_util::future::LocalBoxFuture;
use lib_utils::auth_extractors::AuthenticatedUserId;
use std::{
    future::{ready, Ready},
    rc::Rc,
    sync::Arc,
};
use tracing::{debug, warn};

#[derive(Clone)]
pub struct RateLimitMiddleware {
    rate_limiter: Arc<dyn RateLimiter>,
    config: RateLimitConfig,
}

impl RateLimitMiddleware {
    pub fn new(rate_limiter: Arc<dyn RateLimiter>, config: RateLimitConfig) -> Self {
        Self {
            rate_limiter,
            config,
        }
    }

    pub async fn with_redis(
        redis_url: &str,
        config: RateLimitConfig,
    ) -> Result<Self, RateLimitError> {
        let store = RedisRateLimitStore::new(redis_url, config.key_prefix.clone()).await?;
        let rate_limiter = Arc::new(FixedWindowRateLimiter::new(store, config.clone()));
        
        Ok(Self::new(rate_limiter, config))
    }

    pub fn with_rate_limiter(rate_limiter: Arc<dyn RateLimiter>, config: RateLimitConfig) -> Self {
        Self::new(rate_limiter, config)
    }
}

impl<S, B> Transform<S, ServiceRequest> for RateLimitMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Transform = RateLimitService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RateLimitService {
            service: Rc::new(service),
            rate_limiter: self.rate_limiter.clone(),
            config: self.config.clone(),
        }))
    }
}

pub struct RateLimitService<S> {
    service: Rc<S>,
    rate_limiter: Arc<dyn RateLimiter>,
    config: RateLimitConfig,
}

impl<S, B> Service<ServiceRequest> for RateLimitService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let rate_limiter = self.rate_limiter.clone();
        let config = self.config.clone();

        Box::pin(async move {
            let user_id = match req.extensions().get::<AuthenticatedUserId>() {
                Some(user) => Some(user.0.to_string()),
                None => None,
            };

            match user_id {
                Some(user_id) => {
                    debug!("Checking rate limit for user: {}", user_id);

                    match rate_limiter.is_allowed(&user_id).await {
                        Ok(true) => {
                            debug!("Rate limit check passed for user: {}", user_id);
                            let resp = service.call(req).await?;
                            Ok(resp.map_into_boxed_body())
                        }
                        Ok(false) => {
                            warn!("Rate limit exceeded for user: {}", user_id);
                            
                            let remaining = rate_limiter.get_remaining_requests(&user_id).await.unwrap_or(0);
                            let reset_time = rate_limiter.get_window_reset_time(&user_id).await.unwrap_or(0);
                            
                            let error_message = config.error_message
                                .clone()
                                .unwrap_or_else(|| "Rate limit exceeded".to_string());

                            let response = HttpResponse::TooManyRequests()
                                .insert_header(("X-RateLimit-Limit", config.max_requests.to_string()))
                                .insert_header(("X-RateLimit-Remaining", remaining.to_string()))
                                .insert_header(("X-RateLimit-Reset", reset_time.to_string()))
                                .json(serde_json::json!({
                                    "error": "Rate limit exceeded",
                                    "message": error_message,
                                    "user_id": user_id,
                                    "limit": config.max_requests,
                                    "remaining": remaining,
                                    "reset_in_seconds": reset_time
                                }));

                            let (req, _) = req.into_parts();
                            Ok(ServiceResponse::new(req, response))
                        }
                        Err(e) => {
                            warn!("Rate limiting error for user {}: {}", user_id, e);
                            let resp = service.call(req).await?;
                            Ok(resp.map_into_boxed_body())
                        }
                    }
                }
                None => {
                    warn!("No authenticated user found in request - skipping rate limiting");
                    let resp = service.call(req).await?;
                    Ok(resp.map_into_boxed_body())
                }
            }
        })
    }
}