//! User data routes following RESTful conventions
//!
//! Routes under `/user`:
//! - `GET /user/balances` - Get user balances
//! - `GET /user/orders/open` - Get open orders
//! - `GET /user/orders/history` - Get order history
//! - `GET /user/portfolio` - Get portfolio overview
//! - `GET /user/charts/balance` - Get balance chart data
//! - `GET /user/charts/pnl` - Get PnL chart data
//! - `GET /user/favorites` - Get favorite markets
//! - `POST /user/favorites` - Toggle favorite market

use crate::handlers::user;
use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/user")
            .route("/balances", web::get().to(user::get_balances))
            .service(
                web::scope("/orders")
                    .route("/open", web::get().to(user::get_open_orders))
                    .route("/history", web::get().to(user::get_order_history)),
            )
            .route("/portfolio", web::get().to(user::get_portfolio_overview))
            .service(
                web::scope("/charts")
                    .route("/balance", web::get().to(user::get_balance_chart))
                    .route("/pnl", web::get().to(user::get_pnl_chart)),
            )
            .route("/favorites", web::get().to(user::get_favorite_markets))
            .route("/favorites", web::post().to(user::toggle_favorite_market)),
    );
}
