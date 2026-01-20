use crate::handlers::order;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/order", web::post().to(order::place_order));
    cfg.route("/cancel", web::post().to(order::cancel_order));
}
