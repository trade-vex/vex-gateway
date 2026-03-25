pub mod config;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod repository;
pub mod service;
#[cfg(test)]
pub mod tests;
pub mod utils;

use actix_web::web;

pub use config::AuthConfig;
pub use error::auth_error::AuthError;

/// Configure public routes for auth that don't require authentication
pub fn configure_public_auth_services(cfg: &mut web::ServiceConfig) {
    cfg
        // OAuth strategy routes (all public)
        .service(
            web::scope("/oauth")
                .route(
                    "/providers",
                    web::get().to(handlers::oauth::get_oauth_providers),
                )
                .route(
                    "/{provider}",
                    web::get().to(handlers::oauth::initiate_oauth),
                )
                .route(
                    "/{provider}/callback",
                    web::get().to(handlers::oauth::handle_oauth_callback),
                )
                // Mobile-only: exchange login_code + verifier for session_id
                .route(
                    "/exchange",
                    web::post().to(handlers::oauth::exchange_oauth_login_code),
                ),
        )
        // Public auth routes
        .route("/signup", web::post().to(handlers::auth::signup))
        .route("/login", web::post().to(handlers::auth::login))
        .route("/refresh", web::post().to(handlers::auth::refresh_token))
        .route(
            "/session/refresh-tokens",
            web::post().to(handlers::session::refresh_session_tokens),
        )
        .route(
            "/session/renew",
            web::post().to(handlers::oauth::renew_session),
        )
        .route(
            "/check-email",
            web::post().to(handlers::auth::check_email_exists),
        )
        .route(
            "/resend-verification",
            web::post().to(handlers::auth::resend_verification_email),
        )
        .route(
            "/verify-email",
            web::post().to(handlers::auth::verify_email_with_otp),
        )
        // Password reset routes
        .route(
            "/forgot-password",
            web::post().to(handlers::auth::forgot_password),
        )
        .route(
            "/verify-reset-otp",
            web::post().to(handlers::auth::verify_reset_otp),
        )
        .route(
            "/reset-password",
            web::post().to(handlers::auth::reset_password),
        );
}

/// Configure protected routes for auth that require authentication
pub fn configure_protected_auth_services(cfg: &mut web::ServiceConfig) {
    cfg
        // Protected auth routes
        .route("/logout", web::post().to(handlers::auth::logout))
        .route("/profile", web::get().to(handlers::auth::get_profile))
        .route(
            "/verify-email",
            web::post().to(handlers::auth::verify_email),
        )
        .route(
            "/change-password",
            web::post().to(handlers::auth::change_password),
        )
        // Session routes (public but require session cookie)
        .route(
            "/session/info",
            web::get().to(handlers::session::get_session_info),
        )
        .route(
            "/session/refresh",
            web::post().to(handlers::session::refresh_session),
        );
}
