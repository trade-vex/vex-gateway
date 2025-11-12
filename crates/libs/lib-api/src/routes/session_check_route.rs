//! lib-api/src/routes/session_check_route.rs

use crate::handlers::session_check;
use actix_web::web;

/// Creates and configures the session check route
///
/// Returns a configured route that handles GET requests to `/session-check`.
/// This route requires authentication (JWT or API key) via the combined auth middleware.
/// # Examples
///
/// ```rust
/// use actix_web::{App, web};
/// use lib_api::routes::session_check_route;
///
/// let app = App::new()
///     .service(
///         web::scope("")
///             .wrap(auth_middleware)
///             .configure(session_check_route::configure)
///     );
/// ```
///
/// # Returns
///
/// A configured route that can be registered with an Actix web application.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/session-check", web::get().to(session_check::session_check));
}

