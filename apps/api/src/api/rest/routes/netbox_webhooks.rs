// Hardened inbound NetBox webhook handler with
// journal-based echo dedup (PRIMARY), stamp-based fallback (SECONDARY), and
// hash-window fallback (TERTIARY).
//
// Hardening contract:
// 1. HMAC signature is verified BEFORE any payload logging.
// 2. `payload_hash = SHA-256(canonical_payload)` is computed; raw payload is
//    NOT stored in the journal.
// 3. Journal is probed by `(provider_id, model, entity_identity, payload_hash)`
//    with `state IN ('prepared','sent_unknown','succeeded')` and
//    `created_at < received_at`. If matched -> drop as echo and increment
//    `netbox_webhook_echoes_dropped_total{reason="journal"}`.
//    `sent_unknown` match -> bump journal row to `succeeded` (the webhook
//    IS the post-hoc receipt).
// 4. Stamp-based fallback (`X-Outbound-Request-Id` echoed by NetBox) ->
//    drop as echo with `reason="stamp"`.
// 5. Hash-window fallback (±5s same payload_hash on this provider+model)
//    -> drop as echo with `reason="window"`. Window configurable via
//    `PIPING_SYNC__ECHO_WINDOW_SECONDS` (default 5s; not a primary
//    mechanism — heuristic only).

