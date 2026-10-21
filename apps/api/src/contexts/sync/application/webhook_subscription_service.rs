use std::sync::OnceLock;
use std::time::Instant;

use chrono::Utc;
use hmac::{Hmac, Mac};
use regex::Regex;
use sha2::Sha256;
use uuid::Uuid;

const TEST_FIRE_TIMEOUT_SECS: u64 = 10;
const SECRET_MIN_LEN: usize = 8;
const SECRET_MAX_LEN: usize = 512;

static EVENT_TYPE_REGEX: OnceLock<Regex> = OnceLock::new();

fn event_type_regex() -> &'static Regex {
    // Compile-time-constant pattern; a parse failure is a programmer error
    // caught deterministically at first use, not a runtime condition.
    #[expect(
        clippy::expect_used,
        reason = "compile-time-constant pattern; a parse failure would be a programmer error"
    )]
    EVENT_TYPE_REGEX
        .get_or_init(|| Regex::new(r"^[a-z][a-z0-9.]*$").expect("event type regex is valid"))
}

use crate::contexts::shared::ssrf::check_ssrf;
use crate::contexts::sync::application::webhook_conditions::ConditionSet;
use crate::contexts::sync::application::webhook_event_catalog::{
    WebhookEventCatalogItem, webhook_event_catalog,
};
use crate::contexts::sync::infrastructure::persistence::repositories::webhook_subscription_repository::{
    CreateWebhookSubscription, SqlxWebhookSubscriptionRepository, UpdateWebhookFired,
    UpdateWebhookSubscription, WebhookSubscription, WebhookSubscriptionListFilter,
};
use crate::errors::AppError;

pub struct TestFireResult {
    pub status: u16,
    pub ok: bool,
    pub error: Option<String>,
    pub latency_ms: u64,
}

#[derive(Clone)]
pub struct WebhookSubscriptionService {
    repository: SqlxWebhookSubscriptionRepository,
}

/// Build a reqwest client that does NOT follow redirects, for SSRF-safe outbound
/// requests (test-fire and real webhook delivery). `timeout_secs` bounds each request.
pub fn ssrf_safe_client(timeout_secs: u64) -> Result<reqwest::Client, AppError> {
    // `build` only fails on TLS backend initialization; surface it as
    // `AppError::Internal` so the caller can report or record the failure
    // instead of panicking inside a request path.
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|error| AppError::Internal(format!("build SSRF-safe HTTP client: {error}")))
}

// SSRF guard is provided by `crate::contexts::shared::ssrf::check_ssrf`.

