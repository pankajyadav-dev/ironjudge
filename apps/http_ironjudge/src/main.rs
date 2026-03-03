use axum::{Router, routing::get};
use dotenvy::dotenv;
use std::env;

#[tokio::main]
async fn main() {
    dotenv().ok();
    let addr = env::var("HTTPURL").expect("Httpurl not found");
    let app = Router::new()
        .route("/", get(handler))
        .route("/run", get(handler).post(handler))
        .route("/test", get(handler).post(handler))
        .route("/status", get(handler));
    println!("http server is listiong on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
        .await
        .unwrap();
}

async fn handler() -> &'static str {
    println!("request complete");
    "The server is healthy."
}
