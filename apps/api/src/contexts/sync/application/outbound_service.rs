use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::config::NetboxConfig;
use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::sync::domain::{
    OutboundEvent, OutboundEventCounters, Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE,
    SYNC_PROVIDER_FIELD_KEY_API_TOKEN, SourceOperationalStatus, SourceRuntimeErrorCode,
};
use crate::contexts::sync::infrastructure::{
    OutboundEventRepository, ProviderRepository, SourceRuntimeRepository,
};
use crate::errors::AppError;
use crate::integrations::netbox::{
    NetboxClient, NetboxRepository, NetboxResourceRegistry, NetboxSchemaProvider,
};
use crate::observability::{EventFields, LogLevel, log_event};

/// Worker tuning for [`OutboundService`]: batch size and retry policy.
pub struct OutboundWorkerConfig {
    pub worker_batch_size: usize,
    pub retry_max_attempts: i32,
    pub retry_backoff_ms: i64,
}

pub struct OutboundService<PRepo, EventRepo, RuntimeRepo, SecretRepo, Store>
where
    PRepo: ProviderRepository,
    EventRepo: OutboundEventRepository,
    RuntimeRepo: SourceRuntimeRepository,
    SecretRepo: SecretRepository,
    Store: SecretStore,
{
    providers: Arc<PRepo>,
    outbox: Arc<EventRepo>,
    runtime: Arc<RuntimeRepo>,
    secret_repository: Arc<SecretRepo>,
    secret_store: Arc<Store>,
    worker_batch_size: usize,
    retry_max_attempts: i32,
    retry_backoff_ms: i64,
}

impl<PRepo, EventRepo, RuntimeRepo, SecretRepo, Store>
    OutboundService<PRepo, EventRepo, RuntimeRepo, SecretRepo, Store>
