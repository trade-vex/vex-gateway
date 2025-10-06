//! lib-api/src/routes/users.rs

use crate::handlers::users;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        .route("/balances", web::get().to(users::get_balances))
        .route("/orders/open", web::get().to(users::get_open_orders));
}
