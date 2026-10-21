use std::sync::Arc;

use chrono::Utc;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::config::NetboxConfig;
use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::ssrf::check_ssrf_and_pin;
use crate::contexts::sync::domain::{
    Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE, SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
    SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET, SourceBootstrapStatus, SourceOperationalStatus,
    SourceRuntime, SourceRuntimeErrorCode, parse_custom_provider_config,
};
use crate::contexts::sync::infrastructure::{
    OutboundEventRepository, ProviderListFilter, ProviderRepository, SourceRuntimeRepository,
};
use crate::errors::AppError;
use crate::integrations::netbox::NetboxClient;

/// Runtime wiring for [`SourceService`]: callback base URL and outbox batch size.
pub struct SourceRuntimeConfig {
    pub callback_base_url: String,
    pub outbox_worker_batch_size: usize,
}

pub struct SourceService<PRepo, RuntimeRepo, OutboxRepo, SecretRepo, Store>
where
    PRepo: ProviderRepository,
    RuntimeRepo: SourceRuntimeRepository,
    OutboxRepo: OutboundEventRepository,
    SecretRepo: SecretRepository,
    Store: SecretStore,
{
    providers: Arc<PRepo>,
    runtime: Arc<RuntimeRepo>,
    outbox: Arc<OutboxRepo>,
    callback_base_url: String,
    secret_repository: Arc<SecretRepo>,
    secret_store: Arc<Store>,
    outbox_worker_batch_size: usize,
}

impl<PRepo, RuntimeRepo, OutboxRepo, SecretRepo, Store>
    SourceService<PRepo, RuntimeRepo, OutboxRepo, SecretRepo, Store>
