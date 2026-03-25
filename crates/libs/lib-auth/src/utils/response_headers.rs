use actix_web::HttpResponseBuilder;

pub const SESSION_ID_HEADER: &str = "X-Session-Id";
pub const REFRESH_TOKEN_HEADER: &str = "X-Refresh-Token";
pub const SESSION_EXPIRES_IN_HEADER: &str = "X-Session-Expires-In";
pub const REFRESH_EXPIRES_IN_HEADER: &str = "X-Refresh-Expires-In";
pub const AUTH_EXPOSE_HEADERS: &str =
    "X-Session-Id, X-Refresh-Token, X-Session-Expires-In, X-Refresh-Expires-In";

pub fn set_auth_response_headers(
    builder: &mut HttpResponseBuilder,
    session_id: Option<&str>,
    refresh_token: &str,
    session_expires_in: u64,
    refresh_expires_in: u64,
) {
    if let Some(session_id) = session_id {
        builder.insert_header((SESSION_ID_HEADER, session_id.to_string()));
    }

    builder
        .insert_header((REFRESH_TOKEN_HEADER, refresh_token.to_string()))
        .insert_header((SESSION_EXPIRES_IN_HEADER, session_expires_in.to_string()))
        .insert_header((REFRESH_EXPIRES_IN_HEADER, refresh_expires_in.to_string()))
        .insert_header(("Access-Control-Expose-Headers", AUTH_EXPOSE_HEADERS));
}
