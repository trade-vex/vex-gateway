use actix_web::{
    Error, HttpRequest,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use futures_util::future::{LocalBoxFuture, Ready, ready};
use lib_auth::middleware::jwt_auth::JwtAuthentication;
use lib_auth::service::session_service::SessionService;
use lib_middleware::api_key::middleware::ApiKeyMiddleware;
use std::{rc::Rc, sync::Arc};
use tokio::sync::Mutex as TokioMutex;
use tracing::error;

#[derive(Clone)]
pub struct CombinedAuthMiddleware {
    jwt_middleware: JwtAuthentication,
    api_key_middleware: Arc<TokioMutex<ApiKeyMiddleware>>,
    session_service: Arc<SessionService>,
}

impl CombinedAuthMiddleware {
    pub fn new(
        jwt_middleware: JwtAuthentication,
        api_key_middleware: Arc<TokioMutex<ApiKeyMiddleware>>,
        session_service: Arc<SessionService>,
    ) -> Self {
        Self {
            jwt_middleware,
            api_key_middleware,
            session_service,
        }
    }

    fn detect_auth_type(req: &HttpRequest) -> AuthType {
        if let Some(auth_header) = req.headers().get("authorization")
            && let Ok(auth_str) = auth_header.to_str()
            && auth_str.starts_with("Bearer ")
        {
            return AuthType::Jwt;
        }

        if req.headers().get("X-MBX-APIKEY").is_some() {
            return AuthType::ApiKey;
        }

        // Check for session cookie
        if req.cookie("session_id").is_some() {
            return AuthType::Session;
        }

        AuthType::None
    }
}

#[derive(Debug, Clone, PartialEq)]
enum AuthType {
    Jwt,
    ApiKey,
    Session,
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
            session_service: Arc::clone(&self.session_service),
        }))
    }
}

pub struct CombinedAuthMiddlewareService<S> {
    service: Rc<S>,
    jwt_middleware: JwtAuthentication,
    api_key_middleware: Arc<TokioMutex<ApiKeyMiddleware>>,
    session_service: Arc<SessionService>,
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

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let jwt_middleware = self.jwt_middleware.clone();
        let api_key_middleware = Arc::clone(&self.api_key_middleware);
        let session_service = Arc::clone(&self.session_service);
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
                    let api_key_middleware = api_key_middleware.lock().await;
                    let api_key_service = api_key_middleware
                        .new_transform(service.clone())
                        .await
                        .unwrap();

                    let response = api_key_service.call(req).await?;
                    Ok(response.map_into_boxed_body())
                }

                AuthType::Session => {
                    // Handle session-based authentication
                    if let Some(session_id_cookie) = req.request().cookie("session_id") {
                        let session_id = session_id_cookie.value().to_string();
                        // Never log the session id: it is a bearer credential, and a log
                        // reader could replay it as a cookie to authenticate as this user.
                        tracing::debug!("Session auth: found session_id cookie");

                        // Get session from Redis (contains access_token and refresh_token)
                        match session_service.get_session(&session_id).await {
                            Ok(session_data) => {
                                tracing::debug!(
                                    "Session auth: Session found in Redis, extracting access_token"
                                );
                                // Extract access_token from session
                                let access_token = session_data.access_token.clone();

                                tracing::debug!(
                                    "Session auth: Adding JWT token to Authorization header"
                                );
                                // Add Authorization header to the request with the JWT from session
                                req.headers_mut().insert(
                                    actix_web::http::header::AUTHORIZATION,
                                    actix_web::http::header::HeaderValue::from_str(&format!(
                                        "Bearer {}",
                                        access_token
                                    ))
                                    .map_err(|_| {
                                        actix_web::error::ErrorUnauthorized("Invalid token format")
                                    })?,
                                );

                                tracing::debug!(
                                    "Session auth: Passing request to JWT middleware for validation"
                                );
                                // Now pass to JWT middleware for proper validation
                                let jwt_service =
                                    jwt_middleware.new_transform(service.clone()).await.unwrap();
                                jwt_service.call(req).await
                            }
                            Err(e) => {
                                tracing::warn!("Session auth: Invalid session: {}", e);
                                Err(actix_web::error::ErrorUnauthorized(
                                    "Invalid or expired session",
                                ))
                            }
                        }
                    } else {
                        tracing::debug!("Session auth: No session_id cookie found");
                        Err(actix_web::error::ErrorUnauthorized(
                            "Session cookie not found",
                        ))
                    }
                }

                AuthType::None => {
                    error!("No authentication provided");
                    Err(actix_web::error::ErrorUnauthorized(
                        "Authentication required",
                    ))
                }
            }
        })
    }
}