where
    PRepo: ProviderRepository,
    EventRepo: OutboundEventRepository,
    RuntimeRepo: SourceRuntimeRepository,
    SecretRepo: SecretRepository,
    Store: SecretStore,
{
    pub fn new(
        providers: Arc<PRepo>,
        outbox: Arc<EventRepo>,
        runtime: Arc<RuntimeRepo>,
        secret_repository: Arc<SecretRepo>,
        secret_store: Arc<Store>,
        config: OutboundWorkerConfig,
    ) -> Self {
        Self {
            providers,
            outbox,
            runtime,
            secret_repository,
            secret_store,
            worker_batch_size: config.worker_batch_size.max(1),
            retry_max_attempts: config.retry_max_attempts.max(1),
            retry_backoff_ms: config.retry_backoff_ms.max(1),
        }
    }

    pub async fn enqueue(&self, event: &OutboundEvent) -> Result<(), AppError> {
        self.outbox.enqueue(event).await
    }

    pub async fn process_batch(&self, limit: usize) -> Result<usize, AppError> {
        let limit = if limit == 0 {
            self.worker_batch_size
        } else {
            limit.min(self.worker_batch_size)
        };
        let events = self.outbox.list_pending(limit).await?;
        let mut processed = 0usize;

        for event in events {
            processed += 1;
            self.outbox.mark_in_progress(event.id).await?;

            if let Err(error) = self.process_event(&event).await {
                log_event(
                    LogLevel::Error,
                    "sync.outbound.event.process.failed",
                    EventFields::default()
                        .with_operation("outbound_process_event")
                        .with_resource("provider", event.source_id.to_string())
                        .with_context("event_id", event.id.to_string())
                        .with_context("error", error.to_string()),
                );
            }
        }

        Ok(processed)
    }

    pub async fn replay_dead_letter(
        &self,
        source_id: Uuid,
        limit: Option<usize>,
    ) -> Result<usize, AppError> {
        let limit = limit.unwrap_or(self.worker_batch_size).max(1);
        self.outbox.replay_dead_letter(source_id, limit).await
    }

    pub async fn get_counters(&self, source_id: Uuid) -> Result<OutboundEventCounters, AppError> {
        self.outbox.get_counters(source_id).await
    }

    async fn process_event(&self, event: &OutboundEvent) -> Result<(), AppError> {
        if event
            .correlation_id
            .as_deref()
            .is_some_and(|value| value.starts_with("inbound:"))
        {
            self.outbox.mark_succeeded(event.id).await?;
            self.update_runtime_on_success(event.source_id).await?;
            return Ok(());
        }

        let provider = match self.providers.get_by_id(event.source_id).await? {
            Some(provider) => provider,
            None => {
                self.fail_event(event, "Provider not found for outbound event")
                    .await?;
                return Ok(());
            }
        };

        if !provider.is_enabled {
            self.fail_event(event, "Provider disabled for outbound event")
                .await?;
            return Ok(());
        }
        if provider.provider_type != ProviderType::Netbox {
            self.fail_event(event, "Outbound push supports only NetBox providers")
                .await?;
            return Ok(());
        }
        if !provider.allows_remote_writes() {
            self.outbox.mark_succeeded(event.id).await?;
            self.update_runtime_on_success(event.source_id).await?;
            return Ok(());
        }

        match self.push_to_netbox(&provider, event).await {
            Ok(()) => {
                self.outbox.mark_succeeded(event.id).await?;
                self.update_runtime_on_success(event.source_id).await?;
                log_event(
                    LogLevel::Info,
                    "sync.outbound.event.processed",
                    EventFields::default()
                        .with_operation("outbound_process_event")
                        .with_resource("provider", event.source_id.to_string())
                        .with_context("event_id", event.id.to_string())
                        .with_context("resource_type", &event.resource_type)
                        .with_context("operation", &event.operation)
                        .with_context("outcome", "succeeded"),
                );
            }
            Err(error) => {
                self.retry_or_dead_letter(event, &error).await?;
            }
        }

        Ok(())
    }

    async fn push_to_netbox(
        &self,
        provider: &Provider,
        event: &OutboundEvent,
    ) -> Result<(), AppError> {
        let repository = self.build_netbox_repository(provider).await?;
        let resource_key = canonical_resource_key(&event.resource_type);
        let operation = event.operation.to_ascii_lowercase();

        match operation.as_str() {
            "create" => {
                let payload = outbound_payload_for_upsert(&event.payload)?;
                let _ = repository.create(&resource_key, payload).await?;
                Ok(())
            }
            "update" => {
                let payload = outbound_payload_for_upsert(&event.payload)?;
                if let Some(remote_id) =
                    resolve_remote_id(&repository, &resource_key, &event.payload).await?
                {
                    let _ = repository.update(&resource_key, remote_id, payload).await?;
                } else {
                    let _ = repository.create(&resource_key, payload).await?;
                }
                Ok(())
            }
            "delete" => {
                if let Some(remote_id) =
                    resolve_remote_id(&repository, &resource_key, &event.payload).await?
                {
                    repository.delete(&resource_key, remote_id).await?;
                }
                Ok(())
            }
            other => Err(AppError::Validation(format!(
                "Unsupported outbound operation '{}'",
                other
            ))),
        }
    }

    async fn build_netbox_repository(
        &self,
        provider: &Provider,
    ) -> Result<NetboxRepository, AppError> {
        let config = self.resolve_netbox_config(provider).await?;
        let schema_provider = NetboxSchemaProvider::new(config.clone());
        let schema = schema_provider.get_schema().await?;
        let registry = Arc::new(NetboxResourceRegistry::from_schema(schema)?);
        let client = NetboxClient::from_config(&config)?;
        Ok(NetboxRepository::new(client, registry))
    }

    async fn resolve_provider_secret_value(
        &self,
        provider_id: Uuid,
        field_key: &str,
    ) -> Result<Option<String>, AppError> {
        let Some(binding) = self
            .secret_repository
            .get_usage_binding(SYNC_PROVIDER_CONSUMER_TYPE, provider_id, field_key)
            .await?
        else {
            return Ok(None);
        };

        let value = match self.secret_store.get_secret(binding.secret_id).await {
            Ok(value) => value,
            Err(AppError::MissingSecret(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_string()))
    }

    async fn resolve_netbox_config(&self, provider: &Provider) -> Result<NetboxConfig, AppError> {
        let map = provider.config.as_object().ok_or_else(|| {
            AppError::Validation("Provider config must be a JSON object".to_string())
        })?;

        let base_url = map
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::Validation("Provider config base_url is required".to_string())
            })?
            .to_string();
        let api_token = self
            .resolve_provider_secret_value(provider.id, SYNC_PROVIDER_FIELD_KEY_API_TOKEN)
            .await?
            .or_else(|| {
                map.get("api_token")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            })
            .ok_or_else(|| {
                AppError::Validation("Provider config api_token is required".to_string())
            })?;
        let page_size = map
            .get("page_size")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or_else(|| NetboxConfig::default().page_size);

        Ok(NetboxConfig {
            base_url,
            api_token,
            page_size,
        })
    }

    async fn retry_or_dead_letter(
        &self,
        event: &OutboundEvent,
        error: &AppError,
    ) -> Result<(), AppError> {
        let attempts = event.attempts + 1;
        let message = error.to_string();

        if attempts >= self.retry_max_attempts {
            self.outbox
                .mark_failed(event.id, attempts, None, Some(message.as_str()))
                .await?;
            self.update_runtime_on_failure(
                event.source_id,
                SourceRuntimeErrorCode::OutboundRetryExhausted,
                message.as_str(),
            )
            .await?;
            log_event(
                LogLevel::Warning,
                "sync.outbound.event.dead_letter",
                EventFields::default()
                    .with_operation("outbound_process_event")
                    .with_resource("provider", event.source_id.to_string())
                    .with_context("event_id", event.id.to_string())
                    .with_context("resource_type", &event.resource_type)
                    .with_context("operation", &event.operation)
                    .with_context("attempts", attempts.to_string())
                    .with_context("error", message),
            );
            return Ok(());
        }

        let backoff_ms = exponential_backoff_ms(self.retry_backoff_ms, attempts);
        let next_attempt_at = Utc::now() + Duration::milliseconds(backoff_ms);
        self.outbox
            .mark_failed(
                event.id,
                attempts,
                Some(next_attempt_at),
                Some(message.as_str()),
            )
            .await?;
        self.update_runtime_on_failure(
            event.source_id,
            SourceRuntimeErrorCode::OutboundPushFailed,
            message.as_str(),
        )
        .await?;
        log_event(
            LogLevel::Warning,
            "sync.outbound.event.retry_scheduled",
            EventFields::default()
                .with_operation("outbound_process_event")
                .with_resource("provider", event.source_id.to_string())
                .with_context("event_id", event.id.to_string())
                .with_context("resource_type", &event.resource_type)
                .with_context("operation", &event.operation)
                .with_context("attempts", attempts.to_string())
                .with_context("next_attempt_at", next_attempt_at.to_rfc3339())
                .with_context("error", message),
        );
        Ok(())
    }

    async fn fail_event(&self, event: &OutboundEvent, reason: &str) -> Result<(), AppError> {
        let attempts = event.attempts + 1;
        self.outbox
            .mark_failed(event.id, attempts, None, Some(reason))
            .await?;
        self.update_runtime_on_failure(
            event.source_id,
            SourceRuntimeErrorCode::OutboundPushFailed,
            reason,
        )
        .await
    }

    async fn update_runtime_on_success(&self, source_id: Uuid) -> Result<(), AppError> {
        let mut runtime = match self.runtime.get_by_source_id(source_id).await? {
            Some(runtime) => runtime,
            None => return Ok(()),
        };
        if runtime.operational_status == SourceOperationalStatus::Degraded {
            runtime.operational_status = SourceOperationalStatus::Ready;
        }
        runtime.last_error_code = None;
        runtime.last_error_message = None;
        runtime.updated_at = Utc::now();
        self.runtime.upsert(&runtime).await
    }

    async fn update_runtime_on_failure(
        &self,
        source_id: Uuid,
        code: SourceRuntimeErrorCode,
        message: &str,
    ) -> Result<(), AppError> {
        let mut runtime = match self.runtime.get_by_source_id(source_id).await? {
            Some(runtime) => runtime,
            None => return Ok(()),
        };
        runtime.operational_status = SourceOperationalStatus::Degraded;
        runtime.last_error_code = Some(code);
        runtime.last_error_message = Some(message.to_string());
        runtime.updated_at = Utc::now();
        self.runtime.upsert(&runtime).await
    }
}

