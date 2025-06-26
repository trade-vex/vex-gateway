//! lib-api/src/routes/health_check_route.rs

use crate::handlers::health_check;
use actix_web::web;

/// Creates and configures the health check route
///
/// Returns a configured route that handles GET requests to `/health`.
/// # Examples
///
/// ```rust
/// use actix_web::{App, web};
/// use lib_api::routes::health_check_route;
///
/// let app = App::new()
///     .service(web::scope("").configure(health_check_route::configure));
/// ```
///
/// # Returns
///
/// A configured route that can be registered with an Actix web application.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health_check::health_check));
}
