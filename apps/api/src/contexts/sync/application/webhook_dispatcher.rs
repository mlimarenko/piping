//! Outbound webhook delivery on domain events.
//!
//! The dispatcher is the fan-out target of the audit sink: for every recorded
//! [`AuditEvent`] it finds enabled subscriptions matching the event's
//! `event_type`, evaluates each subscription's optional condition set against
//! the event JSON, and delivers a signed POST to matching targets.
//!
//! Delivery is best-effort: it runs in a spawned task off the audit/request
//! path, applies an SSRF guard + bounded retries, and records the final outcome
//! via the repository. A failure here never propagates back to audit recording.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::audit::application::AuditEventFanout;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::shared::ssrf::check_ssrf;
use crate::contexts::sync::application::webhook_conditions::ConditionSet;
use crate::contexts::sync::application::webhook_subscription_service::{
    compute_hmac_sha256, ssrf_safe_client,
};
use crate::contexts::sync::infrastructure::persistence::repositories::webhook_subscription_repository::{
    SqlxWebhookSubscriptionRepository, UpdateWebhookFired, WebhookSubscription,
};
use crate::observability::{EventFields, log_error_event};

const DELIVERY_TIMEOUT_SECS: u64 = 10;
const MAX_ATTEMPTS: u32 = 3;
const BASE_BACKOFF_MS: u64 = 200;

const DISPATCH_EVENT: &str = "sync.webhook.dispatch";
const DELIVERY_EVENT: &str = "sync.webhook.delivery";

/// Delivers outbound webhooks for domain events.
#[derive(Clone)]
pub struct WebhookDispatcher {
    repository: SqlxWebhookSubscriptionRepository,
}

impl WebhookDispatcher {
    #[must_use]
    pub fn new(repository: SqlxWebhookSubscriptionRepository) -> Self {
        Self { repository }
    }

    /// Best-effort fan-out for a single domain event.
    ///
    /// Fetches enabled subscriptions matching `event.event_type`, filters by
    /// each subscription's condition set, and delivers to matching targets.
    /// All errors are logged, never returned, so audit recording is never
    /// affected.
    async fn dispatch(&self, event: &AuditEvent) {
        let subscriptions = match self
            .repository
            .list_enabled_for_event_type(&event.event_type)
            .await
        {
            Ok(subs) => subs,
            Err(error) => {
                log_error_event(
                    DISPATCH_EVENT,
                    "webhook_subscription_lookup_failed",
                    error.to_string(),
                    EventFields::default().with_operation(event.event_type.clone()),
                );
                return;
            }
        };
        if subscriptions.is_empty() {
            return;
        }

        let event_json = match serde_json::to_value(event) {
            Ok(value) => value,
            Err(error) => {
                log_error_event(
                    DISPATCH_EVENT,
                    "webhook_event_serialize_failed",
                    error.to_string(),
                    EventFields::default().with_operation(event.event_type.clone()),
                );
                return;
            }
        };

        for subscription in subscriptions {
            if !condition_matches(&subscription, &event_json) {
                continue;
            }
            let repository = self.repository.clone();
            let payload = event_json.clone();
            tokio::spawn(async move {
                deliver_with_retries(&repository, subscription, payload).await;
            });
        }
    }
}

#[async_trait]
impl AuditEventFanout for WebhookDispatcher {
    async fn on_event(&self, event: &AuditEvent) {
        self.dispatch(event).await;
    }
}

/// Evaluate a subscription's condition set against the event JSON.
/// No conditions (`None`) always matches. Malformed stored conditions are
/// treated as "do not match" and logged (validation should have rejected them
/// on write, so this is defensive only).
fn condition_matches(subscription: &WebhookSubscription, event_json: &Value) -> bool {
    let Some(raw) = &subscription.conditions else {
        return true;
    };
    match ConditionSet::parse(raw) {
        Ok(set) => set.evaluate(event_json),
        Err(error) => {
            log_error_event(
                DISPATCH_EVENT,
                "webhook_conditions_invalid",
                error.to_string(),
                EventFields::default().with_resource("webhook", subscription.id.to_string()),
            );
            false
        }
    }
}

async fn deliver_with_retries(
    repository: &SqlxWebhookSubscriptionRepository,
    subscription: WebhookSubscription,
    event_json: Value,
) {
    // SSRF guard before any network call; block private/loopback/link-local.
    if let Err(error) = check_ssrf(&subscription.target_url).await {
        record_outcome(repository, subscription.id, 0, Some(error.to_string())).await;
        return;
    }

    let body_str = match serde_json::to_string(&event_json) {
        Ok(s) => s,
        Err(error) => {
            record_outcome(repository, subscription.id, 0, Some(error.to_string())).await;
            return;
        }
    };

    let client = match ssrf_safe_client(DELIVERY_TIMEOUT_SECS) {
        Ok(client) => client,
        Err(error) => {
            record_outcome(repository, subscription.id, 0, Some(error.to_string())).await;
            return;
        }
    };
    let mut last_status: u16 = 0;
    let mut last_error: Option<String> = Some("not attempted".to_string());

    for attempt in 1..=MAX_ATTEMPTS {
        let (status, error) = deliver_once(&client, &subscription, &body_str).await;
        last_status = status;
        last_error = error;

        if last_error.is_none() {
            break;
        }
        if attempt < MAX_ATTEMPTS {
            // Linear-ish backoff: BASE * attempt.
            tokio::time::sleep(Duration::from_millis(BASE_BACKOFF_MS * u64::from(attempt))).await;
        }
    }

    if let Some(err) = &last_error {
        log_error_event(
            DELIVERY_EVENT,
            "webhook_delivery_failed",
            err.clone(),
            EventFields::default()
                .with_resource("webhook", subscription.id.to_string())
                .with_status(last_status.to_string()),
        );
    }

    record_outcome(repository, subscription.id, last_status, last_error).await;
}

/// Perform a single signed POST. Returns `(http_status, error)` where a `None`
/// error means a 2xx response.
async fn deliver_once(
    client: &reqwest::Client,
    subscription: &WebhookSubscription,
    body_str: &str,
) -> (u16, Option<String>) {
    let timestamp_unix = Utc::now().timestamp();
    let nonce = Uuid::now_v7();
    let mut request = client
        .post(&subscription.target_url)
        .header("Content-Type", "application/json")
        .header("X-Piping-Timestamp", timestamp_unix.to_string())
        .header("X-Piping-Nonce", nonce.to_string())
        .body(body_str.as_bytes().to_vec());

    if let Some(secret) = &subscription.secret {
        // Signing string format: "<unix_seconds>.<body>" (matches test-fire).
        let signing_string = format!("{timestamp_unix}.{body_str}");
        let signature = compute_hmac_sha256(secret, signing_string.as_bytes());
        request = request.header("X-Piping-Signature", format!("sha256={signature}"));
    }

    match request.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                (status, None)
            } else {
                (status, Some(format!("HTTP {status}")))
            }
        }
        Err(error) => (0, Some(error.to_string())),
    }
}

async fn record_outcome(
    repository: &SqlxWebhookSubscriptionRepository,
    id: Uuid,
    status: u16,
    error: Option<String>,
) {
    if let Err(err) = repository
        .update_fired(
            id,
            &UpdateWebhookFired {
                last_status: i32::from(status),
                last_error: error,
                last_fired_at: Utc::now(),
            },
        )
        .await
    {
        log_error_event(
            DELIVERY_EVENT,
            "webhook_outcome_persist_failed",
            err.to_string(),
            EventFields::default().with_resource("webhook", id.to_string()),
        );
    }
}
