use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpMessage, HttpRequest,
};
use futures_util::future::{ready, LocalBoxFuture, Ready};
use std::{rc::Rc};
use tracing::{error, info};
use lib_middleware::api_key::{middleware::ApiKeyMiddleware};
use lib_auth::middleware::jwt_auth::{JwtAuthentication, AuthenticatedUser, JwtClaims};
use jsonwebtoken;
use uuid;

#[derive(Clone)]
pub struct CombinedAuthMiddleware {
    jwt_middleware: JwtAuthentication,
    api_key_middleware: ApiKeyMiddleware,
}

impl CombinedAuthMiddleware {
    pub fn new(
        jwt_middleware: JwtAuthentication,
        api_key_middleware: ApiKeyMiddleware,
    ) -> Self {
        Self {
            jwt_middleware,
            api_key_middleware,
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
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = CombinedAuthMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(CombinedAuthMiddlewareService {
            service: Rc::new(service),
            jwt_middleware: self.jwt_middleware.clone(),
            api_key_middleware: self.api_key_middleware.clone(),
        }))
    }
}

pub struct CombinedAuthMiddlewareService<S> {
    service: Rc<S>,
    jwt_middleware: JwtAuthentication,
    api_key_middleware: ApiKeyMiddleware,
}

impl<S, B> Service<ServiceRequest> for CombinedAuthMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let jwt_middleware = self.jwt_middleware.clone();
        let api_key_middleware = self.api_key_middleware.clone();
        let service = Rc::clone(&self.service);

        Box::pin(async move {
            let auth_type = CombinedAuthMiddleware::detect_auth_type(req.request());

            match auth_type {
                AuthType::Jwt => {                    
                    let auth_header = req.headers().get("authorization");
                    
                    if let Some(auth_value) = auth_header {
                        if let Ok(auth_str) = auth_value.to_str() {
                            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                                let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
                                validation.set_issuer(&[&jwt_middleware.issuer]);
                                validation.set_audience(&[&jwt_middleware.audience]);

                                match jsonwebtoken::decode::<JwtClaims>(token, &jwt_middleware.decoding_key, &validation) {
                                    Ok(token_data) => {
                                        if let Ok(user_id) = uuid::Uuid::parse_str(&token_data.claims.sub) {
                                            let authenticated_user = AuthenticatedUser { user_id };
                                            req.extensions_mut().insert(authenticated_user);

                                            info!("JWT authentication successful for user: {}", user_id);
                                            return service.call(req).await;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("JWT validation failed: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    
                    error!("JWT authentication failed");
                    return Err(actix_web::error::ErrorUnauthorized("Invalid or missing JWT token"));
                }

                AuthType::ApiKey => {
                    let api_key_service = api_key_middleware.new_transform(service.clone()).await.unwrap();
                    
                    return api_key_service.call(req).await;
                }

                AuthType::None => {
                    error!("No authentication provided");
                    return Err(actix_web::error::ErrorUnauthorized("Authentication required"));
                }
            }
        })
    }
}
