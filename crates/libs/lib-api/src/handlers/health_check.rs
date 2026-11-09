//! lib-api/src/handlers/health_check.rs

use actix_web::{HttpResponse, web};
use lib_core::{CorrelationService, Publisher};
use std::sync::Arc;

/// Returns whether the VEX Core link is healthy.
pub async fn health_check(
    publisher: web::Data<Arc<Publisher>>,
    correlation_service: web::Data<Arc<CorrelationService>>,
) -> HttpResponse {
    // A lock-free read of the publication's connection flag, which Aeron clears a few seconds
    // after the core stops reading it.
    engine_link_health(publisher.publication.is_connected(), &correlation_service)
}

/// Down while no core reads this gateway's publication: the core is down, or it restarted and
/// lost this gateway's session, which the gateway does not re-establish. Every publish then fails
/// at once, so no request times out. Also down while requests keep timing out (see
/// [`CorrelationService::is_engine_link_healthy`]).
fn engine_link_health(
    core_connected: bool,
    correlation_service: &CorrelationService,
) -> HttpResponse {
    if !core_connected {
        HttpResponse::ServiceUnavailable().body("engine_link: not connected to vex-core")
    } else if !correlation_service.is_engine_link_healthy() {
        HttpResponse::ServiceUnavailable().body("engine_link: response from vex-core timed out")
    } else {
        HttpResponse::Ok().finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use common::OrderCommand;
    use std::time::{Duration, Instant};

    async fn status_and_body(response: HttpResponse) -> (StatusCode, String) {
        let status = response.status();
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[actix_web::test]
    async fn healthy_state_returns_ok() {
        let correlation_service = CorrelationService::new();

        let response = engine_link_health(true, &correlation_service);

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn disconnected_core_is_down_although_nothing_timed_out() {
        // After a core restart every publish fails at once with NotConnected: no request times
        // out, so the timeout window alone would stay healthy.
        let correlation_service = CorrelationService::new();

        let response = engine_link_health(false, &correlation_service);

        assert_eq!(
            status_and_body(response).await,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "engine_link: not connected to vex-core".to_string()
            )
        );
    }

    #[actix_web::test]
    async fn one_timeout_keeps_endpoint_healthy() {
        let correlation_service = CorrelationService::new();
        correlation_service.timeout_request(1);

        let response = engine_link_health(true, &correlation_service);

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn burst_of_timeouts_keeps_endpoint_healthy() {
        let correlation_service = CorrelationService::new();
        correlation_service.timeout_request(1);
        correlation_service.timeout_request(2);
        correlation_service.timeout_request(3);

        let response = engine_link_health(true, &correlation_service);

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn sustained_timeouts_return_service_unavailable_with_failed_check() {
        let correlation_service = CorrelationService::new();
        let t0 = Instant::now();
        correlation_service.timeout_request_at(1, t0);
        correlation_service.timeout_request_at(2, t0 + Duration::from_secs(5));
        correlation_service.timeout_request_at(3, t0 + Duration::from_secs(10));

        let response = engine_link_health(true, &correlation_service);

        assert_eq!(
            status_and_body(response).await,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "engine_link: response from vex-core timed out".to_string()
            )
        );
    }

    #[actix_web::test]
    async fn response_between_timeouts_restarts_window_and_keeps_endpoint_healthy() {
        let correlation_service = CorrelationService::new();
        let t0 = Instant::now();
        correlation_service.timeout_request_at(1, t0);
        correlation_service.timeout_request_at(2, t0 + Duration::from_secs(5));

        let response_id = 3;
        let _receiver = correlation_service.register_request(response_id);
        assert!(correlation_service.route_response(OrderCommand {
            client_order_id: response_id,
            ..OrderCommand::default()
        }));

        correlation_service.timeout_request_at(4, t0 + Duration::from_secs(10));
        correlation_service.timeout_request_at(5, t0 + Duration::from_secs(15));

        let response = engine_link_health(true, &correlation_service);

        assert_eq!(response.status(), StatusCode::OK);
    }
}
