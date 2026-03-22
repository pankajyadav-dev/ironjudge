mod action;
pub mod middleware;

use action::{enqueue_task, get_conn_with_timeout};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use redis::AsyncCommands;
use redis_lib::AppState;
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, error, warn};
use types_lib::*;
use uuid::Uuid;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json("the service is healthy"))
}

pub async fn test_post(
    State(state): State<Arc<AppState>>,
    Json(mut payload): Json<TaskPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    payload.tasktype = TaskType::Test;
    let random_id = enqueue_task(&state, &payload, "test").await?;
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}

pub async fn run_post(
    State(state): State<Arc<AppState>>,
    Json(mut payload): Json<TaskPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    payload.tasktype = TaskType::Run;
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

    let mut task_status: HashMap<String, String> = redis_con
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
            let error = task_status.remove("error");
            let stdout = task_status.remove("stdout");
            let failed_case = task_status.remove("failedcase");
            let results = task_status.remove("results");

            let lifecycle = match task_status.get("message").map(|s| s.as_str()) {
                Some("success") => MessageType::Success,
                Some("testcasefailed") => MessageType::Testcasefailed,
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
                MessageType::Success => ResponsePayload::success(stdout, results, ttpassed),
                MessageType::Testcasefailed => {
                    ResponsePayload::test_failed(ttpassed, failed_case, stdout)
                }
                MessageType::CompileTimeError => ResponsePayload::compiler_error(error),
                MessageType::RunTimeError => {
                    ResponsePayload::runtime_error(error, ttpassed, stdout, failed_case)
                }
                MessageType::TimeLimitError => ResponsePayload::time_error(ttpassed, stdout),
                MessageType::MemoryLimitError => ResponsePayload::memory_error(ttpassed, stdout),
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
