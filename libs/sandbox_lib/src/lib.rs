pub mod action;
pub mod cgroups;
pub mod compilesandbox;
pub mod processor;
pub mod sandbox;
pub mod seccomp;

pub use action::get_heavy_tasks_threads;
use processor::process_single_submission;
use redis_lib::{
    RedisPool, acknowledge_stream_message, push_result_to_redis, set_processing_status,
};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{error, info};
use types_lib::{ResponsePayload, TaskPayload};

pub async fn execute_submissions_detached(
    tasks: Vec<(String, String, TaskPayload)>,
    concurrency_limiter: Arc<Semaphore>,
    redis_pool: Arc<RedisPool>,
    stream_key: String,
    group_name: String,
) {
    for (stream_entry_id, submission_id, payload) in tasks {
        let permit = concurrency_limiter
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore closed");
        let pool = redis_pool.clone();
        let s_key = stream_key.clone();
        let g_name = group_name.clone();

        tokio::spawn(async move {
            if let Err(e) = set_processing_status(&pool, &submission_id).await {
                error!(
                    "failed to set processing status for {}: {}",
                    submission_id, e
                );
            }

            let response = match process_single_submission(&submission_id, &payload).await {
                Ok(resp) => resp,
                Err(e) => {
                    error!("internal sandbox error for {}: {}", submission_id, e);
                    ResponsePayload::error(None)
                }
            };

            if let Err(e) = push_result_to_redis(&pool, &submission_id, &response).await {
                error!(
                    "failed to push result to redis for {}: {}",
                    submission_id, e
                );
            }

            if let Err(e) =
                acknowledge_stream_message(&pool, &s_key, &g_name, &stream_entry_id).await
            {
                error!(
                    "failed to xack stream message {} for {}: {}",
                    stream_entry_id, submission_id, e
                );
            }

            info!(
                "job {} completed with status: {:?}, message: {:?}",
                submission_id, response.status, response.message
            );

            drop(permit);
        });
    }
}
