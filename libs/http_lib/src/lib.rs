use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use redis::AsyncCommands;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use redis_lib::AppState;
use types_lib::*;

/// Pool-get timeout shared across all handlers.
const POOL_TIMEOUT: Duration = Duration::from_secs(2);

/// TTL applied to every `status:{id}` key (1 hour).
const STATUS_TTL_SECS: i64 = 600;

// ---------------------------------------------------------------------------
//  Handlers
// ---------------------------------------------------------------------------

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json("the service is healthy"))
}

pub async fn test_post(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TaskPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let random_id = enqueue_task(&state, &payload, "test").await?;
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}

pub async fn run_post(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TaskPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let random_id = enqueue_task(&state, &payload, "run").await?;
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}

pub async fn status_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ResponsePayload>)> {
    // --- Input validation: reject anything that isn't a valid UUID ---
    if Uuid::parse_str(&id).is_err() {
        warn!(submission_id = %id, "Received invalid UUID in status request");
        return Err((StatusCode::BAD_REQUEST, Json(ResponsePayload::error())));
    }

    let mut redis_con = get_conn_with_timeout(&state).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ResponsePayload::error()),
        )
    })?;

    let task_status: HashMap<String, String> = redis_con
        .hgetall(format!("status:{}", id))
        .await
        .map_err(|e| {
            error!(error = %e, submission_id = %id, "Redis HGETALL failed during status check");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ResponsePayload::error()),
            )
        })?;

    if task_status.is_empty() {
        warn!(submission_id = %id, "Status requested but task was not found in Redis");
        return Err((StatusCode::NOT_FOUND, Json(ResponsePayload::error())));
    }

    let status = task_status.get("status").map(|s| s.as_str());

    debug!(submission_id = %id, current_status = ?status, "Successfully fetched task status");

    let response = match status {
        Some("completed") => {
            let error = task_status.get("error").cloned();
            let stdout = task_status.get("stdout").cloned();
            let failed_case = task_status.get("failedcase").cloned();

            let lifecycle = match task_status.get("message").map(|s| s.as_str()) {
                Some("success") => MessageType::Success,
                Some("compile_time_error") => MessageType::CompileTimeError,
                Some("run_time_error") => MessageType::RunTimeError,
                Some("memory_limit_error") => MessageType::MemoryLimitError,
                Some("time_limit_error") => MessageType::TimeLimitError,
                Some("processing") => MessageType::Processing,
                _ => MessageType::Error,
            };

            let ttpassed = task_status
                .get("ttpassed")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);

            match lifecycle {
                MessageType::Success => ResponsePayload::success(stdout, ttpassed),
                MessageType::CompileTimeError => ResponsePayload::compiler_error(error),
                MessageType::RunTimeError => {
                    ResponsePayload::runtime_error(error, ttpassed, stdout, failed_case)
                }
                MessageType::TimeLimitError => ResponsePayload::time_error(ttpassed),
                MessageType::MemoryLimitError => ResponsePayload::memory_error(ttpassed),
                _ => {
                    error!(submission_id = %id, "Task completed but encountered an unknown lifecycle message");
                    ResponsePayload::error()
                }
            }
        }
        Some("pending") | Some("processing") => ResponsePayload::processing(),
        None => {
            error!(submission_id = %id, "Task exists but 'status' key is missing in the Hash");
            return Err((StatusCode::NOT_FOUND, Json(ResponsePayload::error())));
        }
        Some(unknown_status) => {
            error!(submission_id = %id, status = unknown_status, "Unknown status string encountered");
            ResponsePayload::error()
        }
    };

    Ok((StatusCode::OK, Json(response)))
}

// ---------------------------------------------------------------------------
//  Internal helpers
// ---------------------------------------------------------------------------

/// Get a pooled Redis connection with a timeout so we never hang indefinitely.
async fn get_conn_with_timeout(
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

async fn enqueue_task(
    state: &Arc<AppState>,
    payload: &TaskPayload,
    task_type: &str,
) -> Result<String, (StatusCode, String)> {
    let random_id = Uuid::new_v4().to_string();

    let json_payload = serde_json::to_string(payload).map_err(|e| {
        error!(error = %e, task_type, "Failed to serialize TaskPayload");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error while processing request".to_string(),
        )
    })?;

    let mut redis_con = get_conn_with_timeout(state).await?;

    // Atomic pipeline: set status hash + add to stream + set TTL
    let _: () = redis::pipe()
        .atomic()
        .hset_multiple(
            format!("status:{}", random_id),
            &[("status", "pending")],
        )
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