where
    PRepo: ProviderRepository,
    RuntimeRepo: SourceRuntimeRepository,
    OutboxRepo: OutboundEventRepository,
    SecretRepo: SecretRepository,
    Store: SecretStore,
{
    pub fn new(
        providers: Arc<PRepo>,
        runtime: Arc<RuntimeRepo>,
        outbox: Arc<OutboxRepo>,
        secret_repository: Arc<SecretRepo>,
        secret_store: Arc<Store>,
        config: SourceRuntimeConfig,
    ) -> Self {
        Self {
            providers,
            runtime,
            outbox,
            callback_base_url: config.callback_base_url,
            secret_repository,
            secret_store,
            outbox_worker_batch_size: config.outbox_worker_batch_size.max(1),
        }
    }

    pub async fn list_sources(&self) -> Result<Vec<Provider>, AppError> {
        self.providers.list().await
    }

    pub async fn list_sources_paginated(
        &self,
        limit: usize,
        offset: usize,
        sort: Option<SortSpec>,
        filter: ProviderListFilter,
    ) -> Result<Vec<Provider>, AppError> {
        self.providers
            .list_paginated(limit, offset, sort, filter)
            .await
    }

    pub async fn count_sources(&self, filter: ProviderListFilter) -> Result<usize, AppError> {
        self.providers.count(filter).await
    }

    pub async fn get_source(&self, source_id: Uuid) -> Result<Option<Provider>, AppError> {
        self.providers.get_by_id(source_id).await
    }

    pub async fn create_source(&self, source: Provider) -> Result<Provider, AppError> {
        validate_provider(&source)?;
        let created = self.providers.create(source).await?;
        self.ensure_runtime_initialized(&created).await?;
        Ok(created)
    }

    pub async fn update_source(&self, source: Provider) -> Result<Provider, AppError> {
        validate_provider(&source)?;
        let updated = self.providers.update(source).await?;
        self.ensure_runtime_initialized(&updated).await?;
        Ok(updated)
    }

    pub async fn delete_source(&self, source_id: Uuid) -> Result<(), AppError> {
        if let Some(source) = self.providers.get_by_id(source_id).await?
            && let Some(runtime) = self.runtime.get_by_source_id(source_id).await?
            && let Some(webhook_id) = runtime.external_webhook_id
            && source.allows_remote_writes()
            && let Ok(config) = self.resolve_netbox_config_for_provider(&source).await
            && let Ok(client) = NetboxClient::from_config(&config)
        {
            let _ = client.delete_webhook(webhook_id).await;
            let event_rule_name = format!("piping-source-{}-events", source.id);
            if let Ok(event_rules) = client.list_event_rules_by_name(&event_rule_name).await {
                for event_rule in event_rules {
                    if let Some(event_rule_id) = extract_webhook_id(&event_rule) {
                        let _ = client.delete_event_rule(event_rule_id).await;
                    }
                }
            }
        }
        self.providers.delete(source_id).await
    }

    pub async fn bootstrap_source(&self, source_id: Uuid) -> Result<SourceRuntime, AppError> {
        let source = self
            .providers
            .get_by_id(source_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))?;
        validate_provider(&source)?;

        self.runtime
            .upsert(&SourceRuntime {
                source_id,
                bootstrap_status: SourceBootstrapStatus::Running,
                operational_status: if source.is_enabled {
                    SourceOperationalStatus::Ready
                } else {
                    SourceOperationalStatus::Disabled
                },
                external_webhook_id: None,
                webhook_last_received_at: None,
                last_error_code: None,
                last_error_message: None,
                updated_at: Utc::now(),
            })
            .await?;

        // Read-only and custom sources have no remote webhook to provision.
        if !source.allows_remote_writes() {
            let runtime = SourceRuntime {
                source_id,
                bootstrap_status: SourceBootstrapStatus::Ready,
                operational_status: if source.is_enabled {
                    SourceOperationalStatus::Ready
                } else {
                    SourceOperationalStatus::Disabled
                },
                external_webhook_id: None,
                webhook_last_received_at: None,
                last_error_code: None,
                last_error_message: None,
                updated_at: Utc::now(),
            };
            self.runtime.upsert(&runtime).await?;
            return Ok(runtime);
        }

        match self.perform_webhook_bootstrap(&source).await {
            Ok(webhook_id) => {
                let runtime = SourceRuntime {
                    source_id,
                    bootstrap_status: SourceBootstrapStatus::Ready,
                    operational_status: if source.is_enabled {
                        SourceOperationalStatus::Ready
                    } else {
                        SourceOperationalStatus::Disabled
                    },
                    external_webhook_id: Some(webhook_id),
                    webhook_last_received_at: None,
                    last_error_code: None,
                    last_error_message: None,
                    updated_at: Utc::now(),
                };
                self.runtime.upsert(&runtime).await?;
                Ok(runtime)
            }
            Err(error) => {
                let runtime = SourceRuntime {
                    source_id,
                    bootstrap_status: SourceBootstrapStatus::Failed,
                    operational_status: SourceOperationalStatus::BootstrapFailed,
                    external_webhook_id: None,
                    webhook_last_received_at: None,
                    last_error_code: Some(classify_bootstrap_error(&error)),
                    last_error_message: Some(error.to_string()),
                    updated_at: Utc::now(),
                };
                self.runtime.upsert(&runtime).await?;
                Err(error)
            }
        }
    }

    pub async fn get_runtime_status(
        &self,
        source_id: Uuid,
    ) -> Result<Option<SourceRuntime>, AppError> {
        self.runtime.get_by_source_id(source_id).await
    }

    pub async fn validate_source_connection(&self, source_id: Uuid) -> Result<Provider, AppError> {
        let source = self
            .providers
            .get_by_id(source_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))?;
        validate_provider(&source)?;
        match source.provider_type {
            ProviderType::Netbox => {
                let config = self.resolve_netbox_config_for_provider(&source).await?;
                NetboxClient::from_config(&config)?
                    .validate_connection()
                    .await?;
                Ok(source)
            }
            ProviderType::Custom => {
                let cfg =
                    parse_custom_provider_config(&source.config).map_err(AppError::Validation)?;

                // SSRF guard: resolve DNS, verify all IPs are public, and pin the
                // resolved addresses to prevent DNS rebinding (TOCTOU mitigation).
                let (pinned_host, pinned_addrs) = check_ssrf_and_pin(&cfg.base_url).await?;

                let ssl_verify = cfg.ssl_verify.unwrap_or(true);
                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(!ssl_verify)
                    // No redirect following — a 3xx is treated as "reachable" (status 200..400).
                    // Redirect-following without per-hop SSRF checks would bypass the guard above.
                    .redirect(reqwest::redirect::Policy::none())
                    .resolve_to_addrs(&pinned_host, &pinned_addrs)
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .map_err(|e| AppError::Validation(format!("Failed to build HTTP client: {e}")))?;

                let auth_header_name = cfg
                    .auth_header
                    .as_deref()
                    .unwrap_or("Authorization")
                    .to_string();

                let mut req = client.head(&cfg.base_url);
                if let Some(token) = &cfg.api_token {
                    req = req.header(&auth_header_name, format!("Bearer {token}"));
                }

                let response = req.send().await.map_err(|e| {
                    AppError::Validation(format!("Custom provider probe failed: {e}"))
                })?;

                let status = response.status().as_u16();
                if (200..400).contains(&status) {
                    Ok(source)
                } else {
                    Err(AppError::Validation(format!(
                        "Custom provider returned unexpected status: {status}"
                    )))
                }
            }
        }
    }

    pub async fn replay_outbox(
        &self,
        source_id: Uuid,
        limit: Option<usize>,
    ) -> Result<usize, AppError> {
        // Enforce 404 on unknown provider (contract expectation).
        if self.providers.get_by_id(source_id).await?.is_none() {
            return Err(AppError::NotFound("Provider not found".to_string()));
        }

        let limit = limit.unwrap_or(self.outbox_worker_batch_size).max(1);
        self.outbox.replay_dead_letter(source_id, limit).await
    }

    pub async fn resolve_provider_secret_value(
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

    pub async fn resolve_netbox_config_for_provider(
        &self,
        provider: &Provider,
    ) -> Result<NetboxConfig, AppError> {
        if provider.provider_type != ProviderType::Netbox {
            return Err(AppError::Validation(format!(
                "Provider type '{:?}' is not supported for sync",
                provider.provider_type
            )));
        }

        let map = provider.config.as_object().ok_or_else(|| {
            AppError::Validation("Provider config must be a JSON object".to_string())
        })?;

        let base_url = match map.get("base_url") {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(_) => {
                return Err(AppError::Validation(
                    "Provider config base_url must be a string".to_string(),
                ));
            }
            None => String::new(),
        };
        if base_url.is_empty() {
            return Err(AppError::Validation(
                "Provider config base_url is required for NetBox provider".to_string(),
            ));
        }

        let page_size = match map.get("page_size") {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|&v| v > 0)
                .ok_or_else(|| {
                    AppError::Validation(
                        "Provider config page_size must be a positive integer".to_string(),
                    )
                })?,
            Some(_) => {
                return Err(AppError::Validation(
                    "Provider config page_size must be a number".to_string(),
                ));
            }
            None => NetboxConfig::default().page_size,
        };

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
            .unwrap_or_default();
        if api_token.is_empty() {
            return Err(AppError::Validation(
                "Provider config api_token is required for NetBox provider".to_string(),
            ));
        }

        Ok(NetboxConfig {
            base_url,
            api_token,
            page_size,
        })
    }

    async fn ensure_runtime_initialized(&self, source: &Provider) -> Result<(), AppError> {
        if self.runtime.get_by_source_id(source.id).await?.is_some() {
            return Ok(());
        }
        let runtime = SourceRuntime {
            source_id: source.id,
            bootstrap_status: SourceBootstrapStatus::Pending,
            operational_status: if source.is_enabled {
                SourceOperationalStatus::Ready
            } else {
                SourceOperationalStatus::Disabled
            },
            external_webhook_id: None,
            webhook_last_received_at: None,
            last_error_code: None,
            last_error_message: None,
            updated_at: Utc::now(),
        };
        self.runtime.upsert(&runtime).await
    }

    async fn perform_webhook_bootstrap(&self, source: &Provider) -> Result<String, AppError> {
        if !source.allows_remote_writes() {
            return Err(AppError::Validation(
                "Webhook provisioning requires a bidirectional source".into(),
            ));
        }
        if source.provider_type != ProviderType::Netbox {
            return Err(AppError::Validation(
                "Webhook bootstrap is supported only for netbox providers".to_string(),
            ));
        }

        let config = self.resolve_netbox_config_for_provider(source).await?;
        let client = NetboxClient::from_config(&config)?;
        let webhook_name = format!("piping-source-{}", source.id);
        let callback_url = format!(
            "{}/api/v1/webhooks/netbox/{}",
            self.callback_base_url.trim_end_matches('/'),
            source.id
        );
        let webhook_secret = self
            .resolve_provider_secret_value(source.id, SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET)
            .await?;
        let payload = build_webhook_payload(
            &source.config,
            &webhook_name,
            &callback_url,
            webhook_secret.as_deref(),
        );

        let existing = client.list_webhooks_by_name(&webhook_name).await?;
        let webhook = if let Some(existing) = existing.into_iter().next() {
            let existing_id = extract_webhook_id(&existing)
                .ok_or_else(|| AppError::Provider("NetBox webhook id is missing".to_string()))?;
            client.update_webhook(existing_id, &payload).await?
        } else {
            client.create_webhook(&payload).await?
        };
        let webhook_id = extract_webhook_id(&webhook)
            .map(|value| value.to_string())
            .ok_or_else(|| {
                AppError::Provider("NetBox bootstrap response missing webhook id".to_string())
            })?;
        let event_rule_name = format!("piping-source-{}-events", source.id);
        let event_rule_object_types =
            resolve_event_rule_object_types(&client, &source.config).await?;
        let event_rule_payload = build_event_rule_payload(
            &source.config,
            &event_rule_name,
            &webhook_id,
            &event_rule_object_types,
        );
        let existing_event_rules = client.list_event_rules_by_name(&event_rule_name).await?;
        if let Some(existing_event_rule) = existing_event_rules.into_iter().next() {
            let existing_event_rule_id = extract_webhook_id(&existing_event_rule)
                .ok_or_else(|| AppError::Provider("NetBox event rule id is missing".to_string()))?;
            let _ = client
                .update_event_rule(existing_event_rule_id, &event_rule_payload)
                .await?;
        } else {
            let _ = client.create_event_rule(&event_rule_payload).await?;
        }

        Ok(webhook_id)
    }
}

