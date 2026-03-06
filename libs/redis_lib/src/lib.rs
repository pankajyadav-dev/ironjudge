use deadpool_redis::{Config as DConfig, CreatePoolError, Pool, Runtime};

#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Pool,
    pub stream_name: String,
}

/// Creates a deadpool-redis connection pool.
///
/// `max_size` controls the maximum number of connections in the pool.
/// Passing `None` uses deadpool's default (currently 16).
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

/// Verifies the pool is healthy by issuing a PING.
/// Call this at startup to fail-fast on bad config.
pub async fn ping_redis(pool: &Pool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = pool.get().await?;
    let pong: String = redis::cmd("PING").query_async(&mut *conn).await?;
    if pong != "PONG" {
        return Err(format!("Unexpected PING response: {}", pong).into());
    }
    Ok(())
}
