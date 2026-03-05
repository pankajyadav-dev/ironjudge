use anyhow::{Context, Result}; 
use axum::{
    Router,
    routing::{get, post},
};
use dotenvy::dotenv;
use http_lib::*;
use redis_lib::{AppState, redis_connection_manager};
use std::{env, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer, 
};
use tracing::info; 

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    dotenv().ok();
    let addr = env::var("HTTPURL").context("HTTPURL not found in .env")?;
    let redisurl = env::var("REDISURL").context("REDISURL not found in .env")?;
    let stream_name = env::var("STREAMNAME").context("STREAMNAME not found in .env")?;
    
    let redis_manager = redis_connection_manager(&redisurl)
        .await
        .context("Failed to connect to Redis pool")?;

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
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    info!("IronJudge HTTP server is listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Failed to bind TCP listener. Is the port already in use?")?;
        
    axum::serve(listener, app)
        .await
        .context("Server crashed unexpectedly")?;
        
    Ok(())
}