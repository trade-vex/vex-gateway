use crate::handlers::orders;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/order", web::post().to(orders::order));
}
