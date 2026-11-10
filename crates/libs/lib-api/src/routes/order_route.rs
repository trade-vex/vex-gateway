use crate::handlers::order;
use actix_web::{Route, web};
use lib_middleware::metrics::RateLimitBudget;

/// Placing and cancelling orders, each route wrapped with `rate_limited` for the gateway's rate
/// limit of its budget. Placing an order draws on the `orders` budget; cancelling draws on the
/// `general` one, so that a caller who has used up its order budget can still cancel its resting
/// orders.
pub fn configure(cfg: &mut web::ServiceConfig, rate_limited: fn(Route, RateLimitBudget) -> Route) {
    cfg.route(
        "/order",
        rate_limited(web::post().to(order::place_order), RateLimitBudget::Orders),
    );
    cfg.route(
        "/cancel",
        rate_limited(
            web::post().to(order::cancel_order),
            RateLimitBudget::General,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{
        App, Error, HttpResponse,
        body::MessageBody,
        dev::{ServiceRequest, ServiceResponse},
        http::StatusCode,
        middleware::{Next, from_fn},
        test,
    };

    /// Stands in for the gateway's rate limits, each budget used up: refuses every request with
    /// 429, naming the budget.
    fn used_up(route: Route, budget: RateLimitBudget) -> Route {
        async fn orders(
            req: ServiceRequest,
            _: Next<impl MessageBody>,
        ) -> Result<ServiceResponse, Error> {
            Ok(req.into_response(HttpResponse::TooManyRequests().body("orders")))
        }
        async fn general(
            req: ServiceRequest,
            _: Next<impl MessageBody>,
        ) -> Result<ServiceResponse, Error> {
            Ok(req.into_response(HttpResponse::TooManyRequests().body("general")))
        }

        match budget {
            RateLimitBudget::Orders => route.wrap(from_fn(orders)),
            RateLimitBudget::General => route.wrap(from_fn(general)),
        }
    }

    #[actix_web::test]
    async fn placing_draws_on_the_orders_budget_and_cancelling_on_the_general_one() {
        let app = test::init_service(App::new().configure(|cfg| configure(cfg, used_up))).await;

        for (path, budget) in [("/order", "orders"), ("/cancel", "general")] {
            let response =
                test::call_service(&app, test::TestRequest::post().uri(path).to_request()).await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            assert_eq!(test::read_body(response).await, budget, "{path}");
        }
    }
}
