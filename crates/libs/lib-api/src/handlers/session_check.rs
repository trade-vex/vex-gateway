//! lib-api/src/handlers/session_check.rs

use actix_web::HttpResponse;

/// Returns a 200 OK response if authentication is successful.
///
/// This endpoint is protected by the combined auth middleware (JWT or API key).
/// If the request reaches this handler, authentication has already been validated.
/// This function is used to verify that a session/token is valid.
pub async fn session_check() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "message": "Authentication successful"
    }))
}

