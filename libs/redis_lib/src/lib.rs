use deadpool_redis::{Config as DConfig, CreatePoolError, Pool, Runtime, Connection};
use redis::streams::StreamReadReply;
use tracing::{error, info};
use std::time::{SystemTime, UNIX_EPOCH};
pub use deadpool_redis::Pool as RedisPool;
pub use redis::Script;



use types_lib::{ResponsePayload, StatusType, TaskPayload};
#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Pool,
    pub ratelimit_redis_pool: Pool,
    pub stream_name: String,
    pub lua_script: redis::Script,
}

pub fn redis_connection_pooler(
    redis_url: &str,
    max_size: Option<usize>,
) -> Result<Pool, CreatePoolError> {
    let mut cfg = DConfig::from_url(redis_url);

    if let Some(size) = max_size {
        cfg.pool = Some(deadpool_redis::PoolConfig {
            max_size: size,
            timeouts: deadpool_redis::Timeouts {
                wait: Some(std::time::Duration::from_secs(3)),
                create: Some(std::time::Duration::from_secs(3)),
                recycle: Some(std::time::Duration::from_secs(2)),
            },
            ..Default::default()
        });
    }

    cfg.create_pool(Some(Runtime::Tokio1))
}

pub async fn ping_redis(pool: &Pool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = pool.get().await?;
    let pong: String = redis::cmd("PING").query_async(&mut *conn).await?;
    if pong != "PONG" {
        return Err(format!("Unexpected PING response: {}", pong).into());
    }
    Ok(())
}

/// Returns Vec of (stream_entry_id, submission_id, TaskPayload).
/// The stream_entry_id is the Redis-generated ID needed for XACK.
pub fn process_redis_stream(reply: StreamReadReply) -> Vec<(String, String, TaskPayload)> {
    let mut extracted_tasks = Vec::new();
    for stream_key in reply.keys.into_iter() {
        for mut stream_id in stream_key.ids.into_iter() {
            let stream_entry_id = stream_id.id.clone();

            let submission_id: String = match stream_id.map.remove("id") {
                Some(val) => redis::from_redis_value(val).unwrap_or_default(),
                None => {
                    error!("Missing 'id' in stream message");
                    continue;
                }
            };

            if let Some(payload_redis_value) = stream_id.map.remove("payload") {
                let payload_string: String = match redis::from_redis_value(payload_redis_value) {
                    Ok(s) => s,
                    Err(e) => {
                        error!(
                            "Failed to read payload as string for ID {}: {}",
                            submission_id, e
                        );
                        continue;
                    }
                };

                match serde_json::from_str::<TaskPayload>(&payload_string) {
                    Ok(parsed_payload) => {
                        extracted_tasks.push((stream_entry_id, submission_id, parsed_payload));
                    }
                    Err(e) => {
                        error!("Failed to parse JSON for ID {}: {}", submission_id, e);
                    }
                }
            } else {
                error!(
                    "Missing 'payload' in stream message for ID {}",
                    submission_id
                );
            }
        }
    }
    extracted_tasks
}

const STATUS_TTL_SECS: i64 = 600;

pub async fn set_processing_status(
    pool: &Pool,
    submission_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = pool.get().await?;
    let key = format!("status:{}", submission_id);
    let _: () = redis::pipe()
        .atomic()
        .hset_multiple(&key, &[("status", "processing"), ("message", "processing")])
        .expire(&key, STATUS_TTL_SECS)
        .query_async(&mut *conn)
        .await?;
    Ok(())
}

pub async fn push_result_to_redis(
    pool: &Pool,
    submission_id: &str,
    response: &ResponsePayload,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = pool.get().await?;
    let key = format!("status:{}", submission_id);

    let status_str = match response.status {
        StatusType::Pending => "pending",
        StatusType::Processing => "processing",
        StatusType::Completed => "completed",
        StatusType::Error => "error",
    };

    let message_str = serde_json::to_value(&response.message)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "error".to_string());

    let mut fields: Vec<(&str, String)> = vec![
        ("status", status_str.to_string()),
        ("message", message_str),
        ("ttpassed", response.ttpassed.to_string()),
    ];

    if let Some(ref err) = response.error {
        fields.push(("error", err.clone()));
    }
    if let Some(ref stdout) = response.stdout {
        fields.push(("stdout", stdout.clone()));
    }
    if let Some(ref fc) = response.failedcase {
        fields.push(("failedcase", fc.clone()));
    }
    if let Some(ref res) = response.results {
        fields.push(("results", res.clone()));
    }

    let str_fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let _: () = redis::pipe()
        .atomic()
        .hset_multiple(&key, &str_fields)
        .expire(&key, STATUS_TTL_SECS)
        .query_async(&mut *conn)
        .await?;

    info!(submission_id = %submission_id, "Result pushed to Redis");
    Ok(())
}

/// Acknowledge a stream message via XACK so the consumer group
/// knows this task has been fully processed.
pub async fn acknowledge_stream_message(
    pool: &Pool,
    stream_key: &str,
    group_name: &str,
    stream_entry_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = pool.get().await?;

    let mut redis_pipe = redis::pipe();
    redis_pipe
        .cmd("XACK")
        .arg(stream_key)
        .arg(group_name)
        .arg(stream_entry_id);
    redis_pipe.cmd("XDEL").arg(stream_key).arg(stream_entry_id);

    let _: () = redis_pipe.query_async(&mut *conn).await?;
    info!(stream_entry_id = %stream_entry_id, "Stream message acknowledged via XACK");
    Ok(())
}




pub async fn check_sliding_window_rate_limit(
    conn: &mut Connection,
    client_id: &str,
    limit: i64,
    window_size_seconds: u64,
    lua_script: &redis::Script
) -> Result<bool, redis::RedisError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    let window_size_ms = window_size_seconds * 1000;

    let current_bucket_id = now / window_size_ms;
    let previous_bucket_id = current_bucket_id - 1;

    let time_into_current_bucket_ms = now % window_size_ms;
    let previous_weight = 1.0 - (time_into_current_bucket_ms as f64 / window_size_ms as f64);

    let current_key = format!("rate_limit:{}:{}", client_id, current_bucket_id);
    let previous_key = format!("rate_limit:{}:{}", client_id, previous_bucket_id);

    let expire_seconds = window_size_seconds * 2; 

    let is_allowed: i32 = lua_script
        .key(&current_key)
        .key(&previous_key)
        .arg(limit)
        .arg(previous_weight) 
        .arg(expire_seconds)
        .invoke_async(conn)
        .await?;

    Ok(is_allowed == 1)
}

