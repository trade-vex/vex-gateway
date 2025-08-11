use crate::handlers::trading;
use actix_web::web;

/// Configure trading API routes (Binance-compatible)
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // Market data (NONE security)
        .route("/ping", web::get().to(trading::ping))
        .route("/time", web::get().to(trading::server_time))
        .route("/exchangeInfo", web::get().to(trading::exchange_info))
        
        // Trading (TRADE security)
        .route("/order", web::post().to(trading::place_order))
        .route("/order", web::delete().to(trading::cancel_order))
        
        // Account data (USER_DATA security)
        .route("/account", web::get().to(trading::get_account_info))
        .route("/openOrders", web::get().to(trading::get_open_orders))
        
        // User data stream (USER_STREAM security)
        .route("/userDataStream", web::post().to(trading::create_listen_key))
        .route("/userDataStream", web::put().to(trading::keep_alive_listen_key))
        .route("/userDataStream", web::delete().to(trading::close_listen_key));
}