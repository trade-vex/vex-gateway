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
    algorithm: Algorithm, // Add this field
}

impl JwtAuthentication {
    pub fn new(decoding_key: DecodingKey, issuer: String, audience: String) -> Self {
        Self {
            decoding_key,
            issuer,
            audience,
            algorithm: Algorithm::RS256, // Default to RS256 for production
        }
    }

    // Add a test constructor
    #[cfg(test)]
    pub fn new_with_algorithm(
        decoding_key: DecodingKey,
        issuer: String,
        audience: String,
        algorithm: Algorithm,
    ) -> Self {
        Self {
            decoding_key,
            issuer,
            audience,
            algorithm,
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
            algorithm: self.algorithm, // Add this
        }))
    }
}

pub struct JwtAuthenticationMiddleware<S> {
    service: S,
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
    algorithm: Algorithm, // Add this field
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
        let algorithm = self.algorithm; // Add this

        // Extract Authorization header
        let auth_header = req.headers().get("authorization");

        if let Some(auth_value) = auth_header
            && let Ok(auth_str) = auth_value.to_str()
            && let Some(token) = auth_str.strip_prefix("Bearer ")
        {
            // Use the configured algorithm instead of hardcoded RS256
            let mut validation = Validation::new(algorithm);
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, HttpResponse, test, web};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Test handler that requires authentication
    async fn protected_handler(user: AuthenticatedUser) -> HttpResponse {
        HttpResponse::Ok().json(json!({
            "user_id": user.user_id,
            "message": "Protected resource accessed"
        }))
    }

    fn create_test_jwt(user_id: &str, expired: bool) -> String {
        // Use HS256 algorithm to match the test setup
        let key = EncodingKey::from_secret("test_secret".as_ref());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Failed to get current system time - this should never happen")
            .as_secs();
        let exp = if expired { now - 3600 } else { now + 3600 };

        let claims = JwtClaims {
            sub: user_id.to_string(),
            exp: exp as usize,
            iat: now as usize,
            iss: "vex-gateway".to_string(),
            aud: "vex-api".to_string(),
            jti: "test-jti".to_string(),
        };

        // Use HS256 algorithm
        let header = Header::new(Algorithm::HS256);
        encode(&header, &claims, &key)
            .expect("Failed to encode JWT token - this indicates a configuration issue")
    }

    #[tokio::test]
    async fn test_jwt_middleware_valid_token() {
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256, // Use HS256 for tests
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let valid_token = create_test_jwt("123e4567-e89b-12d3-a456-426614174000", false);

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
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256, // Use HS256 for tests
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
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256, // Use HS256 for tests
        );

        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let expired_token = create_test_jwt("123e4567-e89b-12d3-a456-426614174000", true);

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
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256, // Use HS256 for tests
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
        let middleware = JwtAuthentication::new_with_algorithm(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
            Algorithm::HS256, // Use HS256 for tests
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
        // Test that the default constructor uses RS256
        let decoding_key = DecodingKey::from_secret("test_secret".as_ref());
        let middleware = JwtAuthentication::new(
            decoding_key,
            "vex-gateway".to_string(),
            "vex-api".to_string(),
        );

        // Verify the algorithm is RS256 by checking that HS256 tokens fail
        let app = test::init_service(
            App::new()
                .wrap(middleware)
                .route("/protected", web::get().to(protected_handler)),
        )
        .await;

        let hs256_token = create_test_jwt("123e4567-e89b-12d3-a456-426614174000", false);

        let req = test::TestRequest::get()
            .uri("/protected")
            .insert_header(("Authorization", format!("Bearer {}", hs256_token)))
            .to_request();

        let resp = test::call_service(&app, req).await;
        // Should fail because production middleware expects RS256, but token is HS256
        assert_eq!(resp.status(), 401);
    }
}
