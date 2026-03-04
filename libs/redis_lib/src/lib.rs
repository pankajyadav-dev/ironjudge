use deadpool_redis::{Config as D_Congif, CreatePoolError, Pool, Runtime};
use redis::{Client, RedisError, aio::ConnectionManager};

#[derive(Clone)]
pub struct AppState {
    pub redis_manager: ConnectionManager,
    pub stream_name: String,
}

pub async fn redis_connection_manager(redis_url: &str) -> Result<ConnectionManager, RedisError> {
    let client = Client::open(redis_url)?;
    let manager = ConnectionManager::new(client).await?;
    Ok(manager)
}

pub fn redis_connection_pooler(redis_url: &str) -> Result<Pool, CreatePoolError> {
    let client = D_Congif::from_url(redis_url);
    let pool = client.create_pool(Some(Runtime::Tokio1));
    pool
}
