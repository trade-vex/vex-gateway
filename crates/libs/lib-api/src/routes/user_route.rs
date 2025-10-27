//! lib-api/src/routes/users.rs

use crate::handlers::user;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/balances", web::get().to(user::get_balances))
        .route("/orders/open", web::get().to(user::get_open_orders))
        .route("/orders/history", web::get().to(user::get_order_history));
}
