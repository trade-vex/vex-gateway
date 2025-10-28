use actix_web::{
    Error, HttpMessage, HttpResponse,
    body::{BoxBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use deadpool_redis::Pool;
use deadpool_redis::redis::AsyncCommands;
use futures_util::future::LocalBoxFuture;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, errors::ErrorKind};
use serde::{Deserialize, Serialize};
use serde_json;
use std::future::{Ready, ready};
use std::rc::Rc;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthenticatedUser {
    pub user_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String, // user_id
    exp: usize,
    iat: usize,
    iss: String,
    aud: String,
    jti: String,
    pwd: i64, // password changed timestamp
}

#[derive(Clone)]
pub struct JwtAuthentication {
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
    algorithm: Algorithm,
    redis_pool: Arc<Pool>,
}

impl JwtAuthentication {
    pub fn new(
        decoding_key: DecodingKey,
        issuer: String,
        audience: String,
        redis_pool: Arc<Pool>,
    ) -> Self {
        Self {
            decoding_key,
            issuer,
            audience,
            algorithm: Algorithm::RS256,
            redis_pool,
        }
    }

    #[cfg(test)]
    pub fn new_with_algorithm(
        decoding_key: DecodingKey,
        issuer: String,
        audience: String,
        algorithm: Algorithm,
        redis_pool: Arc<Pool>,
    ) -> Self {
        Self {
            decoding_key,
            issuer,
            audience,
            algorithm,
            redis_pool,
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for JwtAuthentication
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
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
            service: Rc::new(service),
            decoding_key: self.decoding_key.clone(),
            issuer: self.issuer.clone(),
            audience: self.audience.clone(),
            algorithm: self.algorithm,
            redis_pool: self.redis_pool.clone(),
        }))
    }
}

pub struct JwtAuthenticationMiddleware<S> {
    service: Rc<S>,
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
    algorithm: Algorithm,
    redis_pool: Arc<Pool>,
}

