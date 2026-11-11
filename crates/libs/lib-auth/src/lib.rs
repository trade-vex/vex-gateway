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

use actix_web::{Route, web};

pub use config::AuthConfig;
pub use error::auth_error::AuthError;

/// Configure public routes for auth that don't require authentication.
///
/// Signup, login, email verification, and the OAuth redirect and callback limit their own rate per
/// client IP (`EmailService::enforce_rate_limit`). Every other route is wrapped with
/// `rate_limited`: the gateway's rate limit for anonymous callers, per client IP. The routes that
/// send an email or check a code also limit their own rate per email: the password-reset request
/// and code check, and the resend of the verification code.
pub fn configure_public_auth_services(
    cfg: &mut web::ServiceConfig,
    rate_limited: fn(Route) -> Route,
) {
    cfg
        // OAuth strategy routes (all public)
        .service(
            web::scope("/oauth")
                .route(
                    "/providers",
                    rate_limited(web::get().to(handlers::oauth::get_oauth_providers)),
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
                    rate_limited(web::post().to(handlers::oauth::exchange_oauth_login_code)),
                ),
        )
        // Public auth routes
        .route("/signup", web::post().to(handlers::auth::signup))
        .route("/login", web::post().to(handlers::auth::login))
        .route(
            "/refresh",
            rate_limited(web::post().to(handlers::auth::refresh_token)),
        )
        .route(
            "/session/refresh-tokens",
            rate_limited(web::post().to(handlers::session::refresh_session_tokens)),
        )
        .route(
            "/check-email",
            rate_limited(web::post().to(handlers::auth::check_email_exists)),
        )
        .route(
            "/resend-verification",
            rate_limited(web::post().to(handlers::auth::resend_verification_email)),
        )
        .route(
            "/verify-email",
            web::post().to(handlers::auth::verify_email_with_otp),
        )
        // Password reset routes
        .route(
            "/forgot-password",
            rate_limited(web::post().to(handlers::auth::forgot_password)),
        )
        .route(
            "/verify-reset-otp",
            rate_limited(web::post().to(handlers::auth::verify_reset_otp)),
        )
        .route(
            "/reset-password",
            rate_limited(web::post().to(handlers::auth::reset_password)),
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

/// A connection to the tests' Redis: `REDIS_URL`, or the default local port when it is unset. CI
/// sets `REDIS_URL` to its Redis service.
#[cfg(test)]
async fn test_redis() -> redis::aio::ConnectionManager {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(url).expect("REDIS_URL must be a Redis URL");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("these tests need the Redis at REDIS_URL")
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use actix_web::{
        App, Error, HttpResponse,
        body::MessageBody,
        dev::{ServiceRequest, ServiceResponse},
        http::{Method, StatusCode},
        middleware::{Next, from_fn},
        test,
    };

    /// Stands in for the gateway's rate limit: refuses every request with 429.
    async fn refuse(
        req: ServiceRequest,
        _: Next<impl MessageBody>,
    ) -> Result<ServiceResponse, Error> {
        Ok(req.into_response(HttpResponse::TooManyRequests().finish()))
    }

    #[actix_web::test]
    async fn exactly_the_public_routes_without_an_ip_limit_of_their_own_are_rate_limited() {
        let app = test::init_service(App::new().configure(|cfg| {
            configure_public_auth_services(cfg, |route| route.wrap(from_fn(refuse)))
        }))
        .await;

        for (method, path, rate_limited) in [
            (Method::GET, "/oauth/providers", true),
            (Method::GET, "/oauth/google", false),
            (Method::GET, "/oauth/google/callback", false),
            (Method::POST, "/oauth/exchange", true),
            (Method::POST, "/signup", false),
            (Method::POST, "/login", false),
            (Method::POST, "/refresh", true),
            (Method::POST, "/session/refresh-tokens", true),
            (Method::POST, "/check-email", true),
            (Method::POST, "/resend-verification", true),
            (Method::POST, "/verify-email", false),
            (Method::POST, "/forgot-password", true),
            (Method::POST, "/verify-reset-otp", true),
            (Method::POST, "/reset-password", true),
        ] {
            let req = test::TestRequest::default()
                .method(method.clone())
                .uri(path)
                .to_request();
            // A route that is not wrapped reaches its handler, which fails to extract its input
            // here: a 4xx or 5xx other than 429.
            let status = match test::try_call_service(&app, req).await {
                Ok(response) => response.status(),
                Err(error) => error.error_response().status(),
            };
            assert_eq!(
                status == StatusCode::TOO_MANY_REQUESTS,
                rate_limited,
                "{method} {path}: {status}"
            );
        }
    }
}
