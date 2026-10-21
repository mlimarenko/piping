use std::sync::Arc;

use chrono::Utc;
use openssl::hash::Hasher;
use openssl::{hash::MessageDigest, memcmp, pkey::PKey, sign::Signer};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::inventory::domain::Site;
use crate::contexts::inventory::infrastructure::persistence::SiteRepository;
use crate::contexts::sync::domain::{
    InboundWebhookEvent, InboundWebhookEventStatus, SourceOperationalStatus, SourceRuntimeErrorCode,
};
use crate::contexts::sync::infrastructure::{SourceRuntimeRepository, WebhookEventRepository};
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_event};

pub struct WebhookService<RuntimeRepo, EventRepo, SiteRepo, Audit>
where
    RuntimeRepo: SourceRuntimeRepository,
    EventRepo: WebhookEventRepository,
    SiteRepo: SiteRepository,
    Audit: AuditRecorder,
{
    runtime: Arc<RuntimeRepo>,
    events: Arc<EventRepo>,
    sites: Arc<SiteRepo>,
    audit: Arc<Audit>,
}

impl<RuntimeRepo, EventRepo, SiteRepo, Audit>
    WebhookService<RuntimeRepo, EventRepo, SiteRepo, Audit>
where
    RuntimeRepo: SourceRuntimeRepository,
    EventRepo: WebhookEventRepository,
    SiteRepo: SiteRepository,
    Audit: AuditRecorder,
{
    pub fn new(
        runtime: Arc<RuntimeRepo>,
        events: Arc<EventRepo>,
        sites: Arc<SiteRepo>,
        audit: Arc<Audit>,
    ) -> Self {
        Self {
            runtime,
            events,
            sites,
            audit,
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
        let map = payload.as_object().ok_or_else(|| {
            AppError::Validation("Webhook payload must be a JSON object".to_string())
        })?;
        let event_type_raw = map
            .get("event")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation("Webhook payload missing 'event'".to_string()))?
            .to_string();
        let model_raw = map
            .get("object_type")
            .and_then(Value::as_str)
            .or_else(|| map.get("model").and_then(Value::as_str))
            .ok_or_else(|| AppError::Validation("Webhook payload missing 'model'".to_string()))?
            .to_string();
        let event_type = canonical_event_type(&event_type_raw);
        let model = canonical_model(&model_raw);
        let request_id = map
            .get("request_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let resource_id = map
            .get("data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("id"))
            .map(canonical_json_value)
            .unwrap_or_else(|| "none".to_string());
        let dedup_key = format!(
            "{}:{}:{}:{}",
            request_id.clone().unwrap_or_else(|| "none".to_string()),
            event_type,
            model,
            resource_id
        );
        let payload_hash = sha256_hex(raw_bytes);

        Ok(InboundWebhookEvent {
            id: Uuid::now_v7(),
            source_id,
            dedup_key,
            event_type,
            model,
            request_id,
            status: InboundWebhookEventStatus::Accepted,
            payload_hash,
            // In-memory only — NOT persisted. See field doc on InboundWebhookEvent.
            payload: payload.clone(),
            received_at: Utc::now(),
            processed_at: None,
            last_error: None,
        })
    }

    pub async fn ingest_event(
        &self,
        event: &InboundWebhookEvent,
    ) -> Result<InboundWebhookEventStatus, AppError> {
        if self
            .events
            .get_by_dedup_key(event.source_id, &event.dedup_key)
            .await?
            .is_some()
        {
            self.update_runtime_state(
                event.source_id,
                &InboundWebhookEventStatus::Deduplicated,
                None,
                None,
            )
            .await?;
            self.record_audit_event(event, &InboundWebhookEventStatus::Deduplicated, None)
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

        match self.events.create(event).await {
            Ok(()) => {}
            Err(AppError::Conflict(_)) => {
                self.update_runtime_state(
                    event.source_id,
                    &InboundWebhookEventStatus::Deduplicated,
                    None,
                    None,
                )
                .await?;
                self.record_audit_event(event, &InboundWebhookEventStatus::Deduplicated, None)
                    .await?;
                return Ok(InboundWebhookEventStatus::Deduplicated);
            }
            Err(error) => return Err(error),
        }

        self.record_audit_event(event, &InboundWebhookEventStatus::Accepted, None)
            .await?;

        let status = match self.apply_event(event).await {
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
    ) -> Result<InboundWebhookEventStatus, AppError> {
        if event.model != "dcim.site" {
            return Ok(InboundWebhookEventStatus::Ignored);
        }

        let map = match event.payload.as_object() {
            Some(map) => map,
            None => return Ok(InboundWebhookEventStatus::Ignored),
        };
        let data = match map.get("data").and_then(Value::as_object) {
            Some(data) => data,
            None => return Ok(InboundWebhookEventStatus::Ignored),
        };

        let event_type = event.event_type.as_str();
        let site_id = match resolve_site_id(self.sites.as_ref(), data, event_type).await? {
            Some(id) => id,
            None => return Ok(InboundWebhookEventStatus::Ignored),
        };
        if event_type == "deleted" {
            if self.sites.get_by_id(site_id).await?.is_some() {
                self.sites.delete(site_id).await?;
                return Ok(InboundWebhookEventStatus::Applied);
            }
            return Ok(InboundWebhookEventStatus::Ignored);
        }

        let name = data
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let Some(name) = name else {
            return Ok(InboundWebhookEventStatus::Ignored);
        };

        if let Some(mut site) = self.sites.get_by_id(site_id).await? {
            site.name = name.clone();
            site.slug = data
                .get("slug")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| Some(slugify(&name)));
            self.sites.update(&site).await?;
            return Ok(InboundWebhookEventStatus::Applied);
        }

        if event_type == "created" {
            let site = Site {
                id: site_id,
                external_id: None,
                name: name.clone(),
                slug: data
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .or_else(|| Some(slugify(&name))),
                status: Some("active".to_string()),
                tenant_id: None,
                region_id: None,
                site_group_id: None,
                facility: None,
                time_zone: None,
                description: None,
                physical_address: None,
                shipping_address: None,
                latitude: None,
                longitude: None,
                comments: None,
                asns: vec![],
                tags: vec![],
                custom_fields: None,
                piping_synced_at: None,
            };
            self.sites.create(&site).await?;
            return Ok(InboundWebhookEventStatus::Applied);
        }

        Ok(InboundWebhookEventStatus::Ignored)
    }
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

fn extract_uuid_from_data(data: &serde_json::Map<String, Value>, key: &str) -> Option<Uuid> {
    match data.get(key) {
        Some(Value::String(value)) => Uuid::parse_str(value).ok(),
        _ => None,
    }
}

async fn resolve_site_id<Repo: SiteRepository>(
    sites: &Repo,
    data: &serde_json::Map<String, Value>,
    event_type: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Some(local_id) = extract_uuid_from_data(data, "local_id") {
        return Ok(Some(local_id));
    }
    if let Some(local_id) = extract_uuid_from_data(data, "id") {
        return Ok(Some(local_id));
    }

    let slug = data
        .get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(existing) = find_site_by_slug_or_name(sites, slug, name).await? {
        return Ok(Some(existing.id));
    }

    // For events sourced from NetBox numeric IDs, derive a stable local UUID.
    // This keeps updates/deletes correlated for remote-born entities.
    if let Some(remote_id) = extract_remote_id(data) {
        return Ok(Some(remote_site_uuid(&remote_id)));
    }

    if event_type == "created" {
        return Ok(Some(Uuid::now_v7()));
    }

    Ok(None)
}

async fn find_site_by_slug_or_name<Repo: SiteRepository>(
    sites: &Repo,
    slug: Option<&str>,
    name: Option<&str>,
) -> Result<Option<Site>, AppError> {
    const PAGE_SIZE: u32 = 256;
    let mut offset = 0u32;

    loop {
        let page = sites.list_paginated(Some(PAGE_SIZE), Some(offset)).await?;
        if page.is_empty() {
            return Ok(None);
        }
        let page_len = page.len();

        if let Some(found) = page.into_iter().find(|site| {
            let slug_match = slug
                .zip(site.slug.as_deref())
                .is_some_and(|(expected, actual)| actual.eq_ignore_ascii_case(expected));
            let name_match = name.is_some_and(|expected| site.name.eq_ignore_ascii_case(expected));
            slug_match || name_match
        }) {
            return Ok(Some(found));
        }

        if PAGE_SIZE as usize > 0 && page_len < PAGE_SIZE as usize {
            return Ok(None);
        }
        offset = offset.saturating_add(PAGE_SIZE);
    }
}

fn extract_remote_id(data: &serde_json::Map<String, Value>) -> Option<String> {
    match data.get("id") {
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn remote_site_uuid(remote_id: &str) -> Uuid {
    let seed = format!("netbox:dcim.site:{remote_id}");
    Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
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

fn slugify(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .replace(|ch: char| !ch.is_ascii_alphanumeric(), "-")
        .trim_matches('-')
        .to_string()
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

        // We only need the pure `normalize_payload` logic — no repos involved.
        // Build a minimal service using the NoopXxx stubs below.
        let svc = make_noop_service();
        let event = svc
            .normalize_payload(source_id, raw, &parsed)
            .expect("payload should normalize");

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

        let svc = make_noop_service();
        let event = svc
            .normalize_payload(Uuid::now_v7(), raw_with_space, &parsed)
            .expect("payload should normalize");

        let hash_of_raw = sha256_hex(raw_with_space);
        let hash_of_reserialised = sha256_hex(parsed.to_string().as_bytes());
        assert_eq!(event.payload_hash, hash_of_raw);
        assert_ne!(
            hash_of_raw, hash_of_reserialised,
            "raw-byte hash and re-serialised hash should differ for non-canonical JSON"
        );
    }

    // ---------------------------------------------------------------------------
    // Minimal no-op stubs — enough to construct WebhookService for unit tests.
    // No database, no I/O.
    // ---------------------------------------------------------------------------

    struct NoopRuntimeRepo;
    struct NoopEventRepo;
    struct NoopSiteRepo;
    struct NoopAudit;

    impl SourceRuntimeRepository for NoopRuntimeRepo {
        async fn get_by_source_id(
            &self,
            _: Uuid,
        ) -> Result<Option<crate::contexts::sync::domain::SourceRuntime>, AppError> {
            Ok(None)
        }
        async fn upsert(
            &self,
            _: &crate::contexts::sync::domain::SourceRuntime,
        ) -> Result<(), AppError> {
            Ok(())
        }
    }

    impl WebhookEventRepository for NoopEventRepo {
        async fn get_by_dedup_key(
            &self,
            _: Uuid,
            _: &str,
        ) -> Result<Option<InboundWebhookEvent>, AppError> {
            Ok(None)
        }
        async fn create(&self, _: &InboundWebhookEvent) -> Result<(), AppError> {
            Ok(())
        }
        async fn update_status(&self, _: Uuid, _: &str, _: Option<&str>) -> Result<(), AppError> {
            Ok(())
        }
        async fn prune_older_than(&self, _: i64) -> Result<u64, AppError> {
            Ok(0)
        }
    }

    impl SiteRepository for NoopSiteRepo {
        async fn list_paginated(
            &self,
            _: Option<u32>,
            _: Option<u32>,
        ) -> Result<Vec<Site>, AppError> {
            Ok(vec![])
        }
        async fn list_hydrated(
            &self,
            _: crate::contexts::inventory::infrastructure::persistence::SiteFilter,
            _: Option<u32>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<(Vec<crate::contexts::inventory::domain::SiteListView>, i64), AppError>
        {
            Ok((vec![], 0))
        }
        async fn get_by_id(&self, _: Uuid) -> Result<Option<Site>, AppError> {
            Ok(None)
        }
        async fn create(&self, _: &Site) -> Result<(), AppError> {
            Ok(())
        }
        async fn update(&self, _: &Site) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete(&self, _: Uuid) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl AuditRecorder for NoopAudit {
        async fn record_event(&self, _: AuditEvent) -> Result<(), AppError> {
            Ok(())
        }
    }

    fn make_noop_service() -> WebhookService<NoopRuntimeRepo, NoopEventRepo, NoopSiteRepo, NoopAudit>
    {
        WebhookService::new(
            Arc::new(NoopRuntimeRepo),
            Arc::new(NoopEventRepo),
            Arc::new(NoopSiteRepo),
            Arc::new(NoopAudit),
        )
    }
}
