use std::sync::Arc;

use chrono::Utc;
use futures_util::future::BoxFuture;
use openssl::hash::Hasher;
use openssl::{hash::MessageDigest, memcmp, pkey::PKey, sign::Signer};
use serde_json::{Value, json};
use uuid::Uuid;

use super::lock::ProviderLease;
use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::sync::domain::{
    ConflictMode, InboundWebhookEvent, InboundWebhookEventStatus, InventoryCommitAuthorisation,
    InventoryEntity, InventoryIdentity, LocalInventoryCatalogView, MappingConfigDocument,
    MappingConfigurationStatus, MappingDirection, MappingTarget, SourceOperationalStatus,
    SourceRecord, SourceRuntimeErrorCode, live_entity_mapping,
};
use crate::contexts::sync::infrastructure::{
    MappingConfigurationRepository, SourceRuntimeRepository, WebhookEventRepository,
};
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_event};
use redis::aio::ConnectionManager;

/// Reads the authoritative provider record for a live inbound event.
///
/// The event payload is not guaranteed to be the API representation, so
/// created/updated events read the record by id before any local write.
pub trait NetboxRecordReader: Send + Sync {
    fn read_record<'a>(
        &'a self,
        provider_id: Uuid,
        entity: InventoryEntity,
        external_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<SourceRecord>, AppError>>;
}

pub struct WebhookService<RuntimeRepo, EventRepo, Catalog, Mappings, Reader, Audit>
where
    RuntimeRepo: SourceRuntimeRepository,
    EventRepo: WebhookEventRepository,
    Catalog: LocalInventoryCatalogView,
    Mappings: MappingConfigurationRepository,
    Reader: NetboxRecordReader,
    Audit: AuditRecorder,
{
    runtime: Arc<RuntimeRepo>,
    events: Arc<EventRepo>,
    catalog: Arc<Catalog>,
    mappings: Arc<Mappings>,
    reader: Arc<Reader>,
    audit: Arc<Audit>,
    redis: ConnectionManager,
}

impl<RuntimeRepo, EventRepo, Catalog, Mappings, Reader, Audit>
    WebhookService<RuntimeRepo, EventRepo, Catalog, Mappings, Reader, Audit>
