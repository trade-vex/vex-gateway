//! Order routes following RESTful conventions
//!
//! Routes:
//! - `POST /orders` - Place a new order
//! - `DELETE /orders/{order_id}` - Cancel an order by ID

use crate::handlers::order;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/orders")
            .route("", web::post().to(order::place_order))
            .route("/{order_id}", web::delete().to(order::cancel_order)),
    );
}