impl<S, B> Service<ServiceRequest> for JwtAuthenticationMiddleware<S>
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
        // Extract JWT parameters
        let decoding_key = self.decoding_key.clone();
        let issuer = self.issuer.clone();
        let audience = self.audience.clone();
        let algorithm = self.algorithm;
        let redis_pool = self.redis_pool.clone();

        // Check Authorization header
        let auth_header = req.headers().get("authorization");

        if let Some(auth_value) = auth_header
            && let Ok(auth_str) = auth_value.to_str()
            && {
                let parts: Vec<&str> = auth_str.split_whitespace().collect();
                parts.len() == 2 && parts[0].eq_ignore_ascii_case("bearer")
            }
            && {
                let parts: Vec<&str> = auth_str.split_whitespace().collect();
                let token = parts[1];
                !token.is_empty()
            }
        {
            // Extract token from header
            let parts: Vec<&str> = auth_str.split_whitespace().collect();
            let token = parts[1];

            // Validate JWT
            tracing::debug!("JWT middleware: Starting JWT validation");
            let mut validation = Validation::new(algorithm);
            validation.set_issuer(&[issuer]);
            validation.set_audience(&[audience]);

            match decode::<JwtClaims>(token, &decoding_key, &validation) {
                Ok(token_data) => {
                    tracing::debug!("JWT middleware: JWT validation successful");
                    if let Ok(user_id) = Uuid::parse_str(&token_data.claims.sub) {
                        let jti = token_data.claims.jti.clone();
                        let authenticated_user: AuthenticatedUser = AuthenticatedUser { user_id };

                        // Store the authenticated user for later
                        req.extensions_mut().insert(authenticated_user.clone());

                        // Clone the service handle to move into the async block
                        let svc = self.service.clone();

                        return Box::pin(async move {
                            tracing::debug!("JWT middleware: Checking blacklist for JTI: {}", jti);
                            // Check if token is blacklisted BEFORE calling the inner service
                            let blacklist_result = match redis_pool.get().await {
                                Ok(mut conn) => {
                                    let blacklist_key = format!("blacklist:{}", jti);
                                    match conn.exists::<_, bool>(&blacklist_key).await {
                                        Ok(exists) => {
                                            if exists {
                                                tracing::warn!(
                                                    "Attempted use of blacklisted token with JTI: {}",
                                                    jti
                                                );
                                            }
                                            Ok(exists)
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "Failed to check token blacklist: {}",
                                                e
                                            );
                                            Err("Redis blacklist check failed")
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to get Redis connection for blacklist check: {}",
                                        e
                                    );
                                    Err("Redis connection failed")
                                }
                            };

                            match blacklist_result {
                                Ok(true) => {
                                    tracing::warn!("JWT middleware: Token is blacklisted with JTI: {}", jti);
                                    // Token is blacklisted, short-circuit with an unauthorized response
                                    let (req, _) = req.into_parts();
                                    let response =
                                        HttpResponse::Unauthorized().json(serde_json::json!({
                                            "error": {
                                                "code": "token_revoked",
                                                "message": "Token has been revoked",
                                                "status": 401
                                            }
                                        }));
                                    Ok(ServiceResponse::new(req, response).map_into_boxed_body())
                                }
                                Ok(false) => {
                                    tracing::debug!("JWT middleware: Token not blacklisted, proceeding to handler");
                                    // Token is valid and not blacklisted, proceed to the inner service
                                    let res = svc.call(req).await?;
                                    Ok(res.map_into_boxed_body())
                                }
                                Err(_) => {
                                    // Redis failure - fail closed with service unavailable
                                    let (req, _) = req.into_parts();
                                    let response = HttpResponse::ServiceUnavailable().json(serde_json::json!({
                                        "error": {
                                            "code": "service_unavailable",
                                            "message": "Authentication service temporarily unavailable",
                                            "status": 503
                                        }
                                    }));
                                    Ok(ServiceResponse::new(req, response).map_into_boxed_body())
                                }
                            }
                        });
                    }
                }
                Err(e) => {
                    let (error_code, error_message) = match *e.kind() {
                        ErrorKind::ExpiredSignature => {
                            ("token_expired", "Token has expired")
                        }
                        _ => {
                            ("invalid_token", "Invalid authentication token")
                        }
                    };
                    
                    tracing::warn!("JWT middleware: JWT validation failed: {} ({}: {})", e, error_code, error_message);
                    
                    return Box::pin(async move {
                        let (req, _) = req.into_parts();
                        let response = HttpResponse::Unauthorized().json(serde_json::json!({
                            "error": {
                                "code": error_code,
                                "message": error_message,
                                "status": 401
                            }
                        }));
                        Ok(ServiceResponse::new(req, response).map_into_boxed_body())
                    });
                }
            }
        } else {
            tracing::debug!("JWT middleware: No JWT token provided");
        }

        // No valid JWT found - return unauthorized
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, HttpResponse, test, web};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Helper function to create a test Redis pool
    fn create_test_redis_pool() -> Arc<Pool> {
        use deadpool_redis::Config as RedisPoolConfig;
        // Use environment variable for Redis URL, fallback to localhost for local testing
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let redis_cfg = RedisPoolConfig::from_url(redis_url);
        Arc::new(
            redis_cfg
                .create_pool(Some(deadpool_redis::Runtime::Tokio1))
                .expect("Failed to create test Redis pool"),
        )
    }

    // Test handler that requires authentication
    async fn protected_handler(user: AuthenticatedUser) -> HttpResponse {
        HttpResponse::Ok().json(json!({
            "user_id": user.user_id,
            "message": "Protected resource accessed"
        }))
    }

    // Test handler that tracks if it was called using a thread-local approach
    use std::cell::RefCell;
    use std::sync::Arc;

    thread_local! {
        static HANDLER_CALLED: RefCell<bool> = const { RefCell::new(false) };
    }

    async fn tracking_handler(user: AuthenticatedUser) -> HttpResponse {
        HANDLER_CALLED.with(|called| {
            *called.borrow_mut() = true;
        });
        HttpResponse::Ok().json(json!({
            "user_id": user.user_id,
            "message": "Handler was executed"
        }))
    }

    fn create_test_jwt(user_id: &str, expired: bool, jti: &str) -> String {
        let key = EncodingKey::from_secret("test_secret".as_ref());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Failed to get current system time")
            .as_secs();
        let exp = if expired { now - 3600 } else { now + 3600 };

        let claims = JwtClaims {
            sub: user_id.to_string(),
            exp: exp as usize,
            iat: now as usize,
            iss: "vex-gateway".to_string(),
            aud: "vex-api".to_string(),
            jti: jti.to_string(),
            pwd: now as i64, // Use current time as password timestamp for tests
        };

        let header = Header::new(Algorithm::HS256);
        encode(&header, &claims, &key).expect("Failed to encode JWT token")
    }

    #[tokio::test]
    async fn test_jwt_middleware_valid_token() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool.clone(),
        );

        // Clean up any existing blacklist entry
        if let Ok(mut conn) = redis_pool.get().await {
            let _: Result<(), _> = conn.del("blacklist:test-jti-valid").await;
        }

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let valid_token = create_test_jwt(
            "123e4567-e89b-12d3-a456-426614174000",
            false,
            "test-jti-valid",
        );

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", valid_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_jwt_middleware_invalid_token() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool,
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", "Bearer invalid_token"))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn test_jwt_middleware_expired_token() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool,
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let expired_token = create_test_jwt(
            "123e4567-e89b-12d3-a456-426614174000",
            true,
            "test-jti-expired",
        );

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", expired_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn test_jwt_middleware_missing_token() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool,
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let req = test::TestRequest::get().uri("/protected").to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn test_jwt_middleware_malformed_header() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool,
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", "InvalidFormat"))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn test_jwt_middleware_production_rs256() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            redis_pool,
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let hs256_token = create_test_jwt(
            "123e4567-e89b-12d3-a456-426614174000",
            false,
            "test-jti-rs256",
        );

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", hs256_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;
        // Should fail because production middleware expects RS256, but token is HS256
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn test_jwt_middleware_blacklisted_token() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool.clone(),
        );

        // Add a JTI to the blacklist
        if let Ok(mut conn) = redis_pool.get().await {
            let _: () = conn
                .set_ex("blacklist:test-jti-blacklisted", "1", 3600)
                .await
                .unwrap();
        }

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let valid_token = create_test_jwt(
            "123e4567-e89b-12d3-a456-426614174000",
            false,
            "test-jti-blacklisted",
        );

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", valid_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;
        // Should fail because token is blacklisted
        assert_eq!(resp.status(), 401);

        // Verify the error message
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["error"]["code"], "token_revoked");
    }

    #[tokio::test]
    async fn test_jwt_middleware_blacklisted_token_does_not_execute_handler() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool.clone(),
        );

        // Reset the handler called flag
        HANDLER_CALLED.with(|called| {
            *called.borrow_mut() = false;
        });

        // Add a JTI to the blacklist
        if let Ok(mut conn) = redis_pool.get().await {
            let _: () = conn
                .set_ex("blacklist:test-jti-no-exec", "1", 3600)
                .await
                .unwrap();
        }

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(tracking_handler)),
        )
        .await;

        let valid_token = create_test_jwt(
            "123e4567-e89b-12d3-a456-426614174000",
            false,
            "test-jti-no-exec",
        );

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", valid_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;

        // Should get 401 because token is blacklisted
        assert_eq!(resp.status(), 401);

        // Verify the error message
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["error"]["code"], "token_revoked");

        // Most importantly, verify the handler was NOT executed
        let handler_was_called = HANDLER_CALLED.with(|called| *called.borrow());
        assert!(
            !handler_was_called,
            "Handler should NOT have been executed for blacklisted token!"
        );
    }

    #[tokio::test]
    async fn test_jwt_middleware_valid_token_executes_handler() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let redis_pool = create_test_redis_pool();
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256,
            redis_pool.clone(),
        );

        // Reset the handler called flag
        HANDLER_CALLED.with(|called| {
            *called.borrow_mut() = false;
        });

        // Clean up any existing blacklist entry
        if let Ok(mut conn) = redis_pool.get().await {
            let _: Result<(), _> = conn.del("blacklist:test-jti-exec").await;
        }

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(tracking_handler)),
        )
        .await;

        let valid_token = create_test_jwt(
            "123e4567-e89b-12d3-a456-426614174000",
            false,
            "test-jti-exec",
        );

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", valid_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;

        // Should get 200 because token is valid and not blacklisted
        assert_eq!(resp.status(), 200);

        // Verify the response
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["message"], "Handler was executed");

        // Verify the handler WAS executed
        let handler_was_called = HANDLER_CALLED.with(|called| *called.borrow());
        assert!(
            handler_was_called,
            "Handler should have been executed for valid non-blacklisted token!"
        );
    }
}