fn validate_provider(source: &Provider) -> Result<(), AppError> {
    if source.name.trim().is_empty() {
        return Err(AppError::Validation(
            "Provider name cannot be empty".to_string(),
        ));
    }
    match source.provider_type {
        ProviderType::Netbox => {
            validate_netbox_config_shape(&source.config)?;
            validate_netbox_sync_mode(&source.config)?;
        }
        ProviderType::Custom => {
            parse_custom_provider_config(&source.config).map_err(AppError::Validation)?;
        }
    }
    Ok(())
}

fn validate_netbox_config_shape(config: &Value) -> Result<(), AppError> {
    let map = config
        .as_object()
        .ok_or_else(|| AppError::Validation("Provider config must be a JSON object".to_string()))?;

    match map.get("base_url") {
        Some(Value::String(value)) if !value.trim().is_empty() => {}
        Some(Value::String(_)) | None => {
            return Err(AppError::Validation(
                "Provider config base_url is required for NetBox provider".to_string(),
            ));
        }
        Some(_) => {
            return Err(AppError::Validation(
                "Provider config base_url must be a string".to_string(),
            ));
        }
    }

    if let Some(page_size) = map.get("page_size") {
        match page_size {
            Value::Number(value)
                if value
                    .as_u64()
                    .and_then(|raw| usize::try_from(raw).ok())
                    .filter(|raw| *raw > 0)
                    .is_some() => {}
            Value::Number(_) => {
                return Err(AppError::Validation(
                    "Provider config page_size must be a positive integer".to_string(),
                ));
            }
            _ => {
                return Err(AppError::Validation(
                    "Provider config page_size must be a number".to_string(),
                ));
            }
        }
    }

    Ok(())
}

