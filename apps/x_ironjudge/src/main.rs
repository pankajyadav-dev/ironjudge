use anyhow::{Context, Result};
use dotenvy::dotenv;
use redis::{
    AsyncCommands,
    streams::{StreamReadOptions, StreamReadReply},
};
use redis_lib::{process_redis_stream, redis_connection_pooler};
use sandbox_lib::{execute_submissions_detached, get_heavy_tasks_threads};
use std::{env, sync::Arc};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
#[derive(Debug, Clone)]
struct EngineConfig {
    redis_url: String,
    stream_key: String,
    group_name: String,
    consumer_name: String,
    redispayload_len: usize,
}

impl EngineConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            redis_url: env::var("REDISURL").context("Missing REDISURL")?,
            stream_key: env::var("STREAMNAME").context("Missing STREAMNAME")?,
            group_name: env::var("GROUPNAME").context("Missing GROUPNAME")?,
            consumer_name: env::var("CONSUMERNAME").context("Missing CONSUMERNAME")?,
            redispayload_len: env::var("REDISPAYLOADLEN")
                .context("Missing REDISPAYLOADLEN")?
                .parse::<usize>()
                .context("REDISPAYLOADLEN  must be valid number")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    dotenv().ok();
    let heavy_thread_limit = get_heavy_tasks_threads();
    let limiter = Arc::new(Semaphore::new(heavy_thread_limit));
    let config = EngineConfig::from_env()?;
    info!(
        "Starting IronJudge execution engine on stream: {}",
        config.stream_key
    );

    let redis_pool = redis_connection_pooler(&config.redis_url, None)
        .context("Critical: Failed to create Redis pool")?;
    let shared_redis_pool = Arc::new(redis_pool);
    let mut setup_conn = shared_redis_pool
        .get()
        .await
        .context("Failed to get Redis connection for initialization")?;
    let group_setup: redis::RedisResult<()> = setup_conn
        .xgroup_create_mkstream(&config.stream_key, &config.group_name, "$")
        .await;

    match group_setup {
        Ok(_) => {
            info!(
                "Created new stream '{}' and group '{}'.",
                config.stream_key, config.group_name
            );
        }
        Err(e) if e.code() == Some("BUSYGROUP") => {
            info!(
                "Consumer group '{}' already exists. Ready for submissions.",
                config.group_name
            );
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Critical Redis setup error: {}", e));
        }
    }
    loop {
        let mut redis_conn = match shared_redis_pool.get().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("Redis connection failed: {}. Retrying in 1sec", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        let opts = StreamReadOptions::default()
            .group(&config.group_name, &config.consumer_name)
            .count(config.redispayload_len);

        let stream_result: redis::RedisResult<StreamReadReply> = redis_conn
            .xread_options(&[&config.stream_key], &[">"], &opts)
            .await;

        match stream_result {
            Ok(entries) => {
                if entries.keys.is_empty() {
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    continue;
                }
                let stream_payload = process_redis_stream(entries);
                execute_submissions_detached(
                    stream_payload,
                    limiter.clone(),
                    shared_redis_pool.clone(),
                    config.stream_key.clone(),
                    config.group_name.clone(),
                )
                .await;
            }
            Err(e) => {
                warn!("Failed to read from stream: {}. Retrying...", e);
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }
    }
}