where
    RuntimeRepo: SourceRuntimeRepository,
    EventRepo: WebhookEventRepository,
    Catalog: LocalInventoryCatalogView,
    Mappings: MappingConfigurationRepository,
    Reader: NetboxRecordReader,
    Audit: AuditRecorder,
{
    pub fn new(
        runtime: Arc<RuntimeRepo>,
        events: Arc<EventRepo>,
        catalog: Arc<Catalog>,
        mappings: Arc<Mappings>,
        reader: Arc<Reader>,
        audit: Arc<Audit>,
        redis: ConnectionManager,
    ) -> Self {
        Self {
            runtime,
            events,
            catalog,
            mappings,
            reader,
            audit,
            redis,
        }
    }

    pub fn verify_signature(
        &self,
        payload: &[u8],
        signature_header: &str,
        secret: &str,
    ) -> Result<(), AppError> {
        if secret.trim().is_empty() {
            return Err(AppError::Validation(
                "webhook_secret is required for NetBox webhook validation".to_string(),
            ));
        }
        let expected = compute_netbox_signature(payload, secret)?;
        let actual = signature_header.trim().to_ascii_lowercase();
        if expected.len() != actual.len() || !memcmp::eq(expected.as_bytes(), actual.as_bytes()) {
            return Err(AppError::Auth("Invalid webhook signature".to_string()));
        }
        Ok(())
    }

    /// Build an [`InboundWebhookEvent`] from the raw HTTP body bytes and the
    /// already-parsed JSON value.
    ///
    /// `raw_bytes` is hashed with SHA-256 to produce `payload_hash`.  The
    /// parsed `payload` value is stored **in the returned struct only** so that
    /// `apply_event` can act on it during the current request; it is never
    /// written to the database.
    pub fn normalize_payload(
        &self,
        source_id: Uuid,
        raw_bytes: &[u8],
        payload: &Value,
    ) -> Result<InboundWebhookEvent, AppError> {
        normalize_payload(source_id, raw_bytes, payload)
    }

    pub async fn ingest_event(
        &self,
        event: &InboundWebhookEvent,
    ) -> Result<InboundWebhookEventStatus, AppError> {
        // Contention is retryable and must not leave a durable dedup receipt.
        let lease = ProviderLease::acquire(
            self.redis.clone(),
            event.source_id,
            crate::config::constants::SYNC_LOCK_TTL_SECONDS as usize,
        )
        .await?;
        lease
            .run(|authorisation| self.ingest_locked(event, authorisation))
            .await
    }

    async fn ingest_locked(
        &self,
        event: &InboundWebhookEvent,
        authorisation: Arc<dyn InventoryCommitAuthorisation>,
    ) -> Result<InboundWebhookEventStatus, AppError> {
        let receipt = match self
            .events
            .get_by_dedup_key(event.source_id, &event.dedup_key)
            .await?
        {
            Some(receipt) => Some(receipt),
            None => match self.events.create(event).await {
                Ok(()) => None,
                Err(AppError::Conflict(_)) => Some(
                    self.events
                        .get_by_dedup_key(event.source_id, &event.dedup_key)
                        .await?
                        .ok_or_else(|| {
                            AppError::Conflict("Webhook receipt changed during registration".into())
                        })?,
                ),
                Err(error) => return Err(error),
            },
        };
        let mut delivery = event.clone();
        if let Some(receipt) = receipt {
            if receipt.payload_hash != event.payload_hash {
                return Err(AppError::Conflict(
                    "Webhook deduplication key has a different payload".into(),
                ));
            }
            delivery.id = receipt.id;
            delivery.received_at = receipt.received_at;
            if matches!(
                receipt.status,
                InboundWebhookEventStatus::Applied
                    | InboundWebhookEventStatus::Ignored
                    | InboundWebhookEventStatus::Deduplicated
            ) {
                self.update_runtime_state(
                    event.source_id,
                    &InboundWebhookEventStatus::Deduplicated,
                    None,
                    None,
                )
                .await?;
                self.record_audit_event(&delivery, &InboundWebhookEventStatus::Deduplicated, None)
                    .await?;
                log_event(
                    LogLevel::Info,
                    "sync.webhook.inbound.deduplicated",
                    EventFields::default()
                        .with_operation("webhook_deduplicated")
                        .with_resource("provider", event.source_id.to_string())
                        .with_context("event_type", &event.event_type)
                        .with_context("model", &event.model),
                );
                return Ok(InboundWebhookEventStatus::Deduplicated);
            }
            // A journal row alone is not proof of application: lease loss or cancellation
            // can leave accepted/failed work whose inventory transaction rolled back.
            self.events
                .update_status(delivery.id, "accepted", None)
                .await?;
        }
        let event = &delivery;

        self.record_audit_event(event, &InboundWebhookEventStatus::Accepted, None)
            .await?;

        let status = match self.apply_event(event, authorisation).await {
            Ok(status) => status,
            Err(error) => {
                let error_message = error.to_string();
                self.events
                    .update_status(
                        event.id,
                        status_to_string(&InboundWebhookEventStatus::Failed),
                        Some(error_message.as_str()),
                    )
                    .await?;
                self.update_runtime_state(
                    event.source_id,
                    &InboundWebhookEventStatus::Failed,
                    Some(SourceRuntimeErrorCode::WebhookApplyFailed),
                    Some(error_message.as_str()),
                )
                .await?;
                self.record_audit_event(
                    event,
                    &InboundWebhookEventStatus::Failed,
                    Some(error_message.as_str()),
                )
                .await?;
                log_event(
                    LogLevel::Error,
                    "sync.webhook.inbound.failed",
                    EventFields::default()
                        .with_operation("webhook_process")
                        .with_resource("provider", event.source_id.to_string())
                        .with_context("event_type", &event.event_type)
                        .with_context("model", &event.model)
                        .with_context("outcome", "failed")
                        .with_context("error", error_message),
                );
                return Err(error);
            }
        };

        self.events
            .update_status(event.id, status_to_string(&status), None)
            .await?;
        self.update_runtime_state(event.source_id, &status, None, None)
            .await?;
        self.record_audit_event(event, &status, None).await?;
        log_event(
            LogLevel::Info,
            "sync.webhook.inbound.processed",
            EventFields::default()
                .with_operation("webhook_process")
                .with_resource("provider", event.source_id.to_string())
                .with_context("event_type", &event.event_type)
                .with_context("model", &event.model)
                .with_context("outcome", status_to_string(&status)),
        );

        Ok(status)
    }

    pub async fn mark_signature_failure(
        &self,
        source_id: Uuid,
        error: &str,
    ) -> Result<(), AppError> {
        self.update_runtime_state(
            source_id,
            &InboundWebhookEventStatus::Failed,
            Some(SourceRuntimeErrorCode::WebhookInvalidSignature),
            Some(error),
        )
        .await
    }

    async fn update_runtime_state(
        &self,
        source_id: Uuid,
        status: &InboundWebhookEventStatus,
        error_code: Option<SourceRuntimeErrorCode>,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        let mut runtime = match self.runtime.get_by_source_id(source_id).await? {
            Some(runtime) => runtime,
            None => return Ok(()),
        };
        runtime.webhook_last_received_at = Some(Utc::now());
        if *status == InboundWebhookEventStatus::Failed {
            runtime.operational_status = SourceOperationalStatus::Degraded;
            runtime.last_error_code =
                Some(error_code.unwrap_or(SourceRuntimeErrorCode::WebhookApplyFailed));
            runtime.last_error_message = error.map(ToString::to_string);
        } else {
            if runtime.operational_status == SourceOperationalStatus::Degraded {
                runtime.operational_status = SourceOperationalStatus::Ready;
            }
            runtime.last_error_code = None;
            runtime.last_error_message = None;
        }
        runtime.updated_at = Utc::now();
        self.runtime.upsert(&runtime).await
    }

    async fn record_audit_event(
        &self,
        event: &InboundWebhookEvent,
        status: &InboundWebhookEventStatus,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        let audit_event = AuditEvent {
            id: Uuid::now_v7(),
            event_type: format!("sync.webhook.inbound.{}", status_to_string(status)),
            actor_id: None,
            actor_kind: "webhook".to_string(),
            target_type: Some("sync_provider".to_string()),
            target_id: Some(event.source_id),
            metadata: json!({
                "source_id": event.source_id.to_string(),
                "inbound_event_id": event.id.to_string(),
                "dedup_key": event.dedup_key,
                "event_type": event.event_type,
                "model": event.model,
                "request_id": event.request_id,
                "status": status_to_string(status),
                "error": error,
            }),
            created_at: Utc::now(),
        };
        self.audit.record_event(audit_event).await
    }

    async fn apply_event(
        &self,
        event: &InboundWebhookEvent,
        authorisation: Arc<dyn InventoryCommitAuthorisation>,
    ) -> Result<InboundWebhookEventStatus, AppError> {
        if !matches!(event.event_type.as_str(), "created" | "updated" | "deleted") {
            return Ok(InboundWebhookEventStatus::Ignored);
        }
        let Some(config) = self.live_mapping(event.source_id).await? else {
            return Ok(InboundWebhookEventStatus::Ignored);
        };
        let Some((entity, _mapping)) = live_entity_mapping(&config, &event.model) else {
            return Ok(InboundWebhookEventStatus::Ignored);
        };
        if config.conflict_mode != ConflictMode::NetboxWins {
            return Err(AppError::Validation(format!(
                "live inbound events require netbox_wins conflict mode; the active mapping uses {}",
                config.conflict_mode.as_str()
            )));
        }
        let data = event
            .payload
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| AppError::Validation("NetBox webhook requires data".into()))?;
        let identity = InventoryIdentity::netbox(
            event.source_id,
            entity,
            data.get("id").unwrap_or(&Value::Null),
        )?;
        if event.event_type == "deleted" {
            return Ok(
                if self
                    .catalog
                    .delete_live_record(&config, &identity, authorisation)
                    .await?
                {
                    InboundWebhookEventStatus::Applied
                } else {
                    InboundWebhookEventStatus::Ignored
                },
            );
        }
        let record = self
            .reader
            .read_record(event.source_id, entity, &identity.external_id)
            .await?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "NetBox {} {} is not available for live apply",
                    entity.source_key(),
                    identity.external_id
                ))
            })?;
        self.catalog
            .apply_live_record(&config, &record, authorisation)
            .await?;
        Ok(InboundWebhookEventStatus::Applied)
    }

    /// The active bidirectional local-inventory mapping of the provider.
    ///
    /// An event outside such a mapping has no live contract and is ignored;
    /// a pull-only source keeps its existing read-only semantics.
    async fn live_mapping(
        &self,
        provider_id: Uuid,
    ) -> Result<Option<MappingConfigDocument>, AppError> {
        let active = self
            .mappings
            .get_active_for_provider_pair(provider_id, MappingTarget::LocalInventory)
            .await?
            .filter(|config| {
                config.status == MappingConfigurationStatus::Active
                    && config.direction == MappingDirection::Bidirectional
            });
        let Some(active) = active else {
            return Ok(None);
        };
        let document = self
            .mappings
            .get_document(active.logical_id, active.version_id)
            .await?
            .ok_or_else(|| {
                AppError::NotFound("mapping configuration document not found".to_string())
            })?;
        Ok(Some(document))
    }
}

