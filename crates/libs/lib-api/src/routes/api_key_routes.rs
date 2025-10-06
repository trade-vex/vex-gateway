//! lib-api/src/routes/api_keys.rs
use actix_web::web;

use crate::handlers::api_key;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("", web::post().to(api_key::create_api_key))
        .route("", web::get().to(api_key::list_api_keys))
        .route("/{id}", web::get().to(api_key::get_api_key))
        .route("/{id}", web::put().to(api_key::update_api_key))
        .route("/{id}", web::delete().to(api_key::delete_api_key))
        .route("/{id}/toggle", web::patch().to(api_key::toggle_api_key));
}