fn validate_netbox_sync_mode(config: &Value) -> Result<(), AppError> {
    let map = config
        .as_object()
        .ok_or_else(|| AppError::Validation("Provider config must be a JSON object".to_string()))?;
    if let Some(sync_mode) = map.get("sync_mode") {
        let sync_mode = sync_mode.as_str().ok_or_else(|| {
            AppError::Validation("Provider config sync_mode must be a string".to_string())
        })?;
        if sync_mode != "pull_only" && sync_mode != "bidirectional" {
            return Err(AppError::Validation(
                "Provider config sync_mode must be one of: pull_only, bidirectional".to_string(),
            ));
        }
    }
    Ok(())
}

fn classify_bootstrap_error(error: &AppError) -> SourceRuntimeErrorCode {
    match error {
        AppError::Validation(_) => SourceRuntimeErrorCode::BootstrapInvalidConfig,
        _ => SourceRuntimeErrorCode::BootstrapRequestFailed,
    }
}

fn build_webhook_payload(
    config: &Value,
    name: &str,
    callback_url: &str,
    webhook_secret: Option<&str>,
) -> Value {
    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(name.to_string()));
    payload.insert(
        "payload_url".to_string(),
        Value::String(callback_url.to_string()),
    );
    payload.insert("http_method".to_string(), Value::String("POST".to_string()));
    payload.insert(
        "http_content_type".to_string(),
        Value::String("application/json".to_string()),
    );
    if let Some(secret) = webhook_secret
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload.insert("secret".to_string(), Value::String(secret.to_string()));
    } else if let Some(map) = config.as_object()
        && let Some(secret) = map.get("webhook_secret").and_then(Value::as_str)
        && !secret.trim().is_empty()
    {
        payload.insert("secret".to_string(), Value::String(secret.to_string()));
    }

    if let Some(map) = config.as_object() {
        // Older NetBox versions may still use webhook-level events/object_types.
        if let Some(events) = map.get("webhook_events").and_then(Value::as_array) {
            payload.insert("events".to_string(), Value::Array(events.clone()));
        } else {
            payload.insert(
                "events".to_string(),
                json!(["created", "updated", "deleted"]),
            );
        }

        if let Some(object_types) = map.get("webhook_object_types").and_then(Value::as_array) {
            payload.insert(
                "object_types".to_string(),
                Value::Array(object_types.clone()),
            );
        }
    }

    Value::Object(payload)
}

