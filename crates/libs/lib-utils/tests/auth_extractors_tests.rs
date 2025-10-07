//! tests for authentication extractors

use actix_web::{
    test, web, App, HttpMessage,
    http::StatusCode,
};
use lib_utils::auth_extractors::{
    AuthenticatedUserId, AuthContext, 
    extract_user_id_from_request, extract_user_id_from_query
};
use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::api_key::security::{ApiKeyConfig, SecurityType};
use uuid::Uuid;
use std::sync::Arc;

fn create_test_user_id() -> Uuid {
    Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
}

fn create_test_authenticated_user() -> AuthenticatedUser {
    AuthenticatedUser {
        user_id: create_test_user_id(),
    }
}

fn create_test_api_key_config() -> ApiKeyConfig {
    ApiKeyConfig::new("test-api-key-123".to_string())
        .with_hmac_secret("test-secret-key".to_string())
        .with_permissions(vec![SecurityType::Trade])
        .with_enabled(true)
}

#[tokio::test]
async fn test_authenticated_user_id_from_jwt_user() {
    let jwt_user = create_test_authenticated_user();
    let auth_user_id = AuthenticatedUserId::from(&jwt_user);
    
    assert_eq!(auth_user_id.0, create_test_user_id());
}

#[tokio::test]
async fn test_authenticated_user_id_from_jwt_user_owned() {
    let jwt_user = create_test_authenticated_user();
    let auth_user_id = AuthenticatedUserId::from(jwt_user);
    
    assert_eq!(auth_user_id.0, create_test_user_id());
}

#[tokio::test]
async fn test_authenticated_user_id_serialization() {
    let auth_user_id = AuthenticatedUserId(create_test_user_id());
    
    // testing serialization
    let serialized = serde_json::to_string(&auth_user_id).unwrap();
    assert!(serialized.contains("550e8400-e29b-41d4-a716-446655440000"));
    
    // testing deserialization
    let deserialized: AuthenticatedUserId = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.0, create_test_user_id());
}

#[tokio::test]
async fn test_authenticated_user_id_from_request_jwt_success() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|user: AuthenticatedUserId| async move {
                format!("User ID: {}", user.0)
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(create_test_authenticated_user());

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_authenticated_user_id_from_request_api_key_error() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|user: AuthenticatedUserId| async move {
                format!("User ID: {}", user.0)
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(Arc::new(create_test_api_key_config()));

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_authenticated_user_id_from_request_no_auth() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|user: AuthenticatedUserId| async move {
                format!("User ID: {}", user.0)
            }))
    ).await;

    let req = test::TestRequest::get().uri("/test").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_context_jwt_success() {
    let jwt_user = create_test_authenticated_user();
    let auth_context = AuthContext::Jwt(jwt_user.clone());
    
    assert!(auth_context.is_jwt());
    assert!(!auth_context.is_api_key());
    assert_eq!(auth_context.user_id(), Some(create_test_user_id()));
    assert!(auth_context.jwt_user().is_some());
    assert!(auth_context.api_key_config().is_none());
}

#[tokio::test]
async fn test_auth_context_api_key_success() {
    let api_config = create_test_api_key_config();
    let auth_context = AuthContext::ApiKey(Arc::new(api_config.clone()));
    
    assert!(!auth_context.is_jwt());
    assert!(auth_context.is_api_key());
    assert_eq!(auth_context.user_id(), None);
    assert!(auth_context.jwt_user().is_none());
    assert!(auth_context.api_key_config().is_some());
}

#[tokio::test]
async fn test_auth_context_from_request_jwt_success() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|ctx: AuthContext| async move {
                if ctx.is_jwt() {
                    format!("JWT User: {}", ctx.user_id().unwrap())
                } else {
                    "API Key".to_string()
                }
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(create_test_authenticated_user());

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("JWT User"));
    assert!(body_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_auth_context_from_request_api_key_success() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|ctx: AuthContext| async move {
                if ctx.is_jwt() {
                    format!("JWT User: {}", ctx.user_id().unwrap())
                } else {
                    "API Key".to_string()
                }
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(Arc::new(create_test_api_key_config()));

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body_str, "API Key");
}

