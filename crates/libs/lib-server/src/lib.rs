//! lib-server/src/lib.rs

use actix_web::dev::Server;
use actix_web::{App, HttpServer, web};
use std::net::TcpListener;

use lib_api::routes;

pub fn run(listener: TcpListener) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(|| {
        App::new().service(web::scope("").configure(routes::health_check_route::configure))
    })
    .listen(listener)?
    .run();
    Ok(server)
}
