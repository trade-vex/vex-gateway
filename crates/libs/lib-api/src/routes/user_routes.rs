use crate::handlers::user;
use actix_web::web;

/// Configure user-facing API routes (JWT-protected)
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // .route("/profile", web::get().to(user::get_user_profile))
        // .route("/dashboard", web::get().to(user::get_user_dashboard))
        // .route("/settings", web::put().to(user::update_user_settings))
        .route("/profile", web::get().to(crate::handlers::user::get_user_profile))
        .route("/dashboard", web::get().to(crate::handlers::user::get_user_dashboard))
        .route("/settings", web::put().to(crate::handlers::user::update_user_settings))
        .route("/api-keys", web::post().to(user::create_api_key))
        .route("/api-keys", web::get().to(user::list_api_keys))
        .route("/api-keys/{api_key}", web::delete().to(user::delete_api_key));
}