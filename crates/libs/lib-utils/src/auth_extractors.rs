//! Authentication extractors for handlers
//!
//! This module provides extractors that can be used in handlers to get authenticated user information
//! from either JWT tokens or API keys.

use actix_web::{dev::Payload, Error as ActixError, FromRequest, HttpMessage, HttpRequest};
use futures_util::future::{ready, Ready};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::api_key::security::ApiKeyConfig;

/// Represents an authenticated user ID extracted from either JWT or API key authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedUserId(pub Uuid);

impl From<AuthenticatedUser> for AuthenticatedUserId {
    fn from(user: AuthenticatedUser) -> Self {
        Self(user.user_id)
    }
}

impl From<&AuthenticatedUser> for AuthenticatedUserId {
    fn from(user: &AuthenticatedUser) -> Self {
        Self(user.user_id)
    }
}

/// Extractor that gets the authenticated user ID from either JWT or API key authentication
impl FromRequest for AuthenticatedUserId {
    type Error = ActixError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let extensions = req.extensions();

        if let Some(jwt_user) = extensions.get::<AuthenticatedUser>() {
            return ready(Ok(AuthenticatedUserId::from(jwt_user)));
        }

        // An API key acts for the user who owns it.
        if let Some(api_key) = extensions.get::<Arc<ApiKeyConfig>>() {
            return ready(api_key.user_id.map(AuthenticatedUserId).ok_or_else(|| {
                actix_web::error::ErrorUnauthorized("API key is not owned by a user")
            }));
        }

        ready(Err(actix_web::error::ErrorUnauthorized(
            "Authentication required",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{
        dev::{Extensions, ServiceResponse},
        http::StatusCode,
        test as actix_test, web, App,
    };
    use lib_middleware::api_key::security::SecurityType;

    fn test_user_id() -> Uuid {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    fn jwt_user() -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: test_user_id(),
        }
    }

    fn api_key(user_id: Option<Uuid>) -> Arc<ApiKeyConfig> {
        Arc::new(ApiKeyConfig {
            user_id,
            ..ApiKeyConfig::new("test-api-key-123".to_string())
                .with_hmac_secret("test-secret-key".to_string())
                .with_permissions(vec![SecurityType::Trade])
        })
    }

    /// Sends a request, with the extensions `authenticate` leaves on it as the auth middleware
    /// would, to a handler that answers with the `AuthenticatedUserId` it extracted.
    async fn whoami(authenticate: impl FnOnce(&mut Extensions)) -> ServiceResponse {
        let app = actix_test::init_service(App::new().route(
            "/whoami",
            web::get().to(|user: AuthenticatedUserId| async move { user.0.to_string() }),
        ))
        .await;
        let request = actix_test::TestRequest::get().uri("/whoami").to_request();
        authenticate(&mut request.extensions_mut());

        actix_test::call_service(&app, request).await
    }

    #[test]
    fn from_jwt_user() {
        assert_eq!(AuthenticatedUserId::from(&jwt_user()).0, test_user_id());
    }

    #[test]
    fn from_owned_jwt_user() {
        assert_eq!(AuthenticatedUserId::from(jwt_user()).0, test_user_id());
    }

    #[test]
    fn serializes_and_deserializes() {
        let serialized = serde_json::to_string(&AuthenticatedUserId(test_user_id())).unwrap();
        assert!(serialized.contains("550e8400-e29b-41d4-a716-446655440000"));

        let deserialized: AuthenticatedUserId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.0, test_user_id());
    }

    #[test]
    fn clone_keeps_the_user_id() {
        let user_id = AuthenticatedUserId(test_user_id());

        assert_eq!(user_id.clone().0, user_id.0);
    }

    #[actix_web::test]
    async fn jwt_request_reaches_handler_as_the_jwt_user() {
        let response = whoami(|extensions| {
            extensions.insert(jwt_user());
        })
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            actix_test::read_body(response).await,
            test_user_id().to_string()
        );
    }

    #[actix_web::test]
    async fn api_key_request_reaches_handler_as_the_key_owner() {
        let response = whoami(|extensions| {
            extensions.insert(api_key(Some(test_user_id())));
        })
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            actix_test::read_body(response).await,
            test_user_id().to_string()
        );
    }

    #[actix_web::test]
    async fn api_key_without_an_owner_is_unauthorized() {
        let response = whoami(|extensions| {
            extensions.insert(api_key(None));
        })
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn unauthenticated_request_is_unauthorized() {
        let response = whoami(|_| {}).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
