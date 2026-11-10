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
        let has_bearer_token = if let Some(auth_header) = req.headers().get("authorization")
            && let Ok(auth_str) = auth_header.to_str()
            && auth_str.starts_with("Bearer ")
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

                AuthType::Ambiguous => Err(actix_web::error::ErrorBadRequest(
                    "Ambiguous authentication credentials",
                )),

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
    use rsa::{
        Generate, PrivateKey,
        pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
    };
    use std::sync::OnceLock;
    use uuid::Uuid;

    async fn redis_connection() -> ConnectionManager {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = redis::Client::open(redis_url).expect("test Redis URL must be valid");
        ConnectionManager::new(client)
            .await
            .expect("test Redis must be available")
    }

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
        let redis_conn = redis_connection().await;
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
    async fn refreshes_expired_access_token_for_live_session() {
        let redis_conn = redis_connection().await;
        let (encoding_key, decoding_key) = test_keys();
        let issuer = "combined-auth-test".to_string();
        let audience = "combined-auth-audience".to_string();
        let jwt_config = Arc::new(JwtConfig {
            private_key_path: String::new(),
            public_key_path: String::new(),
            access_token_ttl_minutes: 60,
            refresh_token_ttl_days: 1,
            issuer: issuer.clone(),
            audience: audience.clone(),
        });
        let encoding_key = Arc::new(encoding_key);
        let decoding_key = Arc::new(decoding_key);
        let token_service = Arc::new(TokenService::new(
            jwt_config,
            redis_conn.clone(),
            encoding_key.clone(),
            decoding_key.clone(),
        ));
        let session_service = Arc::new(SessionService::new(redis_conn.clone(), 300));
        let user_id = Uuid::new_v4();
        let tokens = token_service
            .mint_tokens(user_id)
            .await
            .expect("initial token mint must succeed");

        let refresh_key = format!("refresh:{}", tokens.refresh_token);
        let mut cleanup_conn = redis_conn.clone();
        let refresh_value: String = cleanup_conn
            .get(&refresh_key)
            .await
            .expect("refresh token must be stored");
        let record: Vec<&str> = refresh_value.split(':').collect();
        let [_, old_jti, token_version] = record[..] else {
            panic!("refresh token value must be user:jti:version, got {refresh_value}");
        };
        let old_jti = old_jti.to_string();
        let token_version: u64 = token_version.parse().expect("token version must be a u64");

        let now = Utc::now().timestamp();
        let expired_access_token = encode(
            &Header::new(Algorithm::RS256),
            &AppClaims {
                sub: user_id.to_string(),
                exp: now - 120,
                iat: now - 3600,
                iss: issuer.clone(),
                aud: audience.clone(),
                jti: old_jti.clone(),
                pwd: now - 3600,
                token_version,
            },
            &encoding_key,
        )
        .expect("expired access token must encode");
        let session_id = session_service
            .create_session(user_id, expired_access_token.clone(), tokens.refresh_token)
            .await
            .expect("live session must be created");

        let jwt_middleware = JwtAuthentication::with_auto_refresh(
            (*decoding_key).clone(),
            issuer,
            audience,
            redis_conn,
            token_service,
            session_service.clone(),
        );
        let middleware = CombinedAuthMiddleware::new(
            jwt_middleware,
            ApiKeyMiddleware::new(),
            session_service.clone(),
        );
        let app = test::init_service(App::new().wrap(middleware).route(
            "/protected",
            web::get().to(|user: AuthenticatedUser| async move {
                HttpResponse::Ok().body(user.user_id.to_string())
            }),
        ))
        .await;

        let request = test::TestRequest::get()
            .uri("/protected")
            .cookie(actix_web::cookie::Cookie::new(
                "session_id",
                session_id.clone(),
            ))
            .to_request();
        let response = test::call_service(&app, request).await;

        assert_eq!(response.status(), StatusCode::OK);
        let updated_session = session_service
            .get_session(&session_id)
            .await
            .expect("live session must remain available");
        assert_ne!(updated_session.access_token, expired_access_token);

        session_service
            .delete_session(&session_id)
            .await
            .expect("test session cleanup must succeed");
        let _: () = cleanup_conn
            .del(format!("refresh:{}", updated_session.refresh_token))
            .await
            .expect("test refresh token cleanup must succeed");
        let _: () = cleanup_conn
            .del(format!("blacklist:{}", old_jti))
            .await
            .expect("test blacklist cleanup must succeed");
    }

    #[actix_web::test]
    async fn signed_api_key_request_acts_as_the_key_owner() {
        let redis_conn = redis_connection().await;
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
}