use axum::{
    Extension, Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use openssl::hash::{Hasher, MessageDigest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::Path;
use crate::api::rest::routes::validation::parse_uuid;
use crate::api::rest::routes::webhook_unauth_limiter::{
    UnauthLimitDecision, WEBHOOK_UNAUTH_LIMITER,
};
use crate::app::AppState;
use crate::contexts::sync::domain::{
    JournalState, ProviderType, SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET,
};
use crate::contexts::sync::infrastructure::ProviderRequestJournalRepository;
use crate::errors::AppError;
use crate::observability::metrics;
use crate::observability::{EventFields, LogLevel, log_event, set_source_id, set_sync_outcome};

/// Default echo time-window for the tertiary hash-window fallback (seconds).
const DEFAULT_ECHO_WINDOW_SECONDS: i64 = 5;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct NetboxWebhookPayload {
    pub event: String,
    pub model: String,
    pub request_id: Option<String>,
    pub timestamp: Option<String>,
    pub data: Value,
    pub snapshots: Option<Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NetboxWebhookAcceptResponse {
    pub accepted: bool,
    pub provider_id: String,
    pub event: String,
    pub model: String,
    pub status: String,
    pub request_id: Option<String>,
}

/// 200 response returned when an inbound webhook is recognised as an echo of
/// our own outbound push and dropped by the dedup machinery.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NetboxWebhookEchoDroppedResponse {
    pub accepted: bool,
    pub echo_dropped: bool,
    /// Dedup mechanism that matched: `journal`, `stamp`, or `window`.
    pub reason: String,
}

/// Receive an inbound NetBox webhook.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/netbox/{provider_id}",
    tag = "webhooks",
    params(
        ("provider_id" = String, Path, description = "Provider id"),
    ),
    request_body = NetboxWebhookPayload,
    responses(
        (status = 202, description = "NetBox webhook accepted", body = NetboxWebhookAcceptResponse),
        (status = 200, description = "NetBox webhook accepted but dropped as an echo", body = NetboxWebhookEchoDroppedResponse),
        (status = 401, description = "NetBox webhook signature rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Provider not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Provider is disabled", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "NetBox webhook request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "NetBox webhook request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn receive_netbox_webhook(
    State(state): State<AppState>,
    Extension(_request_id): Extension<RequestId>,
    _auth_context: Option<Extension<AuthContext>>,
    Path(provider_id): Path<String>,
    headers: HeaderMap,
    payload_bytes: Bytes,
) -> impl IntoResponse {
    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    set_source_id(&provider_id.to_string());

    let provider = match state.services.source_service.get_source(provider_id).await {
        Ok(Some(provider)) => provider,
        Ok(None) => return handle_app_error(AppError::NotFound("Provider not found".to_string())),
        Err(error) => return handle_app_error(error),
    };
    if !provider.is_enabled {
        set_sync_outcome("provider_disabled");
        return handle_app_error(AppError::Conflict("Provider is disabled".to_string()));
    }
    if provider.provider_type != ProviderType::Netbox {
        set_sync_outcome("unsupported_provider_type");
        return handle_app_error(AppError::Validation(
            "Webhook endpoint supports only netbox providers".to_string(),
        ));
    }

    // -----------------------------------------------------------------------
    // STEP 1 — HMAC verification BEFORE any logging.
    // Never log raw payload prior to signature check.
    // -----------------------------------------------------------------------
    let signature = headers
        .get("x-hook-signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if signature.is_empty() {
        set_sync_outcome("missing_signature");
        metrics::record_netbox_inbound_webhook_signature_failure("missing");
        metrics::record_netbox_inbound_webhook("rejected_signature");
        return handle_app_error(AppError::Auth(
            "missing x-hook-signature header".to_string(),
        ));
    }
    let secret = match state
        .services
        .source_service
        .resolve_provider_secret_value(provider.id, SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET)
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => String::new(),
        Err(error) => return handle_app_error(error),
    };

    if let Err(error) =
        state
            .services
            .webhook_service
            .verify_signature(&payload_bytes, signature, &secret)
    {
        // Rate-limit check — BEFORE writing to source_runtime.
        // This prevents a flood of unauthenticated requests from continuously
        // flipping the provider status to Degraded (DoS-driven status flap).
        // Note: this rate limit guards status-flip frequency only; operators
        // should configure a reverse-proxy rate limit for request-level DoS.
        let limit_decision = WEBHOOK_UNAUTH_LIMITER.record_attempt(provider_id).await;
        metrics::record_netbox_webhook_unauth_attempt();

        match limit_decision {
            UnauthLimitDecision::SuppressStatusFlip => {
                // Short window exceeded: log but do NOT flip the provider status.
                log_event(
                    LogLevel::Warning,
                    "sync.webhook.inbound.unauth_rate_limit",
                    EventFields::default()
                        .with_operation("webhook_unauth_limiter")
                        .with_resource("provider", provider_id.to_string())
                        .with_context("decision", "suppress_status_flip")
                        .with_context("reason", "short_window_exceeded"),
                );
            }
            UnauthLimitDecision::Allow | UnauthLimitDecision::ShouldFlipStatus => {
                // Within normal range OR sustained attack past long-window threshold:
                // allow the status flip to proceed so the operator can see it.
                let _ = state
                    .services
                    .webhook_service
                    .mark_signature_failure(provider_id, &error.to_string())
                    .await;
            }
        }

        set_sync_outcome("invalid_signature");
        metrics::record_netbox_inbound_webhook_signature_failure("mismatch");
        metrics::record_netbox_inbound_webhook("rejected_signature");
        log_event(
            LogLevel::Warning,
            "sync.webhook.inbound.rejected",
            EventFields::default()
                .with_operation("webhook_validate_signature")
                .with_resource("provider", provider_id.to_string())
                .with_context("reason", "invalid_signature"),
        );
        return handle_app_error(error);
    }

    // -----------------------------------------------------------------------
    // STEP 2 — Parse + canonical payload hash.
    // -----------------------------------------------------------------------
    let parsed = match parse_webhook_payload(payload_bytes.as_ref()) {
        Ok(parsed) => parsed,
        Err(error) => return handle_app_error(error),
    };
    let received_at = Utc::now();

    // -----------------------------------------------------------------------
    // STEP 3 — Journal probe (PRIMARY).
    // -----------------------------------------------------------------------
    match probe_journal_echo(
        &state,
        provider_id,
        &parsed.canonical_model,
        &parsed.entity_identity,
        &parsed.payload_hash,
        received_at,
    )
    .await
    {
        Ok(Some(response)) => return response,
        Ok(None) => {}
        Err(error) => return handle_app_error(error),
    }

    // -----------------------------------------------------------------------
    // STEP 4 — Stamp-based fallback (SECONDARY).
    // If NetBox echoed back the `X-Outbound-Request-Id` we stamped on push,
    // and the journal has a matching outbound row, drop the webhook.
    // -----------------------------------------------------------------------
    match probe_stamp_echo(&state, &headers, provider_id).await {
        Ok(Some(response)) => return response,
        Ok(None) => {}
        Err(error) => return handle_app_error(error),
    }

    // -----------------------------------------------------------------------
    // STEP 5 — Hash-window fallback (TERTIARY, heuristic).
    // -----------------------------------------------------------------------
    match probe_hash_window_echo(
        &state,
        provider_id,
        &parsed.canonical_model,
        &parsed.payload_hash,
        received_at,
    )
    .await
    {
        Ok(Some(response)) => return response,
        Ok(None) => {}
        Err(error) => return handle_app_error(error),
    }

    // -----------------------------------------------------------------------
    // STEP 6 — Not an echo: process the inbound event normally.
    // -----------------------------------------------------------------------
    match ingest_inbound_event(
        &state,
        provider_id,
        payload_bytes.as_ref(),
        &parsed.payload_value,
        &parsed.typed_payload,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Parsed, hashed pieces of an inbound NetBox webhook (STEP 2).
struct ParsedWebhookPayload {
    payload_value: Value,
    typed_payload: NetboxWebhookPayload,
    canonical_model: String,
    entity_identity: String,
    payload_hash: String,
}

/// STEP 2 — decode the raw body into the typed payload and derive the
/// canonical model, entity identity and raw-bytes SHA-256 hash used by the
/// echo-dedup probes.
fn parse_webhook_payload(payload_bytes: &[u8]) -> Result<ParsedWebhookPayload, AppError> {
    let payload_value: Value =
        serde_json::from_slice(payload_bytes).map_err(AppError::ValidationJson)?;
    let typed_payload: NetboxWebhookPayload =
        serde_json::from_value(payload_value.clone()).map_err(AppError::ValidationJson)?;

    let canonical_model = canonical_model(&typed_payload.model);
    let entity_identity = entity_identity_from_payload(&typed_payload.data);
    let payload_hash = sha256_hex(payload_bytes);

    Ok(ParsedWebhookPayload {
        payload_value,
        typed_payload,
        canonical_model,
        entity_identity,
        payload_hash,
    })
}

/// STEP 3 — primary echo dedup: probe the outbound request journal for a
/// matching `(provider, model, entity, payload_hash)` row. A `sent_unknown`
/// match is promoted to `succeeded` because the webhook is the post-hoc
/// receipt. Returns the drop response when an echo is recognised.
async fn probe_journal_echo(
    state: &AppState,
    provider_id: Uuid,
    canonical_model: &str,
    entity_identity: &str,
    payload_hash: &str,
    received_at: chrono::DateTime<Utc>,
) -> Result<Option<axum::response::Response>, AppError> {
    let journal_repo = &state.repositories.sync_provider_request_journal;
    let probe_started = std::time::Instant::now();
    let primary_probe = journal_repo
        .probe_for_echo_dedup(
            provider_id,
            canonical_model,
            entity_identity,
            payload_hash,
            received_at,
        )
        .await;
    metrics::observe_netbox_journal_probe_duration(
        "primary",
        probe_started.elapsed().as_secs_f64(),
    );
    match primary_probe {
        Ok(Some(journal_match)) => {
            // sent_unknown match: webhook is the post-hoc receipt; promote
            // the journal row to `succeeded`.
            if journal_match.state == JournalState::SentUnknown {
                let _ = journal_repo
                    .transition_to_succeeded(journal_match.journal_id, None, Some(200))
                    .await;
            }
            set_sync_outcome("echo_dropped_journal");
            metrics::record_netbox_webhook_echo_dropped("journal");
            metrics::record_netbox_inbound_webhook("echo_dropped");
            log_event(
                LogLevel::Info,
                "sync.webhook.inbound.echo_dropped",
                EventFields::default()
                    .with_operation("webhook_echo_dedup")
                    .with_resource("provider", provider_id.to_string())
                    .with_context("reason", "journal")
                    .with_context("model", canonical_model)
                    .with_context("entity_identity", entity_identity)
                    .with_context("journal_state", journal_match.state.as_str()),
            );
            Ok(Some(
                (
                    StatusCode::OK,
                    Json(json!({
                        "accepted": true,
                        "echo_dropped": true,
                        "reason": "journal",
                    })),
                )
                    .into_response(),
            ))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    }
}

/// STEP 4 — secondary echo dedup: NetBox echoes the `X-Outbound-Request-Id`
/// we stamped on push. Confirm the outbound row exists in the journal before
/// dropping the webhook.
async fn probe_stamp_echo(
    state: &AppState,
    headers: &HeaderMap,
    provider_id: Uuid,
) -> Result<Option<axum::response::Response>, AppError> {
    let journal_repo = &state.repositories.sync_provider_request_journal;
    let stamped_request_id = headers
        .get("x-outbound-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(stamp) = stamped_request_id.as_deref()
        && let Ok(stamp_uuid) = Uuid::parse_str(stamp)
    {
        // Reuse the journal probe by hashing — we still need a journal
        // row to confirm the outbound was OURS. The probe variant
        // `probe_for_echo_dedup` already covers this when the stamped
        // event_uuid lands as `outbound_event_uuid`. Here we explicitly
        // look up `max_attempt_no` to confirm the outbound exists.
        let stamp_started = std::time::Instant::now();
        let stamp_lookup = journal_repo.max_attempt_no(provider_id, stamp_uuid).await;
        metrics::observe_netbox_journal_probe_duration(
            "stamp",
            stamp_started.elapsed().as_secs_f64(),
        );
        if let Ok(max_attempt) = stamp_lookup
            && max_attempt > 0
        {
            set_sync_outcome("echo_dropped_stamp");
            metrics::record_netbox_webhook_echo_dropped("stamp");
            metrics::record_netbox_inbound_webhook("echo_dropped");
            log_event(
                LogLevel::Info,
                "sync.webhook.inbound.echo_dropped",
                EventFields::default()
                    .with_operation("webhook_echo_dedup")
                    .with_resource("provider", provider_id.to_string())
                    .with_context("reason", "stamp"),
            );
            return Ok(Some(
                (
                    StatusCode::OK,
                    Json(json!({
                        "accepted": true,
                        "echo_dropped": true,
                        "reason": "stamp",
                    })),
                )
                    .into_response(),
            ));
        }
    }
    Ok(None)
}

/// STEP 5 — tertiary, heuristic echo dedup: a matching `payload_hash` for the
/// same provider+model within a configurable ±window is treated as an echo.
async fn probe_hash_window_echo(
    state: &AppState,
    provider_id: Uuid,
    canonical_model: &str,
    payload_hash: &str,
    received_at: chrono::DateTime<Utc>,
) -> Result<Option<axum::response::Response>, AppError> {
    let journal_repo = &state.repositories.sync_provider_request_journal;
    let window_seconds = std::env::var("PIPING_SYNC__ECHO_WINDOW_SECONDS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_ECHO_WINDOW_SECONDS);
    let window_started = std::time::Instant::now();
    let window_probe = journal_repo
        .probe_by_hash_window(
            provider_id,
            canonical_model,
            payload_hash,
            received_at,
            window_seconds,
        )
        .await;
    metrics::observe_netbox_journal_probe_duration(
        "hash_window",
        window_started.elapsed().as_secs_f64(),
    );
    match window_probe {
        Ok(Some(_)) => {
            set_sync_outcome("echo_dropped_window");
            metrics::record_netbox_webhook_echo_dropped("window");
            metrics::record_netbox_inbound_webhook("echo_dropped");
            log_event(
                LogLevel::Info,
                "sync.webhook.inbound.echo_dropped",
                EventFields::default()
                    .with_operation("webhook_echo_dedup")
                    .with_resource("provider", provider_id.to_string())
                    .with_context("reason", "window")
                    .with_context("window_seconds", window_seconds.to_string()),
            );
            Ok(Some(
                (
                    StatusCode::OK,
                    Json(json!({
                        "accepted": true,
                        "echo_dropped": true,
                        "reason": "window",
                    })),
                )
                    .into_response(),
            ))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    }
}

/// STEP 6 — not an echo: normalise the inbound payload and ingest the event,
/// returning the 202 acceptance response.
async fn ingest_inbound_event(
    state: &AppState,
    provider_id: Uuid,
    payload_bytes: &[u8],
    payload_value: &Value,
    typed_payload: &NetboxWebhookPayload,
) -> Result<axum::response::Response, AppError> {
    let event = match state.services.webhook_service.normalize_payload(
        provider_id,
        payload_bytes,
        payload_value,
    ) {
        Ok(value) => value,
        Err(error) => return Err(error),
    };
    let ingest_status = match state.services.webhook_service.ingest_event(&event).await {
        Ok(status) => status,
        Err(error) => return Err(error),
    };
    set_sync_outcome(&format!("{:?}", ingest_status).to_lowercase());
    metrics::record_netbox_inbound_webhook("accepted");
    log_event(
        LogLevel::Info,
        "sync.webhook.inbound.accepted",
        EventFields::default()
            .with_operation("webhook_ingest")
            .with_resource("provider", provider_id.to_string())
            .with_context("event_type", &event.event_type)
            .with_context("model", &event.model)
            .with_context("outcome", format!("{:?}", ingest_status).to_lowercase()),
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(NetboxWebhookAcceptResponse {
            accepted: true,
            provider_id: provider_id.to_string(),
            event: typed_payload.event.clone(),
            model: typed_payload.model.clone(),
            status: format!("{:?}", ingest_status).to_lowercase(),
            request_id: typed_payload.request_id.clone(),
        }),
    )
        .into_response())
}

/// SHA-256 hex of arbitrary bytes (the raw HTTP body of a webhook).
/// We deliberately hash the RAW BYTES (not a re-serialized canonical JSON),
/// so journal matches require byte-for-byte payload equality. NetBox's
/// outbound payloads are deterministic for any given object state.
fn sha256_hex(bytes: &[u8]) -> String {
    let Ok(mut hasher) = Hasher::new(MessageDigest::sha256()) else {
        return String::from("00").repeat(32);
    };
    if hasher.update(bytes).is_err() {
        return String::from("00").repeat(32);
    }
    match hasher.finish() {
        Ok(digest) => digest.iter().map(|byte| format!("{:02x}", byte)).collect(),
        Err(_) => String::from("00").repeat(32),
    }
}

/// Canonicalize NetBox model identifier (`site`, `dcim.site`,
/// `object_type=dcim.site`, etc.) into the form used by the journal.
fn canonical_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.contains('.') {
        return trimmed.to_string();
    }
    // Bare model names (`site`, `device`) -> default DCIM family.
    format!("dcim.{}", trimmed)
}

/// Extract the entity identity (NetBox int id stringified) from the payload's
/// `data.id` field. Falls back to slug or "unknown".
fn entity_identity_from_payload(data: &Value) -> String {
    if let Some(id) = data.get("id") {
        match id {
            Value::Number(n) => return n.to_string(),
            Value::String(s) if !s.is_empty() => return s.clone(),
            _ => {}
        }
    }
    if let Some(Value::String(slug)) = data.get("slug")
        && !slug.is_empty()
    {
        return slug.clone();
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_hex_known_vector() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn canonical_model_keeps_dotted() {
        assert_eq!(canonical_model("dcim.site"), "dcim.site");
        assert_eq!(canonical_model("ipam.prefix"), "ipam.prefix");
    }

    #[test]
    fn canonical_model_namespaces_bare() {
        assert_eq!(canonical_model("site"), "dcim.site");
    }

    #[test]
    fn entity_identity_prefers_int_id() {
        let data = json!({"id": 42, "slug": "alpha"});
        assert_eq!(entity_identity_from_payload(&data), "42");
    }

    #[test]
    fn entity_identity_falls_back_to_slug() {
        let data = json!({"slug": "alpha"});
        assert_eq!(entity_identity_from_payload(&data), "alpha");
    }

    #[test]
    fn entity_identity_unknown_when_missing() {
        let data = json!({});
        assert_eq!(entity_identity_from_payload(&data), "unknown");
    }
}
