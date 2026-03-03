use axum::{
    Router,
    routing::{get, post},
};
use dotenvy::dotenv;
use http_lib::*;
use std::env;
#[tokio::main]
async fn main() {
    dotenv().ok();
    let addr = env::var("HTTPURL").expect("Httpurl not found");
    let app = Router::new()
        .route("/", get(health))
        .route("/run", post(run_post))
        .route("/test", post(test_post))
        .route("/status/{id}", get(status_get));
    println!("http server is listiong on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
        .await
        .unwrap();
}
