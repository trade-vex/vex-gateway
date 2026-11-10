//! lib-server/src/rate_limit.rs
//!
//! The API's rate limits (`[rate_limit]`): the `orders` budget for placing orders, and the
//! `general` budget for every other route that has no limit of its own, cancelling orders
//! included. Each is a fixed window per caller, counted in the gateway's Redis with the primitive
//! the auth endpoints' own limits use (`lib_middleware::rate_limit`).
//!
//! On a protected route the caller is the user the handlers act for, as `AuthenticatedUserId`
//! resolves it: the JWT's, or the API key's owner. A user's sessions and keys therefore share its
//! budgets. These middlewares sit inside the auth middleware, so a request it refuses is not
//! counted. Nor is a request with an API key that has no owner: it acts for no one, and
//! `AuthenticatedUserId` refuses it with 401 here, on every protected route, before it is counted,
//! as it does in the handlers that act for a user. On a public route the caller is the client IP,
//! resolved through the trusted proxies, an IPv6 client by its /64 (`client_ip_key`).

use actix_web::{
    Error, Route,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    error::InternalError,
    middleware::{Next, from_fn},
    web,
};
use lib_auth::{AuthError, middleware::jwt_auth::service_unavailable};
use lib_config::RateLimitConfig;
use lib_middleware::{
    client_ip::client_ip_key,
    metrics::{self, RateLimitBudget},
    rate_limit,
};
use lib_utils::AuthenticatedUserId;
use redis::aio::ConnectionManager;
use tracing::error;

/// Why a request was refused with 503: Redis could not count it.
const STORE_UNAVAILABLE: &str = "Rate limit store is unavailable; please retry";

/// The budgets' limits and the Redis that counts them: the app data of the middlewares here.
#[derive(Clone)]
pub struct RateLimiter {
    redis: ConnectionManager,
    limits: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(redis: ConnectionManager, limits: RateLimitConfig) -> Self {
        Self { redis, limits }
    }

    /// Counts a request of `caller` against its `budget`. Over the budget it is refused with 429
    /// and `Retry-After`, the seconds left in the window, as the auth endpoints' limits refuse.
    /// When Redis cannot count it, it is refused with the 503 the auth middleware gives when Redis
    /// fails it. Either way it goes no further.
    async fn count(&self, budget: RateLimitBudget, caller: &str) -> Result<(), Error> {
        let limit = match budget {
            RateLimitBudget::Orders => self.limits.orders,
            RateLimitBudget::General => self.limits.general,
        };
        let key = format!("rate_limit:api:{}:{caller}", budget.label());

        match rate_limit::hit(&self.redis, &key, limit).await {
            Ok(None) => Ok(()),
            Ok(Some(retry_after_seconds)) => {
                metrics::rate_limited(budget).inc();
                Err(AuthError::RateLimited {
                    retry_after_seconds,
                }
                .into())
            }
            Err(error) => {
                error!(
                    %error,
                    budget = budget.label(),
                    "Rate limit store unavailable; request refused"
                );
                Err(InternalError::from_response(
                    STORE_UNAVAILABLE,
                    service_unavailable(STORE_UNAVAILABLE),
                )
                .into())
            }
        }
    }
}

/// Wraps `route`, a protected route, with its caller's `budget`.
pub(crate) fn per_user(route: Route, budget: RateLimitBudget) -> Route {
    match budget {
        RateLimitBudget::Orders => route.wrap(from_fn(orders)),
        RateLimitBudget::General => route.wrap(from_fn(general)),
    }
}

/// A protected route on its user's `orders` budget.
async fn orders(
    limiter: web::Data<RateLimiter>,
    user: AuthenticatedUserId,
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    limiter
        .count(RateLimitBudget::Orders, &format!("user:{}", user.0))
        .await?;
    next.call(req).await
}

/// A protected route on its user's `general` budget.
pub(crate) async fn general(
    limiter: web::Data<RateLimiter>,
    user: AuthenticatedUserId,
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    limiter
        .count(RateLimitBudget::General, &format!("user:{}", user.0))
        .await?;
    next.call(req).await
}

