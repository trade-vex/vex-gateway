//! lib-server/src/app.rs
//!
//! Application factory for creating the Actix-web application

use actix_web::{App, web};
use lib_api::routes;
use lib_auth::middleware::jwt_auth::JwtAuthentication;
use std::sync::Arc;

use crate::combined_auth::CombinedAuthMiddleware;
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
    // Create JWT middleware
    let jwt_middleware = JwtAuthentication::new(
        app_state.decoding_key.clone(),
        app_state.jwt_config.issuer.clone(),
        app_state.jwt_config.audience.clone(),
        app_state.redis_pool.clone(),
    );

    // Create API key middleware
    let api_key_middleware = app_state.init_api_key_middleware();

    // Create combined auth middleware
    let combined_auth_middleware = CombinedAuthMiddleware::new(jwt_middleware, api_key_middleware);

    App::new()
        .app_data(web::Data::new(app_state.clone()))
        .app_data(web::Data::new(app_state.user_repo.clone()))
        .app_data(web::Data::new(app_state.db.clone()))
        .app_data(web::Data::new(app_state.redis_pool.clone()))
        .app_data(web::Data::new(app_state.http_client.clone()))
        .app_data(web::Data::new(app_state.token_service.clone()))
        .app_data(web::Data::new(app_state.oauth_service.clone()))
        .app_data(web::Data::new(app_state.email_service.clone()))
        .app_data(web::Data::new(app_state.gateway_publisher.clone()))
        // Configure public routes (no authentication required)
        .service(web::scope("/api").configure(lib_auth::configure_public_auth_services))
        // Configure lib-api routes (health check, etc.)
        .service(web::scope("/api").configure(routes::health_check_route::configure))
        // Configure protected routes with combined auth middleware
        .service(
            web::scope("/api")
                .wrap(combined_auth_middleware)
                .configure(lib_auth::configure_protected_auth_services)
                // Configure API routes
                .service(web::scope("/api-key").configure(routes::api_key_routes::configure))
                .service(web::scope("/order").configure(routes::order_route::configure))
                .service(web::scope("/user").configure(routes::user_route::configure)),
        )
}
