use deadpool_redis::{Config as D_Congif, CreatePoolError, Pool, Runtime};

#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Pool,
    pub stream_name: String,
}

pub fn redis_connection_pooler(redis_url: &str) -> Result<Pool, CreatePoolError> {
    let client = D_Congif::from_url(redis_url);
    let pool = client.create_pool(Some(Runtime::Tokio1));
    pool
}

