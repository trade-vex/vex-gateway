//! lib-server/src/app.rs
//!
//! Application factory for creating the Actix-web application

use actix_web::{App, web};
use lib_api::routes;
use lib_auth::middleware::jwt_auth::JwtAuthentication;
use std::sync::Arc;

use crate::state::AppState;

/// Application factory that creates the Actix-web application with all routes and middleware
pub fn create_app(
    app_state: Arc<AppState>,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    // Create services using AppState
    let token_service = app_state.create_token_service();
    let oauth_service = app_state.create_oauth_service();

    // Create JWT middleware
    let jwt_middleware = JwtAuthentication::new(
        app_state.decoding_key.clone(),
        app_state.jwt_config.issuer.clone(),
        app_state.jwt_config.audience.clone(),
        app_state.redis_pool.clone(),
    );

    App::new()
        .app_data(web::Data::new(app_state.clone()))
        .app_data(web::Data::new(app_state.user_repo.clone()))
        .app_data(web::Data::new(app_state.db.clone()))
        .app_data(web::Data::new(app_state.redis_pool.clone()))
        .app_data(web::Data::new(app_state.http_client.clone()))
        .app_data(web::Data::new(token_service))
        .app_data(web::Data::new(oauth_service))
        // Configure public routes (no authentication required)
        .service(web::scope("").configure(lib_auth::configure_public_auth_services))
        // Configure lib-api routes (health check, etc.)
        .service(web::scope("").configure(routes::health_check_route::configure))
        // Configure protected routes with JWT middleware
        .service(
            web::scope("")
                .wrap(jwt_middleware)
                .configure(lib_auth::configure_protected_auth_services)
        )
}
