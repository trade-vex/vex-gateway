//! lib-server/src/app.rs
//!
//! Application factory for creating the Actix-web application

use actix_web::{App, middleware::from_fn, web};
use lib_api::routes;
use lib_auth::middleware::jwt_auth::JwtAuthentication;
use lib_middleware::ApiKeyStore;
use std::sync::Arc;

use crate::combined_auth::CombinedAuthMiddleware;
use crate::edge;
use crate::rate_limit;
use crate::state::AppState;

/// Application factory that creates the Actix-web application with all routes and middleware
pub fn create_app(
    app_state: Arc<AppState>,
    api_key_store: ApiKeyStore,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    // Create JWT middleware with auto-refresh enabled
    let jwt_middleware = JwtAuthentication::with_auto_refresh(
        app_state.decoding_key.clone(),
        app_state.jwt_config.issuer.clone(),
        app_state.jwt_config.audience.clone(),
        app_state.redis_conn.clone(),
        Arc::new(app_state.token_service.clone()),
        Arc::new(app_state.session_service.clone()),
    );

    // Create combined auth middleware using the API key middleware and session service from AppState
    let combined_auth_middleware = CombinedAuthMiddleware::new(
        jwt_middleware,
        app_state.api_key_middleware.clone(),
        Arc::new(app_state.session_service.clone()),
    );

    App::new()
        // Outermost, so that every response carries the security headers and the request id:
        // 404s and the errors the auth middleware returns included
        .wrap(from_fn(edge::middleware))
        .app_data(web::Data::new(app_state.clone()))
        .app_data(web::Data::new(app_state.trusted_proxies.clone()))
        .app_data(web::Data::new(api_key_store))
        .app_data(web::Data::new(app_state.user_repo.clone()))
        .app_data(web::Data::new(app_state.db.clone()))
        .app_data(web::Data::new(app_state.redis_conn.clone()))
        .app_data(web::Data::new(Arc::new(app_state.token_service.clone())))
        .app_data(web::Data::new(Arc::new(app_state.oauth_service.clone())))
        .app_data(web::Data::new(Arc::new(app_state.email_service.clone())))
        .app_data(web::Data::new(Arc::new(app_state.session_service.clone())))
        .app_data(web::Data::new(app_state.engine_link.clone()))
        .app_data(web::Data::new(app_state.correlation_service.clone()))
        .app_data(web::Data::new(app_state.order_idempotency_store.clone()))
        .app_data(web::Data::new(app_state.rate_limiter.clone()))
        // Configure all routes under a single /api scope
        .service(
            web::scope("/api")
                // Register required data for auth handlers
                .app_data(web::Data::new(app_state.user_repo.clone()))
                .app_data(web::Data::new(Arc::new(app_state.email_service.clone())))
                .app_data(web::Data::new(Arc::new(app_state.token_service.clone())))
                .app_data(web::Data::new(Arc::new(app_state.oauth_service.clone())))
                .app_data(web::Data::new(Arc::new(app_state.session_service.clone())))
                // Public routes (no authentication required). Those whose handlers limit no rate
                // per client IP of their own draw on the general budget, keyed by the client IP.
                .configure(|cfg| {
                    lib_auth::configure_public_auth_services(cfg, |route| {
                        route.wrap(from_fn(rate_limit::anonymous))
                    })
                })
                // Health check route: not rate limited. The load balancer's probes stay out of
                // the budgets, and it reads one atomic flag.
                .configure(routes::health_check_route::configure)
                // Protected routes with combined auth middleware. Their rate limits are counted
                // inside it, per user, once it has named the caller.
                .service(
                    web::scope("")
                        .wrap(combined_auth_middleware)
                        // Placing orders: the orders budget; cancelling: the general one.
                        // Registered before the scope below, which matches every path.
                        .configure(|cfg| routes::order_route::configure(cfg, rate_limit::per_user))
                        // Every other protected route: the general budget
                        .service(
                            web::scope("")
                                .wrap(from_fn(rate_limit::general))
                                .app_data(web::Data::new(Arc::new(app_state.balance_store.clone())))
                                .app_data(web::Data::new(Arc::new(app_state.order_store.clone())))
                                .app_data(web::Data::new(app_state.portfolio_repository.clone()))
                                .app_data(web::Data::new(app_state.trading_repository.clone()))
                                .configure(lib_auth::configure_protected_auth_services)
                                // Session check route (requires authentication)
                                .configure(routes::session_check_route::configure)
                                // Configure API routes
                                .service(
                                    web::scope("/api-key")
                                        .app_data(web::Data::new(
                                            app_state.api_key_repository.clone(),
                                        ))
                                        .configure(routes::api_key_routes::configure),
                                )
                                .configure(routes::user_route::configure),
                        ),
                ),
        )
}
