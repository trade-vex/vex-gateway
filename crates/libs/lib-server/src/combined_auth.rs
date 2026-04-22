use actix_web::{
    Error, HttpMessage, HttpRequest,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use futures_util::future::{LocalBoxFuture, Ready, ready};
use lib_auth::middleware::jwt_auth::{AuthenticatedUser, JwtAuthentication};
use lib_auth::service::session_service::SessionService;
use lib_auth::utils::cookie_policy::CookiePolicy;
use lib_middleware::api_key::middleware::ApiKeyMiddleware;
use std::{rc::Rc, sync::Arc};
use tokio::sync::Mutex as TokioMutex;
use tracing::error;

const SESSION_REFRESH_GRACE_SECONDS: i64 = 60;

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

    fn session_refresh_due(req: &HttpRequest, session_ttl_seconds: u64) -> bool {
        let refresh_window = ((session_ttl_seconds as i64) / 4).max(SESSION_REFRESH_GRACE_SECONDS);
        let now = chrono::Utc::now().timestamp();

        req.cookie("session_expires_at")
            .and_then(|cookie| cookie.value().parse::<i64>().ok())
            .map(|expires_at| expires_at - now <= refresh_window)
            .unwrap_or(true)
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

    fn call(&self, req: ServiceRequest) -> Self::Future {
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
                        tracing::debug!("Session auth: Found session_id cookie: {}", session_id);
                        let session_ttl_seconds = session_service.ttl_seconds();
                        let refresh_due = CombinedAuthMiddleware::session_refresh_due(
                            req.request(),
                            session_ttl_seconds,
                        );
                        let cookie_policy =
                            refresh_due.then(|| CookiePolicy::from_request(req.request()));

                        let session_result = if refresh_due {
                            session_service.get_session_and_refresh(&session_id).await
                        } else {
                            session_service.get_session(&session_id).await
                        };

                        match session_result {
                            Ok(session_data) => {
                                req.extensions_mut().insert(AuthenticatedUser {
                                    user_id: session_data.user_id,
                                });

                                let mut response = service.call(req).await?.map_into_boxed_body();

                                if let Some(cookie_policy) = cookie_policy {
                                    let session_expires_at = (chrono::Utc::now().timestamp()
                                        + session_ttl_seconds as i64)
                                        .to_string();
                                    let cookie = cookie_policy.build_cookie(
                                        "session_expires_at",
                                        session_expires_at,
                                        session_ttl_seconds as i64,
                                        false,
                                    );
                                    if let Err(e) = response.response_mut().add_cookie(&cookie) {
                                        tracing::warn!(
                                            "Session auth: Failed to update session_expires_at cookie: {}",
                                            e
                                        );
                                    }
                                }

                                Ok(response)
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
