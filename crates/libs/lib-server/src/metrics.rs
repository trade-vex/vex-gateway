//! lib-server/src/metrics.rs
//!
//! The internal metrics listener: `GET /metrics` in the Prometheus text format, on its own address
//! (`metrics.bind`). Only this listener's app routes there; the public app does not.

use actix_web::{App, HttpResponse, HttpServer, dev::Server, web};
use futures_util::future::try_join;
use lib_middleware::metrics;
use std::{future::Future, io, net::TcpListener};
use tracing::error;

/// Runs `server` with the metrics listener on `listener` beside it, in the same runtime, until
/// `server` stops. The listener handles no signals itself: it keeps serving while `server` drains,
/// and stops once `server` has. The returned future fails as soon as either does.
pub(crate) fn alongside(
    server: Server,
    listener: TcpListener,
) -> io::Result<impl Future<Output = io::Result<()>>> {
    let metrics = HttpServer::new(app)
        // A scrape every few seconds needs no more.
        .workers(1)
        .disable_signals()
        .listen(listener)?
        .run();
    let stop = metrics.handle();
    let public = async move {
        server.await?;
        stop.stop(true).await;
        Ok(())
    };
    Ok(async move { try_join(public, metrics).await.map(|_| ()) })
}

/// `GET /metrics` and nothing else.
fn app() -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    App::new().route("/metrics", web::get().to(render))
}

async fn render() -> HttpResponse {
    match metrics::render() {
        Ok(text) => HttpResponse::Ok()
            .content_type(metrics::CONTENT_TYPE)
            .body(text),
        Err(error) => {
            error!(%error, "Failed to render the metrics");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{
        http::{StatusCode, header},
        test,
    };
    use std::{net::SocketAddr, time::Duration};

    #[actix_web::test]
    async fn the_counters_are_served_in_the_prometheus_text_format() {
        let app = test::init_service(app()).await;

        let res =
            test::call_service(&app, test::TestRequest::get().uri("/metrics").to_request()).await;

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4"
        );
        let body = String::from_utf8(test::read_body(res).await.to_vec()).unwrap();
        for line in [
            "# TYPE vex_gateway_orders_total counter",
            "vex_gateway_orders_total{outcome=\"rejected\"} ",
            "# TYPE vex_gateway_cancels_total counter",
            "vex_gateway_cancels_total{outcome=\"busy\"} ",
            "# TYPE vex_gateway_auth_failures_total counter",
            "vex_gateway_auth_failures_total{method=\"api_key\"} ",
        ] {
            assert!(body.contains(line), "{line:?} missing from:\n{body}");
        }
    }

    /// The real public app needs a live engine link to build (`AppState`), so a stand-in serves
    /// on the public listener: what matters is that the metrics come only from their own one.
    #[actix_web::test]
    async fn the_metrics_have_their_own_listener_which_stops_with_the_server() {
        let public_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let metrics_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let public_addr = public_listener.local_addr().unwrap();
        let metrics_addr = metrics_listener.local_addr().unwrap();
        let server =
            HttpServer::new(|| App::new().route("/api/health", web::get().to(HttpResponse::Ok)))
                .workers(1)
                .disable_signals()
                .listen(public_listener)
                .unwrap()
                .run();
        let public = server.handle();
        let serving = actix_web::rt::spawn(alongside(server, metrics_listener).unwrap());

        let client = reqwest::Client::new();
        let get = |addr: SocketAddr, path: &str| client.get(format!("http://{addr}{path}")).send();
        assert_eq!(get(metrics_addr, "/metrics").await.unwrap().status(), 200);
        assert_eq!(get(public_addr, "/metrics").await.unwrap().status(), 404);
        assert_eq!(
            get(metrics_addr, "/api/health").await.unwrap().status(),
            404
        );

        public.stop(true).await;
        tokio::time::timeout(Duration::from_secs(10), serving)
            .await
            .expect("the metrics listener must stop with the server")
            .unwrap()
            .unwrap();
        assert!(
            get(metrics_addr, "/metrics").await.is_err(),
            "the metrics listener must stop with the server"
        );
    }
}
