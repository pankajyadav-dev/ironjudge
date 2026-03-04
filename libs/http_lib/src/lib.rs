use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use redis::AsyncCommands;
use redis_lib::AppState;
use types_lib::*;
use uuid::Uuid;

pub async fn health() -> Result<impl IntoResponse, (StatusCode, String)> {
    Ok((StatusCode::OK, Json("the service is healthy")))
}
pub async fn test_post(
    State(mut state): State<AppState>,
    Json(payload): Json<TaskPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let random_id = Uuid::new_v4().to_string();
    println!("Received Task: {:?}", payload);
    let json_payload = serde_json::to_string(&payload).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Serialization failed: {}", e),
        )
    })?;

    let _: () = redis::pipe()
        .atomic()
        .hset_multiple(format!("status:{}", random_id), &[("status", "queued")])
        .xadd(&state.stream_name, "*", &[("payload", json_payload)])
        .query_async(&mut state.redis_manager)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Redis error: {}", e),
            )
        })?;
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}
pub async fn run_post(
    State(mut state): State<AppState>,
    Json(payload): Json<TaskPayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    println!("Received Task: {:?}", payload);
    let random_id = Uuid::new_v4().to_string();
    let json_payload = serde_json::to_string(&payload).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Serialization failed: {}", e),
        )
    })?;

    let _: () = redis::pipe()
        .atomic()
        .hset_multiple(format!("status:{}", random_id), &[("status", "queued")])
        .xadd(&state.stream_name, "*", &[("payload", json_payload)])
        .query_async(&mut state.redis_manager)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Redis error: {}", e),
            )
        })?;
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}
pub async fn status_get(
    State(mut state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ResponsePayload>)> {
    println!("Received Status Request: {:?}", id);
    let task_status: HashMap<String, String> = state
        .redis_manager
        .hgetall(format!("status:{}", id))
        .await
        .map_err(|e| {
            println!("Error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ResponsePayload::error()),
            )
        })?;
    if task_status.is_empty() {
        return Err((StatusCode::NOT_FOUND, Json(ResponsePayload::error())));
    }

    let status = task_status.get("status").map(|s| s.as_str());

    println!("task Status: {:?}", task_status);
    println!("Status: {:?}", status);
    let response = match status {
        Some("success") => {
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

                MessageType::RunTimeError => ResponsePayload::runtime_error(
                    error,
                    ttpassed,
                    stdout,
                    failed_case
                ),

                MessageType::TimeLimitError=> ResponsePayload::time_error(ttpassed),

                MessageType::MemoryLimitError => ResponsePayload::memory_error(ttpassed),

                _ => ResponsePayload::error(),
            }
        }
        Some("pending") | Some("queued") => ResponsePayload::processing(),
        None => {
            return Err((StatusCode::NOT_FOUND, Json(ResponsePayload::error())));
        }
        Some(unknown_status) => {
            println!("Warning: Unknown status '{}'", unknown_status);
            ResponsePayload::error()
        }
    };
    Ok((StatusCode::OK, Json(response)))
}
