use redis::aio::ConnectionManager;
use serde_json::Value;
use uuid::Uuid;

use crate::config::constants::HOST_CONFIG_CACHE_TTL_SECONDS;
use crate::errors::AppError;
use crate::storage::cache::redis::cache;

/// Cache for resolved host configuration (non-sensitive only)
pub struct HostConfigCache {
    pub key_prefix: String,
    pub version_key: String,
    pub ttl_seconds: u64,
}

impl HostConfigCache {
    pub fn new() -> Self {
        Self {
            key_prefix: "host_config".to_string(),
            version_key: "host_config:version".to_string(),
            ttl_seconds: HOST_CONFIG_CACHE_TTL_SECONDS,
        }
    }

    pub async fn get_with_context(
        &self,
        conn: &mut ConnectionManager,
        host_id: Uuid,
        context: &str,
    ) -> Result<Option<Value>, AppError> {
        let generation = self.load_generation(conn).await?;
        let key = self.build_cache_key(host_id, generation, context);
        let value = cache::get(conn, &key).await?;
        match value {
            Some(raw) => match serde_json::from_str(&raw) {
                Ok(parsed) => Ok(Some(parsed)),
                Err(_) => {
                    self.invalidate_key(conn, &key).await?;
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    pub async fn set_with_context(
        &self,
        conn: &mut ConnectionManager,
        host_id: Uuid,
        context: &str,
        value: &Value,
    ) -> Result<(), AppError> {
        let generation = self.load_generation(conn).await?;
        let key = self.build_cache_key(host_id, generation, context);
        let raw = serde_json::to_string(value)
            .map_err(|e| AppError::Validation(format!("Invalid cache payload: {}", e)))?;
        cache::set(conn, &key, &raw, Some(self.ttl_seconds)).await
    }

    pub async fn invalidate_with_context(
        &self,
        conn: &mut ConnectionManager,
        host_id: Uuid,
        context: &str,
    ) -> Result<(), AppError> {
        let generation = self.load_generation(conn).await?;
        let key = self.build_cache_key(host_id, generation, context);
        self.invalidate_key(conn, &key).await
    }

    pub async fn bump_generation(&self, conn: &mut ConnectionManager) -> Result<u64, AppError> {
        cache::increment(conn, &self.version_key, 1).await
    }

    fn build_cache_key(&self, host_id: Uuid, generation: u64, context: &str) -> String {
        format!(
            "{}:v{}:{}:{}",
            self.key_prefix, generation, context, host_id
        )
    }

    async fn load_generation(&self, conn: &mut ConnectionManager) -> Result<u64, AppError> {
        let value = cache::get(conn, &self.version_key).await?;
        let generation = decode_generation(&self.version_key, value.as_deref());
        if generation.is_err() {
            cache::delete(conn, &self.version_key).await?;
        }
        generation
    }

    async fn invalidate_key(
        &self,
        conn: &mut ConnectionManager,
        key: &str,
    ) -> Result<(), AppError> {
        cache::delete(conn, key).await
    }
}

impl Default for HostConfigCache {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_generation(key: &str, raw: Option<&str>) -> Result<u64, AppError> {
    match raw {
        Some(raw) => raw.parse::<u64>().map_err(|error| {
            AppError::Internal(format!(
                "Invalid host config cache generation in `{key}`: {error}"
            ))
        }),
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::decode_generation;

    #[test]
    fn missing_generation_is_zero_but_corrupt_generation_is_an_error() {
        assert_eq!(
            decode_generation("host-config-version", None).expect("missing key starts at zero"),
            0
        );
        assert!(
            decode_generation("host-config-version", Some("not-a-generation")).is_err(),
            "a corrupt generation must not select the authoritative v0 host config cache"
        );
    }
}
