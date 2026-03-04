use axum::{
    Router,
    routing::{get, post},
};
use dotenvy::dotenv;
use http_lib::*;
use redis_lib::{AppState, redis_connection_manager};
use std::{env, sync::Arc};
use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() {
    dotenv().ok();
    let addr = env::var("HTTPURL").expect("Httpurl not found");
    let redisurl = env::var("REDISURL").expect("Redis url not found");
    let stream_name = env::var("STREAMNAME").expect("Stream name not found");
    let redis_manager = redis_connection_manager(&redisurl)
        .await
        .expect("Failed to connected redis");

    let state = Arc::new(AppState {
        redis_manager,
        stream_name,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    let app = Router::new()
        .route("/", get(health))
        .route("/run", post(run_post))
        .route("/test", post(test_post))
        .route("/status/{id}", get(status_get))
        .with_state(state)
        .layer(cors);
    println!("http server is listiong on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
        .await
        .unwrap();
}