fn normalize_payload(
    source_id: Uuid,
    raw_bytes: &[u8],
    payload: &Value,
) -> Result<InboundWebhookEvent, AppError> {
    let map = payload
        .as_object()
        .ok_or_else(|| AppError::Validation("Webhook payload must be a JSON object".into()))?;
    let event_type = canonical_event_type(
        map.get("event")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation("Webhook payload missing 'event'".into()))?,
    );
    let model = canonical_model(
        map.get("object_type")
            .or_else(|| map.get("model"))
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation("Webhook payload missing 'model'".into()))?,
    );
    let request_id = map
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let resource_id = map
        .get("data")
        .and_then(|data| data.get("id"))
        .map(canonical_json_value)
        .unwrap_or_else(|| "none".into());
    Ok(InboundWebhookEvent {
        id: Uuid::now_v7(),
        source_id,
        dedup_key: format!(
            "{}:{event_type}:{model}:{resource_id}",
            request_id.as_deref().unwrap_or("none")
        ),
        event_type,
        model,
        request_id,
        status: InboundWebhookEventStatus::Accepted,
        payload_hash: sha256_hex(raw_bytes),
        payload: payload.clone(),
        received_at: Utc::now(),
        processed_at: None,
        last_error: None,
    })
}

fn compute_netbox_signature(payload: &[u8], secret: &str) -> Result<String, AppError> {
    let key = PKey::hmac(secret.as_bytes())
        .map_err(|error| AppError::Auth(format!("Invalid webhook secret: {}", error)))?;
    let mut signer = Signer::new(MessageDigest::sha512(), &key).map_err(|error| {
        AppError::Auth(format!(
            "Failed to initialize signature verifier: {}",
            error
        ))
    })?;
    signer
        .update(payload)
        .map_err(|error| AppError::Auth(format!("Failed to process webhook payload: {}", error)))?;
    let signature = signer.sign_to_vec().map_err(|error| {
        AppError::Auth(format!("Failed to compute webhook signature: {}", error))
    })?;
    Ok(signature
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}

fn status_to_string(status: &InboundWebhookEventStatus) -> &'static str {
    match status {
        InboundWebhookEventStatus::Accepted => "accepted",
        InboundWebhookEventStatus::Deduplicated => "deduplicated",
        InboundWebhookEventStatus::Applied => "applied",
        InboundWebhookEventStatus::Ignored => "ignored",
        InboundWebhookEventStatus::Failed => "failed",
    }
}

