//! lib-api/src/handlers/health_check.rs

use actix_web::HttpResponse;

/// Returns a 200 OK response.
///
/// This function is used to check if the server is running.
pub async fn health_check() -> HttpResponse {
    HttpResponse::Ok().finish()
}
