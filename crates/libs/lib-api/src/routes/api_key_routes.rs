//! lib-api/src/routes/api_keys.rs
use actix_web::web;

use crate::handlers::api_keys;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        .route("", web::post().to(api_keys::create_api_key))
        .route("", web::get().to(api_keys::list_api_keys))
        .route("/{id}", web::get().to(api_keys::get_api_key))
        .route("/{id}", web::put().to(api_keys::update_api_key))
        .route("/{id}", web::delete().to(api_keys::delete_api_key))
        .route("/{id}/toggle", web::patch().to(api_keys::toggle_api_key));
}
