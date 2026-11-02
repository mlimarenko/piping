use redis::Client;
use redis::aio::ConnectionManager;

use crate::errors::AppError;

/// Builds a Redis client from connection URL
pub fn build_client(redis_url: &str) -> Result<Client, AppError> {
    Client::open(redis_url).map_err(Into::into)
}

/// Builds a Redis connection manager from connection URL
pub async fn build_connection_manager(redis_url: &str) -> Result<ConnectionManager, AppError> {
    let client = build_client(redis_url)?;
    client.get_connection_manager().await.map_err(Into::into)
}
