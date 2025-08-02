use actix_web::{
    Error, HttpMessage, HttpResponse,
    body::{BoxBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use futures_util::future::LocalBoxFuture;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use std::future::{Ready, ready};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthenticatedUser {
    pub user_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
struct JwtClaims {
    sub: String, // user_id
    exp: usize,
    iat: usize,
    iss: String,
    aud: String,
    jti: String,
}
#[derive(Clone)]
pub struct JwtAuthentication {
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
}

impl JwtAuthentication {
    pub fn new(decoding_key: DecodingKey, issuer: String, audience: String) -> Self {
        Self {
            decoding_key,
            issuer,
            audience,
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for JwtAuthentication
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type InitError = ();
    type Transform = JwtAuthenticationMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(JwtAuthenticationMiddleware {
            service,
            decoding_key: self.decoding_key.clone(),
            issuer: self.issuer.clone(),
            audience: self.audience.clone(),
        }))
    }
}

pub struct JwtAuthenticationMiddleware<S> {
    service: S,
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
}

impl<S, B> Service<ServiceRequest> for JwtAuthenticationMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let decoding_key = self.decoding_key.clone();
        let issuer = self.issuer.clone();
        let audience = self.audience.clone();

        // Extract Authorization header
        let auth_header = req.headers().get("authorization");

        if let Some(auth_value) = auth_header {
            if let Ok(auth_str) = auth_value.to_str() {
                if let Some(token) = auth_str.strip_prefix("Bearer ") {
                    // Validate JWT token
                    let mut validation = Validation::new(Algorithm::RS256);
                    validation.set_issuer(&[issuer]);
                    validation.set_audience(&[audience]);

                    match decode::<JwtClaims>(token, &decoding_key, &validation) {
                        Ok(token_data) => {
                            // Parse user_id from subject
                            if let Ok(user_id) = Uuid::parse_str(&token_data.claims.sub) {
                                let authenticated_user = AuthenticatedUser { user_id };
                                req.extensions_mut().insert(authenticated_user);

                                let fut = self.service.call(req);
                                return Box::pin(async move {
                                    let response = fut.await?;
                                    Ok(response.map_into_boxed_body())
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("JWT validation failed: {}", e);
                        }
                    }
                }
            }
        }

        // Authentication failed
        Box::pin(async move {
            let (req, _) = req.into_parts();
            let response = HttpResponse::Unauthorized().json(serde_json::json!({
                "error": {
                    "code": "unauthorized",
                    "message": "Invalid or missing authentication token",
                    "status": 401
                }
            }));

            Ok(ServiceResponse::new(req, response).map_into_boxed_body())
        })
    }
}

// Add this extractor for use in handlers
impl actix_web::FromRequest for AuthenticatedUser {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &actix_web::HttpRequest, _: &mut actix_web::dev::Payload) -> Self::Future {
        if let Some(user) = req.extensions().get::<AuthenticatedUser>() {
            ready(Ok(user.clone()))
        } else {
            ready(Err(actix_web::error::ErrorUnauthorized(
                "Not authenticated",
            )))
        }
    }
}
