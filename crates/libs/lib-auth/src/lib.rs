pub mod config;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod repository;
pub mod service;
pub mod state;
#[cfg(test)]
pub mod tests;
pub mod utils;

use actix_web::web;

pub use config::AuthConfig;
pub use error::auth_error::AuthError;
pub use state::AppState;

pub fn configure_services(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/auth")
            // OAuth strategy routes (existing)
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
                    ),
            )
            // Common auth routes
            .route("/signup", web::post().to(handlers::auth::signup))
            .route("/login", web::post().to(handlers::auth::login))
            .route("/logout", web::post().to(handlers::auth::logout))
            .route("/refresh", web::post().to(handlers::auth::refresh_token))
            .route("/profile", web::get().to(handlers::auth::get_profile))
            .route("/verify-email", web::post().to(handlers::auth::verify_email))
            .route("/change-password", web::post().to(handlers::auth::change_password)),
    );
}
