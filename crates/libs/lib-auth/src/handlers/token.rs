use crate::dto::RefreshTokenRequest;
use crate::error::AuthError;
use crate::service::token_service::TokenService;
use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder};

/// Handler for refreshing an access token using a refresh token.
pub async fn refresh(
    req: web::Json<RefreshTokenRequest>,
    app_state: web::Data<AppState>,
) -> Result<impl Responder, AuthError> {
    let token_service = TokenService::new(
        app_state.jwt_config.clone(),
        app_state.redis_pool.clone(),
        app_state.encoding_key.clone(),
    );
    let token_response = token_service.refresh_tokens(&req.refresh_token).await?;

    Ok(HttpResponse::Ok().json(token_response))
}