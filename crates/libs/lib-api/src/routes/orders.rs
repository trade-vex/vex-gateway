use crate::handlers::orders;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/order", web::post().to(orders::order));
    cfg.route("/order/auth", web::post().to(orders::order_with_auth));
    
    cfg.route("/cancel", web::post().to(orders::cancel_order));
    cfg.route("/cancel/auth", web::post().to(orders::cancel_order_with_auth));
}