use axum::{
    extract::{Extension, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use redis_lib::{AppState, check_sliding_window_rate_limit};
use std::sync::Arc;
use tracing::info;

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

    let mut redis_connection = match state.ratelimit_redis_pool.get().await {
        Ok(c) => c,
        Err(_) => {
            info!("Failed to pool connction form rate limiting redis pool");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get redis connection",
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
            "Rate limit exceeded. Please try again",
        )
            .into_response();
    }

    next.run(request).await
}
