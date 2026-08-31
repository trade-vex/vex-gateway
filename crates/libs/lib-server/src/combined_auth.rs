use actix_web::{
    Error, HttpRequest,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use futures_util::future::LocalBoxFuture;
use lib_auth::middleware::jwt_auth::{JwtAuthentication, JwtAuthenticationMiddleware};
use lib_auth::service::session_service::SessionService;
use lib_middleware::api_key::middleware::{ApiKeyMiddleware, ApiKeyMiddlewareService};
use std::{rc::Rc, sync::Arc};
use tracing::error;

#[derive(Clone)]
pub struct CombinedAuthMiddleware {
    jwt_middleware: JwtAuthentication,
    api_key_middleware: ApiKeyMiddleware,
    session_service: Arc<SessionService>,
}

impl CombinedAuthMiddleware {
    pub fn new(
        jwt_middleware: JwtAuthentication,
        api_key_middleware: ApiKeyMiddleware,
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
    type Future = LocalBoxFuture<'static, Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        let jwt_middleware = self.jwt_middleware.clone();
        let api_key_middleware = self.api_key_middleware.clone();
        let session_service = Arc::clone(&self.session_service);

        Box::pin(async move {
            let service = Rc::new(service);
            let jwt_service = jwt_middleware.new_transform(Rc::clone(&service)).await?;
            let api_key_service = api_key_middleware
                .new_transform(Rc::clone(&service))
                .await?;

            Ok(CombinedAuthMiddlewareService {
                service,
                jwt_service: Rc::new(jwt_service),
                api_key_service: Rc::new(api_key_service),
                session_service,
            })
        })
    }
}

pub struct CombinedAuthMiddlewareService<S> {
    service: Rc<S>,
    jwt_service: Rc<JwtAuthenticationMiddleware<Rc<S>>>,
    api_key_service: Rc<ApiKeyMiddlewareService<Rc<S>>>,
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
        let jwt_service = Rc::clone(&self.jwt_service);
        let api_key_service = Rc::clone(&self.api_key_service);
        let session_service = Arc::clone(&self.session_service);

        Box::pin(async move {
            let auth_type = CombinedAuthMiddleware::detect_auth_type(req.request());

            match auth_type {
                AuthType::Jwt => {
                    return jwt_service.call(req).await;
                }

                AuthType::ApiKey => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, HttpResponse, test, web};
    use jsonwebtoken::DecodingKey;
    use lib_middleware::api_key::security::{ApiKeyConfig, SecurityType};
    use redis::aio::ConnectionManager;
    use std::time::Duration;
    use tokio::sync::Barrier;

    #[actix_web::test]
    async fn api_key_requests_reach_downstream_concurrently() {
        let redis_client = redis::Client::open("redis://127.0.0.1:6379/").unwrap();
        let redis_conn = ConnectionManager::new(redis_client).await.unwrap();
        let jwt_middleware = JwtAuthentication::new(
            DecodingKey::from_secret(b"unused-test-secret"),
            "vex-gateway".to_string(),
            "api".to_string(),
            redis_conn.clone(),
        );
        let api_key_middleware = ApiKeyMiddleware::new()
            .with_api_key(
                ApiKeyConfig::new("concurrent-test-key".to_string())
                    .with_permissions(vec![SecurityType::UserStream]),
            )
            .with_default_security(SecurityType::UserStream);
        let session_service = Arc::new(SessionService::new(redis_conn, 30 * 60));
        let combined_auth =
            CombinedAuthMiddleware::new(jwt_middleware, api_key_middleware, session_service);
        let barrier = web::Data::new(Barrier::new(2));

        let app = test::init_service(
            App::new()
                .app_data(barrier.clone())
                .wrap(combined_auth)
                .route(
                    "/test",
                    web::get().to(|barrier: web::Data<Barrier>| async move {
                        barrier.wait().await;
                        HttpResponse::Ok().finish()
                    }),
                ),
        )
        .await;

        let first_request = test::TestRequest::get()
            .uri("/test")
            .insert_header(("X-MBX-APIKEY", "concurrent-test-key"))
            .to_request();
        let second_request = test::TestRequest::get()
            .uri("/test")
            .insert_header(("X-MBX-APIKEY", "concurrent-test-key"))
            .to_request();

        let (first_response, second_response) = tokio::time::timeout(
            Duration::from_secs(2),
            futures_util::future::join(
                test::call_service(&app, first_request),
                test::call_service(&app, second_request),
            ),
        )
        .await
        .expect("both requests should enter the downstream handler before either completes");

        assert!(first_response.status().is_success());
        assert!(second_response.status().is_success());
    }
}