/// A public route on its client IP's `general` budget.
pub(crate) async fn anonymous(
    limiter: web::Data<RateLimiter>,
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    limiter
        .count(RateLimitBudget::General, &client_ip_key(req.request()))
        .await?;
    next.call(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{
        App, HttpMessage, HttpResponse,
        dev::{Service, ServiceFactory},
        http::{
            StatusCode,
            header::{HeaderMap, RETRY_AFTER},
        },
        test::{self, TestRequest},
    };
    use lib_api::routes::order_route;
    use lib_auth::middleware::jwt_auth::AuthenticatedUser;
    use lib_middleware::{TrustedProxies, api_key::security::ApiKeyConfig, rate_limit::Limit};
    use redis::AsyncCommands;
    use std::{
        net::{IpAddr, Ipv6Addr, SocketAddr},
        sync::Arc,
    };
    use uuid::Uuid;

    /// Sets the authenticated user, as the JWT middleware does.
    const TEST_USER: &str = "x-test-user";
    /// Sets an API key owned by this user (`none`: without an owner), as the API-key middleware
    /// does.
    const TEST_KEY_OWNER: &str = "x-test-key-owner";

    /// Both budgets allow `max_requests` a minute.
    fn limits(max_requests: u64) -> RateLimitConfig {
        let limit = Limit {
            max_requests,
            window_seconds: 60,
        };
        RateLimitConfig {
            orders: limit,
            general: limit,
        }
    }

    /// The budgets wired as `create_app` wires them, behind the trusted proxies 10.0.0.0/8:
    /// `GET /public` on the client IP's general budget without authentication; and behind a
    /// stand-in for the auth middleware that names the caller from the test headers, the real
    /// order routes and `GET /other` on the user's general budget. The order handlers find none
    /// of their app data here, so a request the rate limit lets through fails in its handler,
    /// never with 429.
    fn app(
        redis: ConnectionManager,
        limits: RateLimitConfig,
    ) -> App<
        impl ServiceFactory<
            ServiceRequest,
            Config = (),
            Response = ServiceResponse,
            Error = Error,
            InitError = (),
        >,
    > {
        App::new()
            .app_data(web::Data::new(RateLimiter::new(redis, limits)))
            .app_data(web::Data::new(
                TrustedProxies::parse(&["10.0.0.0/8"]).unwrap(),
            ))
            .route(
                "/public",
                web::get().to(HttpResponse::Ok).wrap(from_fn(anonymous)),
            )
            .service(
                web::scope("")
                    .wrap_fn(|req, srv| {
                        let header = |name| {
                            req.headers()
                                .get(name)
                                .map(|value| value.to_str().unwrap().to_string())
                        };
                        if let Some(user) = header(TEST_USER) {
                            req.extensions_mut().insert(AuthenticatedUser {
                                user_id: user.parse().unwrap(),
                            });
                        }
                        if let Some(owner) = header(TEST_KEY_OWNER) {
                            let mut key = ApiKeyConfig::new("test-key".to_string());
                            key.user_id = owner.parse().ok();
                            req.extensions_mut().insert(Arc::new(key));
                        }
                        srv.call(req)
                    })
                    .configure(|cfg| order_route::configure(cfg, per_user))
                    .service(
                        web::scope("")
                            .wrap(from_fn(general))
                            .route("/other", web::get().to(HttpResponse::Ok)),
                    ),
            )
    }

    /// The status, headers and body the client receives: the server renders an error a
    /// middleware returned with `Error::error_response`.
    async fn send<S, R>(app: &S, req: R) -> (StatusCode, HeaderMap, String)
    where
        S: Service<R, Response = ServiceResponse, Error = Error>,
    {
        let response = match test::try_call_service(app, req).await {
            Ok(response) => response.into_parts().1,
            Err(error) => error.error_response(),
        };
        let (status, headers) = (response.status(), response.headers().clone());
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        (status, headers, String::from_utf8(body.to_vec()).unwrap())
    }

    /// An address in a /64 no other test, or earlier run, has counted.
    fn fresh_ip() -> IpAddr {
        IpAddr::V6(Ipv6Addr::from(Uuid::new_v4().as_u128()))
    }

    fn from_peer(req: TestRequest, peer: IpAddr) -> TestRequest {
        req.peer_addr(SocketAddr::new(peer, 1234))
    }

    fn get(path: &str) -> TestRequest {
        from_peer(TestRequest::get().uri(path), fresh_ip())
    }

    fn post(path: &str) -> TestRequest {
        from_peer(TestRequest::post().uri(path), fresh_ip())
    }

    fn as_user(req: TestRequest, user: Uuid) -> TestRequest {
        req.insert_header((TEST_USER, user.to_string()))
    }

    #[actix_web::test]
    async fn over_its_budget_a_caller_gets_429_with_the_rest_of_the_window() {
        let app = test::init_service(app(crate::test_redis().await, limits(2))).await;
        let user = Uuid::new_v4();
        let refused = metrics::rate_limited(RateLimitBudget::General);
        let refused_before = refused.get();

        for _ in 0..2 {
            let (status, ..) = send(&app, as_user(get("/other"), user).to_request()).await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, headers, body) = send(&app, as_user(get("/other"), user).to_request()).await;

        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        let retry_after: u64 = headers
            .get(RETRY_AFTER)
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            (1..=60).contains(&retry_after),
            "Retry-After: {retry_after}"
        );
        assert!(body.contains("\"RATE_LIMITED\""), "{body}");
        // The counter is the process's, and other tests here go over the general budget
        // concurrently, so the count can rise by more.
        assert!(refused.get() > refused_before);
    }

    #[actix_web::test]
    async fn a_user_shares_its_budgets_with_its_api_keys_and_not_with_other_users() {
        let app = test::init_service(app(crate::test_redis().await, limits(1))).await;
        let (user, other_user) = (Uuid::new_v4(), Uuid::new_v4());

        assert_eq!(
            send(&app, as_user(get("/other"), user).to_request())
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            send(&app, as_user(get("/other"), user).to_request())
                .await
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
        // The user's API key, from another address, draws on the same budget.
        let with_key = get("/other").insert_header((TEST_KEY_OWNER, user.to_string()));
        assert_eq!(
            send(&app, with_key.to_request()).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );

        // Another user has a budget of its own.
        assert_eq!(
            send(&app, as_user(get("/other"), other_user).to_request())
                .await
                .0,
            StatusCode::OK
        );
    }

    #[actix_web::test]
    async fn a_user_over_the_orders_budget_can_still_cancel() {
        let app = test::init_service(app(crate::test_redis().await, limits(1))).await;
        let user = Uuid::new_v4();
        let refused = metrics::rate_limited(RateLimitBudget::Orders);
        let refused_before = refused.get();

        assert_ne!(
            send(&app, as_user(post("/order"), user).to_request())
                .await
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            send(&app, as_user(post("/order"), user).to_request())
                .await
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert!(refused.get() > refused_before);

        // Cancelling draws on the general budget, which the orders above did not touch...
        assert_ne!(
            send(&app, as_user(post("/cancel"), user).to_request())
                .await
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
        // ...and which other requests use up like any other route's.
        assert_eq!(
            send(&app, as_user(post("/cancel"), user).to_request())
                .await
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[actix_web::test]
    async fn anonymous_callers_are_keyed_by_the_trusted_client_ip() {
        let app = test::init_service(app(crate::test_redis().await, limits(1))).await;
        let via_proxy = |client: IpAddr| {
            from_peer(
                TestRequest::get().uri("/public"),
                "10.0.0.2".parse().unwrap(),
            )
            .insert_header(("X-Forwarded-For", client.to_string()))
        };

        // Behind the trusted proxy, each client its own budget.
        let client = fresh_ip();
        assert_eq!(
            send(&app, via_proxy(client).to_request()).await.0,
            StatusCode::OK
        );
        assert_eq!(
            send(&app, via_proxy(client).to_request()).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            send(&app, via_proxy(fresh_ip()).to_request()).await.0,
            StatusCode::OK
        );

        // An untrusted peer is its own address, whatever X-Forwarded-For it sends.
        let peer = fresh_ip();
        let spoofed = || {
            from_peer(TestRequest::get().uri("/public"), peer)
                .insert_header(("X-Forwarded-For", fresh_ip().to_string()))
        };
        assert_eq!(send(&app, spoofed().to_request()).await.0, StatusCode::OK);
        assert_eq!(
            send(&app, spoofed().to_request()).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[actix_web::test]
    async fn an_ipv6_client_is_keyed_by_its_64() {
        let app = test::init_service(app(crate::test_redis().await, limits(1))).await;
        let network = Uuid::new_v4().as_u128() & !u128::from(u64::MAX);
        let public =
            |ip: u128| from_peer(TestRequest::get().uri("/public"), Ipv6Addr::from(ip).into());

        assert_eq!(
            send(&app, public(network | 1).to_request()).await.0,
            StatusCode::OK
        );
        // Another address in the same /64 is the same client...
        assert_eq!(
            send(&app, public(network | 0xabcd).to_request()).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        // ...and an address in the next /64 another one.
        let next_network = network.wrapping_add(1 << 64);
        assert_eq!(
            send(&app, public(next_network | 1).to_request()).await.0,
            StatusCode::OK
        );
    }

    #[actix_web::test]
    async fn an_api_key_without_an_owner_is_refused_before_it_is_counted() {
        let app = test::init_service(app(crate::test_redis().await, limits(1))).await;
        let peer = fresh_ip();
        let ownerless_key = from_peer(TestRequest::get().uri("/other"), peer)
            .insert_header((TEST_KEY_OWNER, "none"));

        let (status, _, body) = send(&app, ownerless_key.to_request()).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, "API key is not owned by a user");
        // Not counted against its address either: that budget is still whole.
        assert_eq!(
            send(
                &app,
                from_peer(TestRequest::get().uri("/public"), peer).to_request()
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    #[actix_web::test]
    async fn a_redis_error_refuses_the_request_with_the_json_503() {
        let redis = crate::test_redis().await;
        let app = test::init_service(app(redis.clone(), limits(100))).await;
        let user = Uuid::new_v4();
        // INCR fails on a value that is not an integer.
        let _: () = redis
            .clone()
            .set_ex(
                format!("rate_limit:api:general:user:{user}"),
                "not-a-count",
                60,
            )
            .await
            .unwrap();

        let (status, headers, body) = send(&app, as_user(get("/other"), user).to_request()).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            headers.get(actix_web::http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({
                "error": {
                    "code": "service_unavailable",
                    "message": "Rate limit store is unavailable; please retry",
                    "status": 503
                }
            })
        );
    }
}