impl WebhookSubscriptionService {
    pub fn new(repository: SqlxWebhookSubscriptionRepository) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        filter: &WebhookSubscriptionListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<WebhookSubscription>, u64), AppError> {
        self.repository.list(filter, limit, offset).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<WebhookSubscription>, AppError> {
        self.repository.get(id).await
    }

    /// Return the curated catalog of webhook-worthy domain events.
    #[must_use]
    pub fn event_catalog(&self) -> &'static [WebhookEventCatalogItem] {
        webhook_event_catalog()
    }

    pub fn validate_create(
        name: &str,
        target_url: &str,
        secret: Option<&str>,
        event_types: &[String],
        conditions: Option<&serde_json::Value>,
    ) -> Result<(), AppError> {
        Self::validate_name(name)?;
        Self::validate_target_url(target_url)?;
        if let Some(s) = secret {
            Self::validate_secret(s)?;
        }
        Self::validate_event_types(event_types)?;
        if let Some(c) = conditions {
            Self::validate_conditions(c)?;
        }
        Ok(())
    }

    pub fn validate_update(
        name: Option<&str>,
        target_url: Option<&str>,
        secret: Option<Option<&str>>,
        event_types: Option<&[String]>,
        conditions: Option<Option<&serde_json::Value>>,
    ) -> Result<(), AppError> {
        if let Some(n) = name {
            Self::validate_name(n)?;
        }
        if let Some(u) = target_url {
            Self::validate_target_url(u)?;
        }
        if let Some(Some(s)) = secret {
            Self::validate_secret(s)?;
        }
        if let Some(et) = event_types {
            Self::validate_event_types(et)?;
        }
        if let Some(Some(c)) = conditions {
            Self::validate_conditions(c)?;
        }
        Ok(())
    }

    fn validate_conditions(conditions: &serde_json::Value) -> Result<(), AppError> {
        ConditionSet::validate(conditions)
    }

    fn validate_name(name: &str) -> Result<(), AppError> {
        let len = name.chars().count();
        if !(1..=200).contains(&len) {
            return Err(AppError::Validation(
                "name must be between 1 and 200 characters".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_target_url(target_url: &str) -> Result<(), AppError> {
        let parsed = url::Url::parse(target_url).map_err(|_| {
            AppError::Validation(format!("target_url is not a valid URL: {target_url}"))
        })?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(AppError::Validation(
                "target_url scheme must be http or https".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_secret(secret: &str) -> Result<(), AppError> {
        let len = secret.chars().count();
        if !(SECRET_MIN_LEN..=SECRET_MAX_LEN).contains(&len) {
            return Err(AppError::Validation(format!(
                "secret must be between {SECRET_MIN_LEN} and {SECRET_MAX_LEN} characters"
            )));
        }
        Ok(())
    }

    fn validate_event_types(event_types: &[String]) -> Result<(), AppError> {
        if event_types.is_empty() {
            return Err(AppError::Validation(
                "event_types must not be empty".to_string(),
            ));
        }
        let re = event_type_regex();
        for et in event_types {
            if !re.is_match(et) {
                return Err(AppError::Validation(format!(
                    "event type '{et}' does not match ^[a-z][a-z0-9.]*$"
                )));
            }
        }
        Ok(())
    }

    pub async fn create(
        &self,
        input: CreateWebhookSubscription,
    ) -> Result<WebhookSubscription, AppError> {
        self.repository.create(&input).await
    }

    pub async fn update(
        &self,
        id: Uuid,
        input: UpdateWebhookSubscription,
    ) -> Result<Option<WebhookSubscription>, AppError> {
        self.repository.update(id, &input).await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        self.repository.delete(id).await
    }

    pub async fn clear_secret(&self, id: Uuid) -> Result<bool, AppError> {
        self.repository.clear_secret(id).await
    }

    pub async fn test_fire(&self, id: Uuid) -> Result<Option<TestFireResult>, AppError> {
        let Some(subscription) = self.repository.get(id).await? else {
            return Ok(None);
        };

        // SSRF guard: block private/loopback/link-local targets before making any request.
        check_ssrf(&subscription.target_url).await?;

        let (timestamp_unix, body_str, body_bytes) = test_fire_payload()?;

        // Use a dedicated no-redirect client for test-fire to prevent SSRF via redirects.
        let client = ssrf_safe_client(TEST_FIRE_TIMEOUT_SECS)?;
        let request = build_test_fire_request(
            &client,
            &subscription,
            timestamp_unix,
            &body_str,
            &body_bytes,
        );

        let start = Instant::now();
        let (status, error) = send_test_fire_request(request).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        self.persist_test_fire_result(id, status, &error).await;

        Ok(Some(TestFireResult {
            status,
            ok: error.is_none(),
            error,
            latency_ms,
        }))
    }

    async fn persist_test_fire_result(&self, id: Uuid, status: u16, error: &Option<String>) {
        let fired_at = Utc::now();
        if let Err(err) = self
            .repository
            .update_fired(
                id,
                &UpdateWebhookFired {
                    last_status: i32::from(status),
                    last_error: error.clone(),
                    last_fired_at: fired_at,
                },
            )
            .await
        {
            tracing::warn!(webhook_id = %id, error = %err, "Failed to persist webhook fire result");
        }
    }
}

/// Build the `test.fire` event body and its pre-serialized forms.
fn test_fire_payload() -> Result<(i64, String, Vec<u8>), AppError> {
    let timestamp_unix = Utc::now().timestamp();
    let now_iso = Utc::now().to_rfc3339();
    let body = serde_json::json!({
        "event": "test.fire",
        "timestamp": now_iso,
    });
    let body_str = serde_json::to_string(&body)
        .map_err(|e| AppError::Validation(format!("serialization error: {e}")))?;
    let body_bytes = body_str.as_bytes().to_vec();
    Ok((timestamp_unix, body_str, body_bytes))
}

/// Build the unsigned request, then sign it when the subscription has a secret.
fn build_test_fire_request(
    client: &reqwest::Client,
    subscription: &WebhookSubscription,
    timestamp_unix: i64,
    body_str: &str,
    body_bytes: &[u8],
) -> reqwest::RequestBuilder {
    let nonce = Uuid::now_v7();
    let mut request = client
        .post(&subscription.target_url)
        .header("Content-Type", "application/json")
        .header("X-Piping-Timestamp", timestamp_unix.to_string())
        .header("X-Piping-Nonce", nonce.to_string())
        .body(body_bytes.to_vec());

    if let Some(secret) = &subscription.secret {
        // Signing string format: "<unix_seconds>.<body>"
        let signing_string = format!("{timestamp_unix}.{body_str}");
        let signature = compute_hmac_sha256(secret, signing_string.as_bytes());
        request = request.header("X-Piping-Signature", format!("sha256={signature}"));
    }

    request
}

/// Send the test-fire request and classify the outcome.
async fn send_test_fire_request(request: reqwest::RequestBuilder) -> (u16, Option<String>) {
    match request.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let error = if (200..300).contains(&status) {
                None
            } else {
                Some(format!("HTTP {status}"))
            };
            (status, error)
        }
        Err(error) => (0u16, Some(error.to_string())),
    }
}

/// Compute the hex-encoded HMAC-SHA256 of `body` using `secret` as the key.
/// Shared between manual test-fire and real outbound webhook delivery.
pub fn compute_hmac_sha256(secret: &str, body: &[u8]) -> String {
    // HMAC accepts a key of any length, so `new_from_slice` is infallible here.
    #[expect(
        clippy::expect_used,
        reason = "HMAC accepts a key of any length, so InvalidLength is unreachable"
    )]
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("an HMAC key of any size is accepted");
    mac.update(body);
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}
