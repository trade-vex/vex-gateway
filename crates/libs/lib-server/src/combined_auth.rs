use actix_web::{
    Error, HttpRequest,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    error::InternalError,
};
use futures_util::future::LocalBoxFuture;
use lib_auth::AuthError;
use lib_auth::middleware::jwt_auth::{
    JwtAuthentication, JwtAuthenticationMiddleware, service_unavailable,
};
use lib_auth::service::session_service::SessionService;
use lib_middleware::api_key::middleware::{ApiKeyMiddleware, ApiKeyMiddlewareService};
use lib_middleware::metrics::{self, AuthMethod};
use std::{rc::Rc, sync::Arc};

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
        let has_bearer_token = if let Some(auth_header) = req.headers().get("authorization")
            && let Ok(auth_str) = auth_header.to_str()
            // The scheme is case-insensitive (RFC 7235), as the JWT middleware reads it.
            && auth_str
                .get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer "))
        {
            true
        } else {
            false
        };
        let has_api_key = req.headers().get("X-MBX-APIKEY").is_some();
        let has_session = req.cookie("session_id").is_some();

        if [has_bearer_token, has_api_key, has_session]
            .into_iter()
            .filter(|present| *present)
            .count()
            > 1
        {
            return AuthType::Ambiguous;
        }

        match (has_bearer_token, has_api_key, has_session) {
            (true, false, false) => AuthType::Jwt,
            (false, true, false) => AuthType::ApiKey,
            (false, false, true) => AuthType::Session,
            _ => AuthType::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum AuthType {
    Jwt,
    ApiKey,
    Session,
    Ambiguous,
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
                    // The API-key check refuses with an error, never a response, and the routes
                    // behind it answer with responses. A 5xx is the gateway's own fault.
                    let response = api_key_service.call(req).await.inspect_err(|error| {
                        if error.as_response_error().status_code().is_client_error() {
                            metrics::auth_failures(AuthMethod::ApiKey).inc();
                        }
                    })?;
                    Ok(response.map_into_boxed_body())
                }

                AuthType::Session => {
                    // Handle session-based authentication
                    if let Some(session_id_cookie) = req.request().cookie("session_id") {
                        let session_id = session_id_cookie.value().to_string();
                        // Never log the session id: it is a bearer credential, and a log
                        // reader could replay it as a cookie to authenticate as this user.
                        tracing::debug!("Session auth: found session_id cookie");

                        // Get session from Redis (contains the access token)
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
                                        metrics::auth_failures(AuthMethod::Jwt).inc();
                                        actix_web::error::ErrorUnauthorized("Invalid token format")
                                    })?,
                                );

                                tracing::debug!(
                                    "Session auth: Passing request to JWT middleware for validation"
                                );
                                // Now pass to JWT middleware for proper validation
                                jwt_service.call(req).await
                            }
                            Err(AuthError::InvalidRequest) => {
                                tracing::warn!("Session auth: unknown or expired session");
                                metrics::auth_failures(AuthMethod::Jwt).inc();
                                Err(actix_web::error::ErrorUnauthorized(
                                    "Invalid or expired session",
                                ))
                            }
                            // A store error is not a logout: 503, as the JWT middleware answers.
                            Err(e) => {
                                tracing::error!("Session auth: session store unavailable: {}", e);
                                let message = "Authentication service temporarily unavailable";
                                Err(InternalError::from_response(
                                    message,
                                    service_unavailable(message),
                                )
                                .into())
                            }
                        }
                    } else {
                        tracing::debug!("Session auth: No session_id cookie found");
                        metrics::auth_failures(AuthMethod::Jwt).inc();
                        Err(actix_web::error::ErrorUnauthorized(
                            "Session cookie not found",
                        ))
                    }
                }

                AuthType::Ambiguous => Err(actix_web::error::ErrorBadRequest(
                    "Ambiguous authentication credentials",
                )),

                AuthType::None => {
                    tracing::debug!("No authentication provided");
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
    use std::time::Duration;
    use tokio::sync::Barrier;

    #[actix_web::test]
    async fn api_key_requests_reach_downstream_concurrently() {
        let redis_conn = crate::test_redis().await;
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

    use actix_web::http::StatusCode;
    use chrono::Utc;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use lib_auth::{
        config::JwtConfig, dto::jwt_response_dto::AppClaims,
        middleware::jwt_auth::AuthenticatedUser, service::TokenService,
    };
    use lib_middleware::SignatureVerifier;
    use lib_utils::AuthenticatedUserId;
    use redis::AsyncCommands;
    use redis::aio::ConnectionManager;
    use rsa::{
        Generate, PrivateKey,
        pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
    };
    use std::sync::OnceLock;
    use uuid::Uuid;

    /// One RSA key pair per test binary: generating a key in a debug build is slow.
    fn test_keys() -> (EncodingKey, DecodingKey) {
        static KEYS: OnceLock<(EncodingKey, DecodingKey)> = OnceLock::new();
        KEYS.get_or_init(|| {
            let private_key = PrivateKey::generate();
            let private_pem = private_key
                .to_pkcs8_pem(LineEnding::LF)
                .expect("test private key must encode");
            let public_pem = private_key
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .expect("test public key must encode");
            (
                EncodingKey::from_rsa_pem(private_pem.as_bytes())
                    .expect("test private key must be valid"),
                DecodingKey::from_rsa_pem(public_pem.as_bytes())
                    .expect("test public key must be valid"),
            )
        })
        .clone()
    }

    #[actix_web::test]
    async fn rejects_ambiguous_bearer_and_api_key_credentials() {
        let redis_conn = crate::test_redis().await;
        let session_service = Arc::new(SessionService::new(redis_conn.clone(), 300));
        let (_, decoding_key) = test_keys();
        let jwt_middleware = JwtAuthentication::new(
            decoding_key,
            "test-issuer".to_string(),
            "test-audience".to_string(),
            redis_conn,
        );
        let middleware =
            CombinedAuthMiddleware::new(jwt_middleware, ApiKeyMiddleware::new(), session_service);
        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let request = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", "Bearer jwt-token"))
            .insert_header(("X-MBX-APIKEY", "api-key"))
            .to_request();
        let error = test::try_call_service(&app, request)
            .await
            .expect_err("ambiguous credentials must be rejected");

        assert_eq!(
            error.as_response_error().status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(error.to_string(), "Ambiguous authentication credentials");
    }

    #[actix_web::test]
    async fn refused_credentials_are_counted_by_method() {
        let redis_conn = crate::test_redis().await;
        let jwt_middleware = JwtAuthentication::new(
            DecodingKey::from_secret(b"unused-test-secret"),
            "vex-gateway".to_string(),
            "api".to_string(),
            redis_conn.clone(),
        );
        // A route that needs a key, so that an unknown one is refused.
        let api_key_middleware =
            ApiKeyMiddleware::new().with_default_security(SecurityType::UserStream);
        let session_service = Arc::new(SessionService::new(redis_conn, 300));
        let app = test::init_service(
            App::new()
                .wrap(CombinedAuthMiddleware::new(
                    jwt_middleware,
                    api_key_middleware,
                    session_service,
                ))
                .route("/protected", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let unknown_key = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("X-MBX-APIKEY", "unknown-test-key"));
        let unknown_session =
            test::TestRequest::get()
                .uri("/protected")
                .cookie(actix_web::cookie::Cookie::new(
                    "session_id",
                    Uuid::new_v4().to_string(),
                ));

        for (method, request) in [
            (AuthMethod::ApiKey, unknown_key),
            (AuthMethod::Jwt, unknown_session),
        ] {
            let failures = metrics::auth_failures(method);
            let before = failures.get();

            let error = test::try_call_service(&app, request.to_request())
                .await
                .expect_err("unknown credentials must be refused");

            assert_eq!(
                error.as_response_error().status_code(),
                StatusCode::UNAUTHORIZED,
                "{method:?}"
            );
            // The counters are the process's, and other tests here refuse credentials
            // concurrently, so the count can rise by more.
            assert!(failures.get() > before, "{method:?}");
        }
    }

    const ISSUER: &str = "combined-auth-test";
    const AUDIENCE: &str = "combined-auth-audience";

    /// A gateway's Redis, token service and session service, signing with `test_keys`.
    async fn session_services() -> (ConnectionManager, Arc<TokenService>, Arc<SessionService>) {
        let redis_conn = crate::test_redis().await;
        let (encoding_key, decoding_key) = test_keys();
        let token_service = TokenService::new(
            Arc::new(JwtConfig {
                private_key_path: String::new(),
                public_key_path: String::new(),
                access_token_ttl_minutes: 60,
                issuer: ISSUER.to_string(),
                audience: AUDIENCE.to_string(),
            }),
            redis_conn.clone(),
            Arc::new(encoding_key),
            Arc::new(decoding_key),
        );
        let session_service = SessionService::new(redis_conn.clone(), 300);
        (
            redis_conn,
            Arc::new(token_service),
            Arc::new(session_service),
        )
    }

    /// The gateway's authentication with these services, built as `create_app` builds it.
    fn session_auth(
        redis_conn: ConnectionManager,
        token_service: Arc<TokenService>,
        session_service: Arc<SessionService>,
    ) -> CombinedAuthMiddleware {
        let jwt_middleware = JwtAuthentication::with_auto_refresh(
            test_keys().1,
            ISSUER.to_string(),
            AUDIENCE.to_string(),
            redis_conn,
            token_service,
            session_service.clone(),
        );
        CombinedAuthMiddleware::new(jwt_middleware, ApiKeyMiddleware::new(), session_service)
    }

    /// A protected route that answers its user's id.
    async fn user_id_handler(user: AuthenticatedUser) -> HttpResponse {
        HttpResponse::Ok().body(user.user_id.to_string())
    }

    fn with_session(session_id: &str) -> test::TestRequest {
        test::TestRequest::get()
            .uri("/protected")
            .cookie(actix_web::cookie::Cookie::new("session_id", session_id))
    }

    fn status_of(result: Result<ServiceResponse, Error>) -> StatusCode {
        match result {
            Ok(response) => response.status(),
            Err(error) => error.error_response().status(),
        }
    }

    /// A Redis error reading the session is a 503, not the 401 that logs the browser out.
    #[actix_web::test]
    async fn a_session_store_error_is_503_not_a_logout() {
        let (mut conn, token_service, session_service) = session_services().await;
        let session_id = Uuid::new_v4().to_string();
        let session_key = format!("session:{session_id}");
        // Any Redis error will do: GET of a key that holds a set answers WRONGTYPE.
        let _: () = conn.sadd(&session_key, "not-a-session").await.unwrap();
        let app = test::init_service(
            App::new()
                .wrap(session_auth(conn.clone(), token_service, session_service))
                .route("/protected", web::get().to(user_id_handler)),
        )
        .await;

        let result = test::try_call_service(&app, with_session(&session_id).to_request()).await;

        assert_eq!(status_of(result), StatusCode::SERVICE_UNAVAILABLE);
        let _: () = conn.del(&session_key).await.unwrap();
    }

    /// A Redis error extending a live session is a 503, and the session's token is not revoked.
    #[actix_web::test]
    async fn a_store_error_extending_the_session_is_503_and_revokes_nothing() {
        let (mut conn, token_service, session_service) = session_services().await;
        let user_id = Uuid::new_v4();
        let tokens = token_service.mint_tokens(user_id).await.unwrap();
        let session_id = session_service
            .create_session(user_id, tokens.access_token.clone())
            .await
            .unwrap();
        // Any Redis error will do: SADD to the user's session index answers WRONGTYPE once the
        // index holds a string.
        let index_key = format!("user:{user_id}:sessions");
        let _: () = conn.set(&index_key, "not-a-set").await.unwrap();
        let app = test::init_service(
            App::new()
                .wrap(session_auth(
                    conn.clone(),
                    token_service.clone(),
                    session_service.clone(),
                ))
                .route("/protected", web::get().to(user_id_handler)),
        )
        .await;

        let result = test::try_call_service(&app, with_session(&session_id).to_request()).await;

        assert_eq!(status_of(result), StatusCode::SERVICE_UNAVAILABLE);
        let jti = token_service
            .validate_jwt(&tokens.access_token)
            .unwrap()
            .jti;
        let blacklisted: bool = conn.exists(format!("blacklist:{jti}")).await.unwrap();
        assert!(!blacklisted, "a store error must not revoke the token");
        let _: () = conn.del(&index_key).await.unwrap();
        session_service.delete_session(&session_id).await.unwrap();
    }

    /// A live session of a new user, holding an access token that expired two minutes ago.
    async fn session_with_an_expired_token(
        token_service: &TokenService,
        session_service: &SessionService,
    ) -> (Uuid, String, String) {
        let user_id = Uuid::new_v4();
        let now = Utc::now().timestamp();
        let expired_access_token = encode(
            &Header::new(Algorithm::RS256),
            &AppClaims {
                sub: user_id.to_string(),
                exp: now - 120,
                iat: now - 3600,
                iss: ISSUER.to_string(),
                aud: AUDIENCE.to_string(),
                jti: Uuid::new_v4().to_string(),
                token_version: token_service.get_token_version(user_id).await.unwrap(),
            },
            &test_keys().0,
        )
        .unwrap();
        let session_id = session_service
            .create_session(user_id, expired_access_token.clone())
            .await
            .unwrap();
        (user_id, session_id, expired_access_token)
    }

    /// Requests a browser sends together once its access token expired are all served: each
    /// renews the token, and the session keeps the last one.
    #[actix_web::test]
    async fn two_parallel_requests_with_an_expired_access_token_both_succeed() {
        let (conn, token_service, session_service) = session_services().await;
        let (user_id, session_id, expired_access_token) =
            session_with_an_expired_token(&token_service, &session_service).await;
        let app = test::init_service(
            App::new()
                .wrap(session_auth(
                    conn,
                    token_service.clone(),
                    session_service.clone(),
                ))
                .route("/protected", web::get().to(user_id_handler)),
        )
        .await;

        let (first, second) = futures_util::future::join(
            test::try_call_service(&app, with_session(&session_id).to_request()),
            test::try_call_service(&app, with_session(&session_id).to_request()),
        )
        .await;

        assert_eq!(status_of(first), StatusCode::OK);
        assert_eq!(status_of(second), StatusCode::OK);
        let session = session_service.get_session(&session_id).await.unwrap();
        assert_ne!(session.access_token, expired_access_token);
        let claims = token_service.validate_jwt(&session.access_token).unwrap();
        assert_eq!(claims.sub, user_id.to_string());
        session_service.delete_session(&session_id).await.unwrap();
    }

    /// A session's token is renewed at the version it was issued at: after a logout or a
    /// password change bumped the user's version, the expired token is refused, not renewed.
    #[actix_web::test]
    async fn expired_token_of_a_bumped_version_is_not_renewed() {
        let (mut conn, token_service, session_service) = session_services().await;
        let (user_id, session_id, _) =
            session_with_an_expired_token(&token_service, &session_service).await;
        token_service.bump_token_version(user_id).await.unwrap();
        let app = test::init_service(
            App::new()
                .wrap(session_auth(
                    conn.clone(),
                    token_service,
                    session_service.clone(),
                ))
                .route("/protected", web::get().to(user_id_handler)),
        )
        .await;

        let response = test::call_service(&app, with_session(&session_id).to_request()).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = test::read_body_json(response).await;
        assert_eq!(body["error"]["code"], "token_revoked");
        session_service.delete_session(&session_id).await.unwrap();
        let _: () = conn.del(format!("token_version:{user_id}")).await.unwrap();
    }

    #[actix_web::test]
    async fn signed_api_key_request_acts_as_the_key_owner() {
        let redis_conn = crate::test_redis().await;
        let owner = Uuid::new_v4();
        let secret = "owner-test-secret";
        let api_key_middleware = ApiKeyMiddleware::new()
            .with_api_key(ApiKeyConfig {
                user_id: Some(owner),
                ..ApiKeyConfig::new("owner-test-key".to_string())
                    .with_hmac_secret(secret.to_string())
                    .with_permissions(vec![SecurityType::Trade])
            })
            .with_default_security(SecurityType::Trade)
            .with_replay_store(redis_conn.clone());
        let jwt_middleware = JwtAuthentication::new(
            DecodingKey::from_secret(b"unused-test-secret"),
            "vex-gateway".to_string(),
            "api".to_string(),
            redis_conn.clone(),
        );
        let session_service = Arc::new(SessionService::new(redis_conn, 300));
        let middleware =
            CombinedAuthMiddleware::new(jwt_middleware, api_key_middleware, session_service);
        // The extractors `place_order` takes: the caller's identity and the signed JSON body.
        let app = test::init_service(App::new().wrap(middleware).route(
            "/order",
            web::post().to(
                |user: AuthenticatedUserId, _order: web::Json<serde_json::Value>| async move {
                    user.0.to_string()
                },
            ),
        ))
        .await;

        let order = r#"{"market_id":1,"side":"Buy","price":100,"volume":10}"#;
        let query = format!("timestamp={}", Utc::now().timestamp_millis());
        let payload =
            SignatureVerifier::build_request_payload("POST", "/order", &query, order, false);
        let signature =
            SignatureVerifier::generate_hmac(&payload, secret).expect("test request must sign");
        let request = test::TestRequest::post()
            .uri(&format!("/order?{query}&signature={signature}"))
            .insert_header(("X-MBX-APIKEY", "owner-test-key"))
            .insert_header(("content-type", "application/json"))
            .set_payload(order)
            .to_request();
        let response = test::call_service(&app, request).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(test::read_body(response).await, owner.to_string());
    }

    #[actix_web::test]
    async fn read_only_key_reads_but_cannot_trade() {
        let redis_conn = crate::test_redis().await;
        let secret = "read-only-test-secret";
        let api_key_middleware = crate::state::with_api_key_policy(
            ApiKeyMiddleware::new()
                .with_api_key(
                    ApiKeyConfig::new("read-only-test-key".to_string())
                        .with_hmac_secret(secret.to_string())
                        .with_permissions(vec![SecurityType::UserData]),
                )
                .with_replay_store(redis_conn.clone()),
        );
        let jwt_middleware = JwtAuthentication::new(
            DecodingKey::from_secret(b"unused-test-secret"),
            "vex-gateway".to_string(),
            "api".to_string(),
            redis_conn.clone(),
        );
        let session_service = Arc::new(SessionService::new(redis_conn, 300));
        let app = test::init_service(
            App::new()
                .wrap(CombinedAuthMiddleware::new(
                    jwt_middleware,
                    api_key_middleware,
                    session_service,
                ))
                .route("/api/balances", web::get().to(HttpResponse::Ok))
                .route("/api/order", web::post().to(HttpResponse::Ok))
                // A route for the signed-in person only, as `/api/profile` is.
                .route(
                    "/api/profile",
                    web::get().to(|_: AuthenticatedUser| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;
        let signed = |method: &str, path: &str| {
            let query = format!("timestamp={}", Utc::now().timestamp_millis());
            let payload = SignatureVerifier::build_request_payload(method, path, &query, "", false);
            let signature = SignatureVerifier::generate_hmac(&payload, secret).unwrap();
            test::TestRequest::default()
                .method(method.parse().unwrap())
                .uri(&format!("{path}?{query}&signature={signature}"))
                .insert_header(("X-MBX-APIKEY", "read-only-test-key"))
                .to_request()
        };
        let status = |result: Result<ServiceResponse, Error>| match result {
            Ok(response) => response.status(),
            Err(error) => error.as_response_error().status_code(),
        };

        let read = test::try_call_service(&app, signed("GET", "/api/balances")).await;
        assert_eq!(status(read), StatusCode::OK);
        let trade = test::try_call_service(&app, signed("POST", "/api/order")).await;
        assert_eq!(status(trade), StatusCode::FORBIDDEN);
        let profile = test::try_call_service(&app, signed("GET", "/api/profile")).await;
        assert_eq!(status(profile), StatusCode::UNAUTHORIZED);
    }

    /// RFC 7235: the scheme is case-insensitive, as the JWT middleware already reads it.
    #[actix_web::test]
    async fn lowercase_bearer_scheme_is_routed_to_jwt() {
        let redis_conn = crate::test_redis().await;
        let (encoding_key, decoding_key) = test_keys();
        let (issuer, audience) = ("combined-auth-test", "combined-auth-audience");
        let token_service = TokenService::new(
            Arc::new(JwtConfig {
                private_key_path: String::new(),
                public_key_path: String::new(),
                access_token_ttl_minutes: 60,
                issuer: issuer.to_string(),
                audience: audience.to_string(),
            }),
            redis_conn.clone(),
            Arc::new(encoding_key),
            Arc::new(decoding_key.clone()),
        );
        let tokens = token_service
            .mint_tokens(Uuid::new_v4())
            .await
            .expect("token mint must succeed");
        let jwt_middleware = JwtAuthentication::new(
            decoding_key,
            issuer.to_string(),
            audience.to_string(),
            redis_conn.clone(),
        );
        let session_service = Arc::new(SessionService::new(redis_conn.clone(), 300));
        let app = test::init_service(
            App::new()
                .wrap(CombinedAuthMiddleware::new(
                    jwt_middleware,
                    ApiKeyMiddleware::new(),
                    session_service,
                ))
                .route("/protected", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let request = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("bearer {}", tokens.access_token)))
            .to_request();
        let status = match test::try_call_service(&app, request).await {
            Ok(response) => response.status(),
            Err(error) => error.as_response_error().status_code(),
        };

        assert_eq!(status, StatusCode::OK);
    }
}
