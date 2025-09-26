use crate::dto::auth_request_dto::OAuthCallbackQuery;
use crate::error::auth_error::AuthError;
use crate::service::oauth_service::OAuthService;
use crate::service::token_service::TokenService;
use crate::state::AppState;
use actix_web::{HttpResponse, web};
use std::sync::Arc;
use tracing::error;

/// Initiate OAuth flow
/// GET /auth/oauth/{provider}
pub async fn initiate_oauth(
    provider: web::Path<String>,
    app_state: web::Data<AppState>,
) -> Result<HttpResponse, AuthError> {
    // Create TokenService from AppState components
    let token_service = Arc::new(TokenService::new(
        Arc::new(app_state.jwt_config.clone()),
        app_state.redis_pool.clone(),
        Arc::new(app_state.encoding_key.clone()),
        Arc::new(app_state.decoding_key.clone()),
    ));

    let oauth_service = OAuthService::new(app_state.into_inner(), token_service);

    match oauth_service.get_auth_url(&provider).await {
        Ok(url) => Ok(HttpResponse::Found()
            .append_header(("Location", url))
            .finish()),
        Err(e) => {
            error!("Failed to initiate OAuth: {}", e);
            Err(e)
        }
    }
}

/// Handle OAuth callback
/// GET /auth/oauth/{provider}/callback?code=...&state=...
pub async fn handle_oauth_callback(
    provider: web::Path<String>,
    query: web::Query<OAuthCallbackQuery>,
    app_state: web::Data<AppState>,
) -> Result<HttpResponse, AuthError> {
    // Create TokenService from AppState components
    let token_service = Arc::new(TokenService::new(
        Arc::new(app_state.jwt_config.clone()),
        app_state.redis_pool.clone(),
        Arc::new(app_state.encoding_key.clone()),
        Arc::new(app_state.decoding_key.clone()),
    ));

    let oauth_service = OAuthService::new(app_state.into_inner(), token_service);

    match oauth_service
        .handle_callback(&provider, &query.code, &query.state)
        .await
    {
        Ok(response) => Ok(HttpResponse::Ok().json(response)),
        Err(e) => {
            error!("OAuth callback error: {}", e);
            Err(e)
        }
    }
}

/// Get list of available OAuth providers
/// GET /auth/oauth/providers
pub async fn get_oauth_providers(
    app_state: web::Data<AppState>,
) -> Result<HttpResponse, AuthError> {
    let providers: Vec<String> = app_state.oauth_providers.keys().cloned().collect();

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "providers": providers
    })))
}
