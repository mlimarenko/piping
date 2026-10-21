use std::sync::Arc;

use redis::aio::ConnectionManager;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::NetboxConfig;
use crate::config::constants::LONG_CACHE_TTL_SECONDS;
use crate::errors::AppError;
use crate::integrations::netbox::NetboxClient;
use crate::observability::{instrument_span, operation_span};
use crate::storage::cache::redis::cache;

const SCHEMA_CACHE_KEY_PREFIX: &str = "netbox:schema";

pub struct NetboxSchemaProvider {
    netbox_config: NetboxConfig,
    cache: Option<Arc<Mutex<ConnectionManager>>>,
    cache_ttl_seconds: u64,
}

impl NetboxSchemaProvider {
    pub fn new(netbox_config: NetboxConfig) -> Self {
        Self {
            netbox_config,
            cache: None,
            cache_ttl_seconds: LONG_CACHE_TTL_SECONDS,
        }
    }

    pub fn new_with_cache(
        netbox_config: NetboxConfig,
        cache: Arc<Mutex<ConnectionManager>>,
    ) -> Self {
        Self {
            netbox_config,
            cache: Some(cache),
            cache_ttl_seconds: LONG_CACHE_TTL_SECONDS,
        }
    }

    pub async fn get_schema(&self) -> Result<Value, AppError> {
        let span = operation_span(
            "netbox.schema.get",
            &[("base_url", self.netbox_config.base_url.clone())],
        );
        instrument_span(span, async {
            if let Some(cache) = &self.cache
                && let Some(schema) = self.get_cached_schema(cache).await?
            {
                crate::observability::log_event(
                    crate::observability::LogLevel::Info,
                    "integration.netbox.schema.cache_hit",
                    crate::observability::EventFields::default()
                        .with_operation("netbox.schema.get")
                        .with_resource("integration_call", "netbox.schema")
                        .with_context("provider", "netbox")
                        .with_context("base_url", self.netbox_config.base_url.clone()),
                );
                return Ok(schema);
            }

            let client = NetboxClient::from_config(&self.netbox_config)?;
            let schema = client.get_schema().await?;

            if let Some(cache) = &self.cache
                && let Err(error) = self.set_cached_schema(cache, &schema).await
            {
                crate::observability::log_error_event(
                    "integration.netbox.schema.cache_write.failed",
                    "cache_write_error",
                    error.to_string(),
                    crate::observability::EventFields::default()
                        .with_operation("netbox.schema.cache_write")
                        .with_resource("integration_call", "netbox.schema")
                        .with_context("provider", "netbox")
                        .with_context("base_url", self.netbox_config.base_url.clone()),
                );
            }

            Ok(schema)
        })
        .await
    }

    pub async fn refresh_schema(&self) -> Result<Value, AppError> {
        let span = operation_span(
            "netbox.schema.refresh",
            &[("base_url", self.netbox_config.base_url.clone())],
        );
        instrument_span(span, async {
            if let Some(cache) = &self.cache {
                self.invalidate_cached_schema(cache).await?;
            }

            let client = NetboxClient::from_config(&self.netbox_config)?;
            let schema = client.get_schema().await?;

            if let Some(cache) = &self.cache
                && let Err(error) = self.set_cached_schema(cache, &schema).await
            {
                crate::observability::log_error_event(
                    "integration.netbox.schema.cache_write.failed",
                    "cache_write_error",
                    error.to_string(),
                    crate::observability::EventFields::default()
                        .with_operation("netbox.schema.cache_write")
                        .with_resource("integration_call", "netbox.schema")
                        .with_context("provider", "netbox")
                        .with_context("base_url", self.netbox_config.base_url.clone()),
                );
            }

            Ok(schema)
        })
        .await
    }

    pub async fn invalidate_cache(&self) -> Result<(), AppError> {
        if let Some(cache) = &self.cache {
            self.invalidate_cached_schema(cache).await?;
        }
        Ok(())
    }

    fn build_cache_key(&self) -> String {
        format!(
            "{}:{}",
            SCHEMA_CACHE_KEY_PREFIX, self.netbox_config.base_url
        )
    }

    async fn get_cached_schema(
        &self,
        cache: &Arc<Mutex<ConnectionManager>>,
    ) -> Result<Option<Value>, AppError> {
        let cache_key = self.build_cache_key();
        let mut conn = cache.lock().await;
        let cached = cache::get(&mut conn, &cache_key).await?;
        match cached {
            Some(raw) => match serde_json::from_str(&raw) {
                Ok(parsed) => Ok(Some(parsed)),
                Err(error) => {
                    crate::observability::log_error_event(
                        "integration.netbox.schema.cache_payload.invalid",
                        "cache_payload_error",
                        error.to_string(),
                        crate::observability::EventFields::default()
                            .with_operation("netbox.schema.cache_read")
                            .with_resource("integration_call", "netbox.schema")
                            .with_context("provider", "netbox")
                            .with_context("base_url", self.netbox_config.base_url.clone())
                            .with_context("cache_key", cache_key.clone()),
                    );
                    cache::delete(&mut conn, &cache_key).await?;
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    async fn set_cached_schema(
        &self,
        cache: &Arc<Mutex<ConnectionManager>>,
        schema: &Value,
    ) -> Result<(), AppError> {
        let cache_key = self.build_cache_key();
        let raw = serde_json::to_string(schema)
            .map_err(|e| AppError::Validation(format!("Invalid cache payload: {}", e)))?;
        let mut conn = cache.lock().await;
        cache::set(&mut conn, &cache_key, &raw, Some(self.cache_ttl_seconds)).await
    }

    async fn invalidate_cached_schema(
        &self,
        cache: &Arc<Mutex<ConnectionManager>>,
    ) -> Result<(), AppError> {
        let cache_key = self.build_cache_key();
        let mut conn = cache.lock().await;
        cache::delete(&mut conn, &cache_key).await
    }
}
