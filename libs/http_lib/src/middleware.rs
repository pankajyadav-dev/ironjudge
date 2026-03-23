use axum::{
    Json,
    extract::{Extension, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use redis_lib::{AppState, check_sliding_window_rate_limit};
use std::{sync::Arc, time::Duration};
use tracing::{error, info};
use types_lib::ResponsePayload;
const POOL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct RateLimitConfig {
    pub get_limit: i64,
    pub post_limit: i64,
    pub get_window_seconds: u64,
    pub post_window_seconds: u64,
}

pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    Extension(config): Extension<RateLimitConfig>,
    request: Request,
    next: Next,
) -> Response {
    // info!("middleware is working");
    let method = request.method().clone();
    let method_str = method.as_str();

    let user_id = match request.headers().get("x-user-id") {
        Some(val) => val.to_str().unwrap_or("unknown").to_string(),
        None => {
            info!("Request failed. Header is missing");
            return (StatusCode::BAD_REQUEST, "Missing header").into_response();
        }
    };

    let (limit, window) = match method {
        Method::POST => (config.post_limit, config.post_window_seconds),
        Method::GET => (config.get_limit, config.get_window_seconds),
        _ => (config.get_limit, config.get_window_seconds),
    };

    let identity = format!("{}:{}", method_str, user_id);

    let mut redis_connection = match get_ratelimit_conn_with_timeout(&state).await {
        Ok(conn) => conn,
        Err(_) => {
            info!("Request failed. Redis connection error");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ResponsePayload::error(Some(
                    "Internal server Error: R".to_string(),
                ))),
            )
                .into_response();
        }
    };

    let isallowed = check_sliding_window_rate_limit(
        &mut redis_connection,
        &identity,
        limit,
        window,
        &state.lua_script,
    )
    .await
    .unwrap_or(false);

    if !isallowed {
        info!("Request failed. Rate limit exceeded");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ResponsePayload::error(Some(
                "Rate limit exceeded. Please try again after some time".to_string(),
            ))),
        )
            .into_response();
    }

    next.run(request).await
}

async fn get_ratelimit_conn_with_timeout(
    state: &Arc<AppState>,
) -> Result<deadpool_redis::Connection, (StatusCode, String)> {
    tokio::time::timeout(POOL_TIMEOUT, state.ratelimit_redis_pool.get())
        .await
        .map_err(|_| {
            error!("Timed out waiting for Redis connection from pool");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Service temporarily unavailable".to_string(),
            )
        })?
        .map_err(|e| {
            error!(error = %e, "Failed to get Redis connection from pool");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Redis connection error".to_string(),
            )
        })
}