fn canonical_resource_key(resource_type: &str) -> String {
    match resource_type {
        "dcim.site" => "dcim.sites".to_string(),
        other => other.to_string(),
    }
}

fn outbound_payload_for_upsert(payload: &Value) -> Result<Value, AppError> {
    let mut payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::Validation("Outbound payload must be JSON object".to_string()))?;
    payload.remove("id");
    payload.remove("local_id");
    payload.remove("remote_id");
    payload.remove("previous_slug");
    Ok(Value::Object(payload))
}

async fn resolve_remote_id(
    repository: &NetboxRepository,
    resource_key: &str,
    payload: &Value,
) -> Result<Option<String>, AppError> {
    let map = match payload.as_object() {
        Some(map) => map,
        None => return Ok(None),
    };

    if let Some(remote_id) = map.get("remote_id").and_then(scalar_to_string) {
        return Ok(Some(remote_id));
    }
    if let Some(remote_id) = map.get("id").and_then(Value::as_i64) {
        return Ok(Some(remote_id.to_string()));
    }
    if let Some(remote_id) = map
        .get("id")
        .and_then(Value::as_str)
        .and_then(parse_numeric_string)
    {
        return Ok(Some(remote_id.to_string()));
    }

    for (payload_key, filter_key) in [
        ("slug", "slug"),
        ("previous_slug", "slug"),
        ("name", "name"),
    ] {
        let Some(value) = map.get(payload_key).and_then(Value::as_str) else {
            continue;
        };
        let mut filters = HashMap::new();
        filters.insert(filter_key.to_string(), Value::String(value.to_string()));
        let page = repository
            .list(resource_key, Some(1), Some(0), Some(filters), None)
            .await?;
        if let Some(identifier) = page.results.first().and_then(extract_record_id) {
            return Ok(Some(identifier));
        }
    }

    Ok(None)
}

fn extract_record_id(record: &Value) -> Option<String> {
    record
        .as_object()
        .and_then(|map| map.get("id"))
        .and_then(scalar_to_string)
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_numeric_string(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

fn exponential_backoff_ms(base_ms: i64, attempts: i32) -> i64 {
    let exponent: u32 = attempts.saturating_sub(1).try_into().unwrap_or(0);
    let factor = 2_i64.saturating_pow(exponent.min(8));
    base_ms.saturating_mul(factor).clamp(base_ms, 300_000)
}
