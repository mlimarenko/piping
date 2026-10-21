use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::storage::cache::redis::cache;

const MCP_CAPTURE_KEY_PREFIX: &str = "mcp:capture:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCaptureBlob {
    pub capture_id: String,
    pub target_id: String,
    pub session_id: String,
    pub connection_id: String,
    pub owner_api_token_id: String,
    pub owner_user_id: Option<String>,
    pub format: String,
    pub mime_type: String,
    pub width: u16,
    pub height: u16,
    pub bytes: usize,
    pub image_base64: String,
    pub created_at: String,
    pub expires_at: String,
}

pub async fn put(
    conn: &mut ConnectionManager,
    blob: &McpCaptureBlob,
    ttl_seconds: u64,
) -> Result<(), AppError> {
    let value = serde_json::to_string(blob)?;
    cache::set(
        conn,
        &capture_key(&blob.capture_id),
        &value,
        Some(ttl_seconds),
    )
    .await
}

pub async fn get(
    conn: &mut ConnectionManager,
    capture_id: &str,
) -> Result<Option<McpCaptureBlob>, AppError> {
    let Some(value) = cache::get(conn, &capture_key(capture_id)).await? else {
        return Ok(None);
    };
    let blob = serde_json::from_str(&value)?;
    Ok(Some(blob))
}

fn capture_key(capture_id: &str) -> String {
    format!("{MCP_CAPTURE_KEY_PREFIX}{capture_id}")
}