fn canonical_json_value(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn canonical_model(model: &str) -> String {
    match model.trim() {
        "site" | "dcim.site" => "dcim.site".to_string(),
        other => other.to_string(),
    }
}

/// Compute the SHA-256 hex digest of arbitrary bytes.
/// Used to produce `payload_hash` — the only form in which the raw webhook
/// body is persisted (the raw body itself is NEVER stored).
fn sha256_hex(bytes: &[u8]) -> String {
    let Ok(mut hasher) = Hasher::new(MessageDigest::sha256()) else {
        // Deterministic sentinel: failed to instantiate hasher (should never
        // happen in practice). Using "00" * 32 would look like a real hash,
        // so we use a clearly-invalid sentinel that passes NOT NULL but
        // signals the failure.
        return "sha256_error".to_string();
    };
    if hasher.update(bytes).is_err() {
        return "sha256_error".to_string();
    }
    match hasher.finish() {
        Ok(digest) => digest.iter().map(|byte| format!("{:02x}", byte)).collect(),
        Err(_) => "sha256_error".to_string(),
    }
}

fn canonical_event_type(event_type: &str) -> String {
    match event_type.trim() {
        "created" | "object_created" => "created".to_string(),
        "updated" | "object_updated" => "updated".to_string(),
        "deleted" | "object_deleted" => "deleted".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // normalize_payload must produce a hash and never embed the
    // raw payload bytes in the persisted struct fields.
    // ---------------------------------------------------------------------------

    /// Verify the SHA-256 helper matches a known vector.
    #[test]
    fn sha256_hex_known_vector() {
        // SHA-256("abc") per NIST FIPS 180-4
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// `normalize_payload` must populate `payload_hash` from the raw bytes.
    #[test]
    fn normalize_payload_produces_hash_from_raw_bytes() {
        let raw =
            b"{\"event\":\"created\",\"model\":\"dcim.site\",\"data\":{\"id\":1,\"name\":\"x\"}}";
        let parsed: Value = serde_json::from_slice(raw).expect("fixture payload should parse");
        let source_id = Uuid::now_v7();

        let event = normalize_payload(source_id, raw, &parsed).expect("payload should normalize");

        // Hash must be the SHA-256 of the raw bytes (64 hex chars).
        let expected_hash = sha256_hex(raw);
        assert_eq!(event.payload_hash, expected_hash);
        assert_eq!(
            event.payload_hash.len(),
            64,
            "SHA-256 hex is always 64 chars"
        );

        // The in-memory payload field should be set for apply_event processing.
        assert_eq!(event.payload, parsed);

        // Structural metadata must be extracted correctly.
        assert_eq!(event.source_id, source_id);
        assert_eq!(event.event_type, "created");
        assert_eq!(event.model, "dcim.site");
    }

    /// `normalize_payload` raw bytes produce a hash that differs from
    /// re-serialising the parsed JSON (proving we hash the actual wire bytes).
    #[test]
    fn payload_hash_is_of_raw_bytes_not_re_serialised_json() {
        // JSON with non-canonical whitespace — re-serialising would change the hash.
        let raw_with_space = b"{ \"event\": \"updated\", \"model\": \"dcim.site\", \"data\": {\"id\":2,\"name\":\"y\"} }";
        let parsed: Value =
            serde_json::from_slice(raw_with_space).expect("fixture payload should parse");

        let event = normalize_payload(Uuid::now_v7(), raw_with_space, &parsed)
            .expect("payload should normalize");

        let hash_of_raw = sha256_hex(raw_with_space);
        let hash_of_reserialised = sha256_hex(parsed.to_string().as_bytes());
        assert_eq!(event.payload_hash, hash_of_raw);
        assert_ne!(
            hash_of_raw, hash_of_reserialised,
            "raw-byte hash and re-serialised hash should differ for non-canonical JSON"
        );
    }
}
