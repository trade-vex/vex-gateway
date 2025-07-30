use crate::dto::{CallbackQuery};
use crate::error::AuthError;
use crate::service::{sso_service::SsoService, token_service::TokenService, sso_service::UserRepository}; // Assuming UserRepository is defined in your main app
use crate::state::AppState;
use actix_session::Session;
use actix_web::{web, HttpResponse, Responder};

/// Handler for Step 1: Redirecting the user to the OIDC provider.
pub async fn redirect_to_provider(
    provider: web::Path<String>,
    app_state: web::Data<AppState>,
    user_repo: web::Data<dyn UserRepository>, // Injected from main app
    session: Session,
) -> Result<impl Responder, AuthError> {
    let sso_service = SsoService::new(app_state.clone().into_inner(), user_repo.into_inner());
    let redirect_url = sso_service.generate_redirect_url(&provider.into_inner(), &session)?;

    Ok(HttpResponse::Found()
        .insert_header(("Location", redirect_url))
        .finish())
}

/// Handler for Step 2: Processing the callback from the OIDC provider.
pub async fn handle_callback(
    provider: web::Path<String>,
    query: web::Query<CallbackQuery>,
    app_state: web::Data<AppState>,
    user_repo: web::Data<dyn UserRepository>,
    session: Session,
) -> Result<impl Responder, AuthError> {
    let provider_name = provider.into_inner();
    
    // Process OIDC callback to get our internal user ID
    let sso_service = SsoService::new(app_state.clone().into_inner(), user_repo.into_inner());
    let user_id = sso_service
        .process_callback(&provider_name, query.code.clone(), query.state.clone(), &session)
        .await?;
        
    // Mint our own JWTs for the user
    let token_service = TokenService::new(
        app_state.jwt_config.clone(),
        app_state.redis_pool.clone(),
        app_state.encoding_key.clone(),
    );
    let token_response = token_service.mint_tokens(user_id).await?;

    Ok(HttpResponse::Ok().json(token_response))
}