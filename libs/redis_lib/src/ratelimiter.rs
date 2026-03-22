use deadpool_redis::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn check_sliding_window_rate_limit(
    conn: &mut Connection,
    client_id: &str,
    limit: i64,
    window_size_seconds: u64,
    lua_script: &redis::Script,
) -> Result<bool, redis::RedisError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
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
