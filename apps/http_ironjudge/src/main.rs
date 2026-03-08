use anyhow::{Context, Result};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    routing::{get, post},
};
use dotenvy::dotenv;
use http_lib::*;
use redis_lib::{AppState, ping_redis, redis_connection_pooler};
use std::{env, sync::Arc, time::Duration};
use tower_http::{
    cors::{Any, CorsLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::info;

const MAX_BODY_SIZE: usize = 1_048_576;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    dotenv().ok();
    let addr = env::var("HTTPURL").context("HTTPURL not found in .env")?;
    let redisurl = env::var("REDISURL").context("REDISURL not found in .env")?;
    let stream_name = env::var("STREAMNAME").context("STREAMNAME not found in .env")?;
    let pool_size: Option<usize> = env::var("REDIS_POOL_SIZE")
        .ok()
        .and_then(|v| v.parse().ok());

    let redis_pool = redis_connection_pooler(&redisurl, pool_size)
        .context("Failed to create Redis connection pool")?;

    ping_redis(&redis_pool)
        .await
        .map_err(|e| anyhow::anyhow!("Redis health-check failed at startup: {e}"))?;
    info!("Redis connection pool ready");

    let state = Arc::new(AppState {
        redis_pool,
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
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    info!("IronJudge HTTP server is listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Failed to bind TCP listener. Is the port already in use?")?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Server crashed unexpectedly")?;

    info!("Server shut down gracefully");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => info!("Received SIGINT, shutting down..."),
            _ = sigterm.recv() => info!("Received SIGTERM, shutting down..."),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to listen for Ctrl+C");
        info!("Received Ctrl+C, shutting down...");
    }
}
