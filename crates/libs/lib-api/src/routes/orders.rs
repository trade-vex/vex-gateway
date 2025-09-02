use crate::handlers::orders;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/order", web::post().to(orders::order));
    cfg.route("/order/auth", web::post().to(orders::order_with_auth));
    cfg.route("/order/context", web::post().to(orders::order_with_auth_context));
}