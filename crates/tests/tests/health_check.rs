// //! tests/health_check.rs

// use lib_server::run;
// use std::net::TcpListener;

// // This test is a simple integration test that checks if the health check endpoint is working.
// // It spawns a new application instance and sends a request to the health check endpoint.
// // It then asserts that the response is successful and that the content length is 0.
// #[tokio::test]
// async fn health_check_works() {
//     // Setup
//     let address = spawn_app();
//     let client = reqwest::Client::new();

//     // Send request via reqwest client
//     let response = client
//         // Use the returned application address
//         .get(format!("{}/health", &address))
//         .send()
//         .await
//         .expect("Failed to execute request.");

//     // Assert
//     assert!(response.status().is_success());
//     assert_eq!(Some(0), response.content_length());

//     // spawn_app is a helper function that spawns a new application instance
//     // and returns the address of the application.
//     fn spawn_app() -> String {
//         let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
//         // We retrieve the port assigned to us by the OS
//         let port = listener.local_addr().unwrap().port();
//         let server = run(listener).expect("Failed to bind address");
//         // Spawn the server task and store the handle
//         tokio::spawn(server);
//         // We return the application address to the caller!
//         format!("http://127.0.0.1:{port}")
//     }
// }