#[tokio::test]
async fn test_auth_context_from_request_no_auth() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|ctx: AuthContext| async move {
                if ctx.is_jwt() {
                    format!("JWT User: {}", ctx.user_id().unwrap())
                } else {
                    "API Key".to_string()
                }
            }))
    ).await;

    let req = test::TestRequest::get().uri("/test").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_extract_user_id_from_request_jwt_success() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let user_id = extract_user_id_from_request(&req, None).unwrap();
                format!("User ID: {}", user_id)
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(create_test_authenticated_user());

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_extract_user_id_from_request_api_key_with_fallback() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let fallback_user_id = create_test_user_id();
                let user_id = extract_user_id_from_request(&req, Some(fallback_user_id)).unwrap();
                format!("User ID: {}", user_id)
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(Arc::new(create_test_api_key_config()));

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_extract_user_id_from_request_api_key_no_fallback() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let result = extract_user_id_from_request(&req, None);
                if result.is_err() {
                    "Error".to_string()
                } else {
                    "Success".to_string()
                }
            }))
    ).await;

    let mut req = test::TestRequest::get().uri("/test").to_request();
    req.extensions_mut().insert(Arc::new(create_test_api_key_config()));

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body_str, "Error");
}

#[tokio::test]
async fn test_extract_user_id_from_request_no_auth() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let result = extract_user_id_from_request(&req, None);
                if result.is_err() {
                    "Error".to_string()
                } else {
                    "Success".to_string()
                }
            }))
    ).await;

    let req = test::TestRequest::get().uri("/test").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body_str, "Error");
}

#[tokio::test]
async fn test_extract_user_id_from_query_success() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let user_id = extract_user_id_from_query(&req).unwrap();
                format!("User ID: {}", user_id)
            }))
    ).await;

    let user_id = create_test_user_id();
    let req = test::TestRequest::get()
        .uri(&format!("/test?user_id={}", user_id))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_extract_user_id_from_query_missing_parameter() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let result = extract_user_id_from_query(&req);
                if result.is_err() {
                    "Error".to_string()
                } else {
                    "Success".to_string()
                }
            }))
    ).await;

    let req = test::TestRequest::get().uri("/test").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body_str, "Error");
}

#[tokio::test]
async fn test_extract_user_id_from_query_invalid_format() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let result = extract_user_id_from_query(&req);
                if result.is_err() {
                    "Error".to_string()
                } else {
                    "Success".to_string()
                }
            }))
    ).await;

    let req = test::TestRequest::get()
        .uri("/test?user_id=invalid-uuid")
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body_str, "Error");
}

#[tokio::test]
async fn test_extract_user_id_from_query_empty_query() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let result = extract_user_id_from_query(&req);
                if result.is_err() {
                    "Error".to_string()
                } else {
                    "Success".to_string()
                }
            }))
    ).await;

    let req = test::TestRequest::get()
        .uri("/test?")
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(body_str, "Error");
}

#[tokio::test]
async fn test_extract_user_id_from_query_multiple_params() {
    let app = test::init_service(
        App::new()
            .route("/test", web::get().to(|req: actix_web::HttpRequest| async move {
                let user_id = extract_user_id_from_query(&req).unwrap();
                format!("User ID: {}", user_id)
            }))
    ).await;

    let user_id = create_test_user_id();
    let req = test::TestRequest::get()
        .uri(&format!("/test?user_id={}&other_param=value", user_id))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    
    let body = test::read_body(resp).await;
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_authenticated_user_id_clone() {
    let auth_user_id = AuthenticatedUserId(create_test_user_id());
    let cloned = auth_user_id.clone();
    
    assert_eq!(auth_user_id.0, cloned.0);
}

#[tokio::test]
async fn test_auth_context_clone() {
    let jwt_user = create_test_authenticated_user();
    let auth_context = AuthContext::Jwt(jwt_user.clone());
    let cloned = auth_context.clone();
    
    assert_eq!(auth_context.user_id(), cloned.user_id());
    assert_eq!(auth_context.is_jwt(), cloned.is_jwt());
}

#[tokio::test]
async fn test_authenticated_user_id_debug() {
    let auth_user_id = AuthenticatedUserId(create_test_user_id());
    let debug_str = format!("{:?}", auth_user_id);
    
    assert!(debug_str.contains("AuthenticatedUserId"));
    assert!(debug_str.contains("550e8400-e29b-41d4-a716-446655440000"));
}

#[tokio::test]
async fn test_auth_context_debug() {
    let jwt_user = create_test_authenticated_user();
    let auth_context = AuthContext::Jwt(jwt_user);
    let debug_str = format!("{:?}", auth_context);
    
    // Debug output format may vary, just check it's not empty
    assert!(!debug_str.is_empty());
    assert!(debug_str.len() > 10);
}
