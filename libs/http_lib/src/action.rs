use axum::http::StatusCode;
use redis_lib::AppState;
use std::{sync::Arc, time::Duration};
use tracing::{error, info};
use types_lib::TaskPayload;
use uuid::Uuid;

const POOL_TIMEOUT: Duration = Duration::from_secs(2);

const STATUS_TTL_SECS: i64 = 600;

pub async fn get_conn_with_timeout(
    state: &Arc<AppState>,
) -> Result<deadpool_redis::Connection, (StatusCode, String)> {
    tokio::time::timeout(POOL_TIMEOUT, state.redis_pool.get())
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

pub async fn enqueue_task(
    state: &Arc<AppState>,
    payload: &TaskPayload,
    task_type: &str,
) -> Result<String, (StatusCode, String)> {
    let random_id = Uuid::now_v7().to_string();

    let json_payload = serde_json::to_string(payload).map_err(|e| {
        error!(error = %e, task_type, "Failed to serialize TaskPayload");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error while processing request".to_string(),
        )
    })?;

    let mut redis_con = get_conn_with_timeout(state).await?;

    let _: () = redis::pipe()
        .atomic()
        .hset_multiple(format!("status:{}", random_id), &[("status", "pending")])
        .expire(format!("status:{}", random_id), STATUS_TTL_SECS)
        .xadd(
            &state.stream_name,
            "*",
            &[
                ("payload", json_payload.as_str()),
                ("id", random_id.as_str()),
                ("task_type", task_type),
            ],
        )
        .query_async(&mut *redis_con)
        .await
        .map_err(|e| {
            error!(error = %e, submission_id = %random_id, "Failed to write task to Redis stream");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error while queuing task".to_string(),
            )
        })?;

    info!(submission_id = %random_id, task_type, "Task successfully enqueued");
    Ok(random_id)
}
