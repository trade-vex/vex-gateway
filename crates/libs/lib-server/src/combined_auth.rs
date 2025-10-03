use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpRequest,
};
use futures_util::future::{ready, LocalBoxFuture, Ready};
use std::{rc::Rc, sync::Arc};
use tracing::error;
use lib_middleware::api_key::{middleware::ApiKeyMiddleware};
use lib_auth::middleware::jwt_auth::JwtAuthentication;

#[derive(Clone)]
pub struct CombinedAuthMiddleware {
    jwt_middleware: JwtAuthentication,
    api_key_middleware: Arc<ApiKeyMiddleware>,
}

impl CombinedAuthMiddleware {
    pub fn new(
        jwt_middleware: JwtAuthentication,
        api_key_middleware: ApiKeyMiddleware,
    ) -> Self {
        Self {
            jwt_middleware,
            api_key_middleware: Arc::new(api_key_middleware),
        }
    }

    fn detect_auth_type(req: &HttpRequest) -> AuthType {
        if let Some(auth_header) = req.headers().get("authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                if auth_str.starts_with("Bearer ") {
                    return AuthType::Jwt;
                }
            }
        }
        
        if req.headers().get("X-MBX-APIKEY").is_some() {
            return AuthType::ApiKey;
        }
        
        AuthType::None
    }
}

#[derive(Debug, Clone, PartialEq)]
enum AuthType {
    Jwt,
    ApiKey,
    None,
}

impl<S, B> Transform<S, ServiceRequest> for CombinedAuthMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static + actix_web::body::MessageBody,
{
    type Response = ServiceResponse<actix_web::body::BoxBody>;
    type Error = Error;
    type Transform = CombinedAuthMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(CombinedAuthMiddlewareService {
            service: Rc::new(service),
            jwt_middleware: self.jwt_middleware.clone(),
            api_key_middleware: Arc::clone(&self.api_key_middleware),
        }))
    }
}

pub struct CombinedAuthMiddlewareService<S> {
    service: Rc<S>,
    jwt_middleware: JwtAuthentication,
    api_key_middleware: Arc<ApiKeyMiddleware>,
}

impl<S, B> Service<ServiceRequest> for CombinedAuthMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static + actix_web::body::MessageBody,
{
    type Response = ServiceResponse<actix_web::body::BoxBody>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let jwt_middleware = self.jwt_middleware.clone();
        let api_key_middleware = Arc::clone(&self.api_key_middleware);
        let service = Rc::clone(&self.service);

        Box::pin(async move {
            let auth_type = CombinedAuthMiddleware::detect_auth_type(req.request());

            match auth_type {
                AuthType::Jwt => {
                    // Use the JWT middleware's transform to handle JWT validation
                    let jwt_service = jwt_middleware.new_transform(service.clone()).await.unwrap();
                    return jwt_service.call(req).await;
                }

                AuthType::ApiKey => {
                    let api_key_service = api_key_middleware.new_transform(service.clone()).await.unwrap();
                    let response = api_key_service.call(req).await?;
                    return Ok(response.map_into_boxed_body());
                }

                AuthType::None => {
                    error!("No authentication provided");
                    return Err(actix_web::error::ErrorUnauthorized("Authentication required"));
                }
            }
        })
    }
}