fn build_event_rule_payload(
    config: &Value,
    name: &str,
    webhook_id: &str,
    object_types: &[String],
) -> Value {
    let mut payload = Map::new();
    payload.insert("name".to_string(), Value::String(name.to_string()));
    payload.insert("enabled".to_string(), Value::Bool(true));
    payload.insert(
        "event_types".to_string(),
        Value::Array(
            extract_webhook_events(config)
                .iter()
                .map(|event| Value::String(to_event_rule_event(event).to_string()))
                .collect(),
        ),
    );
    payload.insert(
        "object_types".to_string(),
        Value::Array(
            object_types
                .iter()
                .map(|value| Value::String(value.clone()))
                .collect(),
        ),
    );
    payload.insert(
        "action_type".to_string(),
        Value::String("webhook".to_string()),
    );
    payload.insert(
        "action_object_type".to_string(),
        Value::String("extras.webhook".to_string()),
    );

    if let Ok(webhook_id) = webhook_id.parse::<i64>() {
        payload.insert("action_object_id".to_string(), json!(webhook_id));
    } else {
        payload.insert(
            "action_object_id".to_string(),
            Value::String(webhook_id.to_string()),
        );
    }

    Value::Object(payload)
}

fn extract_webhook_events(config: &Value) -> Vec<String> {
    let Some(map) = config.as_object() else {
        return vec![
            "created".to_string(),
            "updated".to_string(),
            "deleted".to_string(),
        ];
    };

    let Some(events) = map.get("webhook_events").and_then(Value::as_array) else {
        return vec![
            "created".to_string(),
            "updated".to_string(),
            "deleted".to_string(),
        ];
    };

    let mut normalized = events
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|event| !event.is_empty())
        .map(|event| event.to_string())
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        normalized = vec![
            "created".to_string(),
            "updated".to_string(),
            "deleted".to_string(),
        ];
    }
    normalized
}

fn to_event_rule_event(event: &str) -> &str {
    match event {
        "created" | "object_created" => "object_created",
        "updated" | "object_updated" => "object_updated",
        "deleted" | "object_deleted" => "object_deleted",
        other => other,
    }
}

fn extract_event_rule_object_type_values(config: &Value) -> Vec<Value> {
    let Some(map) = config.as_object() else {
        return vec![Value::String("dcim.site".to_string())];
    };

    let Some(object_types) = map.get("webhook_object_types").and_then(Value::as_array) else {
        return vec![Value::String("dcim.site".to_string())];
    };

    let normalized = object_types
        .iter()
        .filter_map(|value| match value {
            Value::String(value) if !value.trim().is_empty() => {
                Some(Value::String(value.trim().to_string()))
            }
            Value::Number(value) => Some(Value::Number(value.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        vec![Value::String("dcim.site".to_string())]
    } else {
        normalized
    }
}

async fn resolve_event_rule_object_types(
    client: &NetboxClient,
    config: &Value,
) -> Result<Vec<String>, AppError> {
    let raw = extract_event_rule_object_type_values(config);
    let mut resolved = Vec::new();
    for item in raw {
        match item {
            Value::String(value) => {
                let value = value.trim();
                if !value.is_empty() {
                    resolved.push(value.to_string());
                }
            }
            Value::Number(value) => {
                let object_type_id = value.as_i64().ok_or_else(|| {
                    AppError::Validation("webhook_object_types number must fit in i64".to_string())
                })?;
                let object_type = client.get_object_type_key(object_type_id).await?;
                resolved.push(object_type);
            }
            _ => {}
        }
    }
    if resolved.is_empty() {
        resolved.push("dcim.site".to_string());
    }
    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}

fn extract_webhook_id(value: &Value) -> Option<String> {
    let map = value.as_object()?;
    match map.get("id") {
        Some(Value::String(id)) => Some(id.clone()),
        Some(Value::Number(id)) => Some(id.to_string()),
        _ => None,
    }
}
