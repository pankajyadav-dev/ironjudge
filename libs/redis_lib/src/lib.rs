use deadpool_redis::{Config as DConfig, CreatePoolError, Pool, Runtime};
use redis::streams::StreamReadReply;
use tracing::error;
use types_lib::TaskPayload;
#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Pool,
    pub stream_name: String,
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

pub fn process_redis_stream(reply: StreamReadReply) -> Vec<(String, TaskPayload)> {
    let mut extracted_tasks = Vec::new();
    for stream_key in reply.keys.into_iter() {
        for mut stream_id in stream_key.ids.into_iter() {
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
                        extracted_tasks.push((submission_id, parsed_payload));
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
