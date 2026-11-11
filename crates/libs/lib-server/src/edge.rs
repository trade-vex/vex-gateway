//! lib-server/src/edge.rs
//!
//! The app's outermost middleware: every response carries the security headers and the request's
//! id, and everything logged while handling the request carries the id.

use actix_web::{
    Error,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    error::InternalError,
    http::header::{self, HeaderMap, HeaderName, HeaderValue},
    middleware::Next,
};
use tracing::Instrument;
use uuid::Uuid;

/// Correlates a client's report with the gateway's logs: taken from the client when usable,
/// echoed on the response.
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The longest client `X-Request-Id` accepted. A generated one, a UUID, has 36 characters.
const MAX_REQUEST_ID_LEN: usize = 64;

/// Set on every response whose handler did not set the header itself. The API returns
/// credentials, balances and orders: no cache keeps them, no browser sniffs them into another
/// type or frames them, and no link from them sends their URL to another site.
const SECURITY_HEADERS: [(HeaderName, &str); 4] = [
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::CACHE_CONTROL, "no-store"),
    (header::X_FRAME_OPTIONS, "DENY"),
    (header::REFERRER_POLICY, "no-referrer"),
];

/// Runs the request in a span carrying its id, then stamps the response with the security
/// headers and the id.
///
/// Not `DefaultHeaders`: an error a middleware returns instead of a response, such as the auth
/// middleware's 401, passes through it unchanged and is rendered after every middleware has run.
/// This one renders such an error itself, so that response carries the headers too.
pub(crate) async fn middleware(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse, Error> {
    let request_id = request_id(req.headers());
    // A span's level decides only whether the span exists; it logs nothing itself. At ERROR it
    // exists whenever anything is logged, so the request's warnings and errors carry its id at
    // any configured log level.
    let span = tracing::error_span!("request", request_id = %request_id);
    let request_id =
        HeaderValue::try_from(request_id).expect("a request id is a valid header value");

    // Inside the async block, the inner services' synchronous `call`, routing included, runs in
    // the span too, not only the futures they return.
    match async { next.call(req).await }.instrument(span).await {
        Ok(mut res) => {
            stamp(res.headers_mut(), request_id);
            Ok(res.map_into_boxed_body())
        }
        Err(err) => {
            let mut res = err.error_response();
            stamp(res.headers_mut(), request_id);
            Err(InternalError::from_response(err, res).into())
        }
    }
}

/// The client's `X-Request-Id` when it is 1 to 64 characters of `[A-Za-z0-9-]`, so it can inject
/// nothing into a log line or a header; otherwise a new UUID v4.
fn request_id(headers: &HeaderMap) -> String {
    headers
        .get(&X_REQUEST_ID)
        .and_then(|id| id.to_str().ok())
        .filter(|id| {
            (1..=MAX_REQUEST_ID_LEN).contains(&id.len())
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned)
}

/// Adds the security headers the handler did not set, and the request id.
fn stamp(headers: &mut HeaderMap, request_id: HeaderValue) {
    for (name, value) in SECURITY_HEADERS {
        if !headers.contains_key(&name) {
            headers.insert(name, HeaderValue::from_static(value));
        }
    }
    headers.insert(X_REQUEST_ID, request_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{
        App, HttpResponse,
        dev::{Service, ServiceFactory},
        error::{ErrorBadRequest, ErrorUnauthorized},
        http::StatusCode,
        middleware::from_fn,
        test::{self, TestRequest},
        web,
    };
    use std::{
        io,
        sync::{Arc, LazyLock, Mutex},
    };

    /// Collects what the subscriber writes.
    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// What the tests log at WARN and above, through the global subscriber. Installed before any
    /// test sends a request: tracing caches whether a callsite is enabled when it is first hit,
    /// from whichever thread hits it, so a subscriber installed later, or for one thread only,
    /// can find the span disabled.
    static LOGS: LazyLock<Logs> = LazyLock::new(|| {
        let logs = Logs::default();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer({
                let logs = logs.clone();
                move || logs.clone()
            })
            .init();
        logs
    });

    /// The middleware around one route per way a response comes about, besides the app's own 404.
    fn app() -> App<
        impl ServiceFactory<
            ServiceRequest,
            Config = (),
            Response = ServiceResponse,
            Error = Error,
            InitError = (),
        >,
    > {
        LazyLock::force(&LOGS);
        App::new()
            .wrap(from_fn(middleware))
            .route("/ok", web::get().to(HttpResponse::Ok))
            .route(
                "/cached",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .insert_header((header::CACHE_CONTROL, "max-age=60"))
                        .finish()
                }),
            )
            .route(
                "/handler-error",
                web::get().to(|| async { Err::<HttpResponse, _>(ErrorBadRequest("bad input")) }),
            )
            .route(
                "/logs",
                web::get().to(|| async {
                    tracing::warn!("order rejected");
                    HttpResponse::Ok().finish()
                }),
            )
            // Rejects like the auth middleware: an error in place of a response
            .service(
                web::scope("/protected")
                    .wrap_fn(|_req, _srv| async {
                        Err::<ServiceResponse, _>(ErrorUnauthorized("Authentication required"))
                    })
                    .route("", web::get().to(HttpResponse::Ok)),
            )
    }

    /// The status and headers the client receives. The server renders an error a middleware
    /// returned with `Error::error_response`, after every middleware has run.
    async fn send<S, R>(app: &S, req: R) -> (StatusCode, HeaderMap)
    where
        S: Service<R, Response = ServiceResponse, Error = Error>,
    {
        match test::try_call_service(app, req).await {
            Ok(res) => (res.status(), res.headers().clone()),
            Err(err) => {
                let res = err.error_response();
                (res.status(), res.headers().clone())
            }
        }
    }

    fn echoed_id(headers: &HeaderMap) -> &str {
        headers
            .get(&X_REQUEST_ID)
            .expect("every response carries a request id")
            .to_str()
            .unwrap()
    }

    fn assert_generated(headers: &HeaderMap) {
        let id = Uuid::parse_str(echoed_id(headers)).expect("a generated id is a UUID");
        assert_eq!(id.get_version_num(), 4);
    }

    #[actix_web::test]
    async fn normal_error_and_not_found_responses_carry_the_headers_and_the_id() {
        let app = test::init_service(app()).await;

        for (path, status) in [
            ("/ok", StatusCode::OK),
            ("/handler-error", StatusCode::BAD_REQUEST),
            ("/protected", StatusCode::UNAUTHORIZED),
            ("/missing", StatusCode::NOT_FOUND),
        ] {
            let req = TestRequest::get()
                .uri(path)
                .insert_header((X_REQUEST_ID, "client-id-1"))
                .to_request();
            let (actual_status, headers) = send(&app, req).await;

            assert_eq!(actual_status, status, "{path}");
            for (name, value) in SECURITY_HEADERS {
                assert_eq!(headers.get(&name).unwrap(), value, "{path}: {name}");
            }
            assert_eq!(echoed_id(&headers), "client-id-1", "{path}");
        }
    }

    #[actix_web::test]
    async fn a_header_the_handler_set_is_kept() {
        let app = test::init_service(app()).await;

        let (_, headers) = send(&app, TestRequest::get().uri("/cached").to_request()).await;

        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "max-age=60");
        assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
    }

    #[actix_web::test]
    async fn a_valid_request_id_is_echoed() {
        let app = test::init_service(app()).await;

        for id in ["a", "Order-42-retry", &"7".repeat(MAX_REQUEST_ID_LEN)] {
            let req = TestRequest::get()
                .uri("/ok")
                .insert_header((X_REQUEST_ID, id))
                .to_request();
            let (_, headers) = send(&app, req).await;

            assert_eq!(echoed_id(&headers), id);
        }
    }

    #[actix_web::test]
    async fn an_overlong_or_illegal_request_id_is_replaced() {
        let app = test::init_service(app()).await;

        let overlong = "7".repeat(MAX_REQUEST_ID_LEN + 1);
        let rejected: [&[u8]; 7] = [
            overlong.as_bytes(),
            b"",
            b"id with spaces",
            b"id_underscore",
            b"id\tforged=1",
            b"{\"forged\":1}",
            "caf\u{e9}".as_bytes(),
        ];
        for id in rejected {
            let req = TestRequest::get()
                .uri("/ok")
                .insert_header((X_REQUEST_ID, HeaderValue::from_bytes(id).unwrap()))
                .to_request();
            let (_, headers) = send(&app, req).await;

            assert_ne!(echoed_id(&headers).as_bytes(), id);
            assert_generated(&headers);
        }
    }

    #[actix_web::test]
    async fn a_missing_request_id_is_generated_per_request() {
        let app = test::init_service(app()).await;

        let (_, first) = send(&app, TestRequest::get().uri("/ok").to_request()).await;
        let (_, second) = send(&app, TestRequest::get().uri("/missing").to_request()).await;

        assert_generated(&first);
        assert_generated(&second);
        assert_ne!(echoed_id(&first), echoed_id(&second));
    }

    #[actix_web::test]
    async fn what_a_request_logs_carries_its_id_even_at_warn_level() {
        let app = test::init_service(app()).await;

        let req = TestRequest::get()
            .uri("/logs")
            .insert_header((X_REQUEST_ID, "client-id-2"))
            .to_request();
        send(&app, req).await;

        let logs = String::from_utf8(LOGS.0.lock().unwrap().clone()).unwrap();
        assert!(
            logs.lines()
                .any(|line| line.contains("request{request_id=client-id-2}")
                    && line.ends_with("order rejected")),
            "{logs}"
        );
    }
}
