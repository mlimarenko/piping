use std::collections::HashMap;

use crate::config::{DEFAULT_HTTP_TIMEOUT, NetboxConfig};
use crate::errors::AppError;
use crate::observability::metrics::{self, ExternalTarget};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Method, Url};
use serde_json::Value;

/// NetBox API client with token auth and pagination helpers
pub struct NetboxClient {
    base_url: String,
    client: reqwest::Client,
}

/// Capture needs HTTP status evidence without exposing provider error bodies.
pub(crate) enum CaptureGetResponse {
    Json(Value),
    HttpStatus(u16),
}

/// One NetBox object type (content type) as event rules and the registry see it.
///
/// `key` is the canonical `app_label.model` spelling (NetBox's `value` when
/// present), which is what event-rule `object_types` accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetboxObjectType {
    pub id: i64,
    pub key: String,
    pub app_label: String,
    pub model: String,
}

impl NetboxClient {
    /// GET-only capture boundary. Neither schema paths nor pagination links may
    /// replace the authenticated origin or introduce query/fragment components.
    /// The existing client's no-redirect/no-retry policy remains authoritative.
    pub(crate) async fn capture_get(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<CaptureGetResponse, AppError> {
        let invalid = || AppError::Validation("netbox_capture:request_path".into());
        let base = Url::parse(&self.base_url).map_err(|_| invalid())?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !path.starts_with("api/")
            || !path.ends_with('/')
            || path.split('/').any(|part| part == "." || part == "..")
            || !path
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_'))
            || path.contains("//")
        {
            return Err(invalid());
        }
        let mut call = metrics::start_external_call(ExternalTarget::Netbox, "capture_get");
        let mut url = Url::parse(&format!("{}/{}", self.base_url.trim_end_matches('/'), path))
            .map_err(|_| invalid())?;
        if !params.is_empty() {
            let mut query = url.query_pairs_mut();
            for (key, value) in params {
                query.append_pair(key, value);
            }
        }
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| AppError::External("netbox_capture:request_failed".into()))?;
        if !response.status().is_success() {
            return Ok(CaptureGetResponse::HttpStatus(response.status().as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AppError::External("netbox_capture:response_read".into()))?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::External("netbox_capture:response_encoding".into()))?;
        call.succeeded();
        Ok(CaptureGetResponse::Json(value))
    }

    pub fn from_config(config: &NetboxConfig) -> Result<Self, AppError> {
        Self::new(config.base_url.clone(), config.api_token.clone())
    }

    pub fn new(base_url: String, api_token: String) -> Result<Self, AppError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let auth_value = format!("Token {}", api_token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value)
                .map_err(|e| AppError::Validation(format!("Invalid NetBox token: {}", e)))?,
        );

        let client = reqwest::Client::builder()
            .timeout(DEFAULT_HTTP_TIMEOUT)
            .default_headers(headers)
            // The outbox owns retries. In particular, a possible POST must never
            // be resent by an HTTP retry policy or a redirect behind that owner.
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AppError::External(format!("Failed to build NetBox client: {}", e)))?;

        Ok(Self { base_url, client })
    }

    pub async fn validate_connection(&self) -> Result<(), AppError> {
        let _ = self
            .request_json(Method::GET, "api/status/", None, None)
            .await?;
        Ok(())
    }

    pub async fn request_json(
        &self,
        method: Method,
        path: &str,
        params: Option<&HashMap<String, Value>>,
        body: Option<&Value>,
    ) -> Result<Value, AppError> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut url = Url::parse(&url)
            .map_err(|e| AppError::Validation(format!("Invalid NetBox URL: {}", e)))?;

        let operation = if method == Method::GET {
            "get"
        } else if method == Method::POST {
            "post"
        } else if method == Method::PATCH {
            "patch"
        } else if method == Method::DELETE {
            "delete"
        } else {
            "other"
        };
        let mut call = metrics::start_external_call(ExternalTarget::Netbox, operation);

        if let Some(params) = params {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in params {
                match value {
                    Value::String(value) => {
                        pairs.append_pair(key, value);
                    }
                    Value::Number(value) => {
                        pairs.append_pair(key, &value.to_string());
                    }
                    Value::Bool(value) => {
                        pairs.append_pair(key, &value.to_string());
                    }
                    Value::Array(items) => {
                        for item in items {
                            if let Some(text) = scalar_to_string(item) {
                                pairs.append_pair(key, &text);
                            }
                        }
                    }
                    Value::Null => {}
                    Value::Object(_) => {
                        pairs.append_pair(key, &value.to_string());
                    }
                }
            }
        }

        let deleting = method == Method::DELETE;
        let mut builder = self.client.request(method, url);

        if let Some(body) = body {
            builder = builder.json(body);
        }

        let response = builder.send().await.map_err(|e| {
            AppError::External(format!("NetBox request failed: {}", e.without_url()))
        })?;

        let status = response.status();
        // A retry after a completed DELETE has the same successful absent state.
        // No other method or status is suppressed.
        if deleting && status == reqwest::StatusCode::NOT_FOUND {
            call.succeeded();
            return Ok(Value::Null);
        }
        if !status.is_success() {
            return Err(AppError::External(format!(
                "NetBox returned status {}",
                status
            )));
        }

        let bytes = response.bytes().await.map_err(|e| {
            AppError::External(format!(
                "Failed to read NetBox response: {}",
                e.without_url()
            ))
        })?;
        if bytes.is_empty() {
            call.succeeded();
            return Ok(Value::Null);
        }
        let value = serde_json::from_slice::<Value>(&bytes)
            .map_err(|e| AppError::External(format!("Invalid NetBox JSON: {}", e)))?;
        call.succeeded();
        Ok(value)
    }

    pub async fn list_webhooks_by_name(&self, name: &str) -> Result<Vec<Value>, AppError> {
        let mut params = HashMap::new();
        params.insert("name".to_string(), Value::String(name.to_string()));
        let response = self
            .request_json(Method::GET, "api/extras/webhooks/", Some(&params), None)
            .await?;
        match response {
            Value::Object(map) => Ok(map
                .get("results")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default()),
            Value::Array(items) => Ok(items),
            _ => Ok(Vec::new()),
        }
    }

    pub async fn create_webhook(&self, payload: &Value) -> Result<Value, AppError> {
        self.request_json(Method::POST, "api/extras/webhooks/", None, Some(payload))
            .await
    }

    pub async fn update_webhook(
        &self,
        webhook_id: impl ToString,
        payload: &Value,
    ) -> Result<Value, AppError> {
        let path = format!(
            "api/extras/webhooks/{}/",
            webhook_id.to_string().trim_matches('/')
        );
        self.request_json(Method::PATCH, &path, None, Some(payload))
            .await
    }

    pub async fn delete_webhook(&self, webhook_id: impl ToString) -> Result<(), AppError> {
        let path = format!(
            "api/extras/webhooks/{}/",
            webhook_id.to_string().trim_matches('/')
        );
        let _ = self.request_json(Method::DELETE, &path, None, None).await?;
        Ok(())
    }

    pub async fn list_event_rules_by_name(&self, name: &str) -> Result<Vec<Value>, AppError> {
        let mut params = HashMap::new();
        params.insert("name".to_string(), Value::String(name.to_string()));
        let response = self
            .request_json(Method::GET, "api/extras/event-rules/", Some(&params), None)
            .await?;
        match response {
            Value::Object(map) => Ok(map
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()),
            Value::Array(items) => Ok(items),
            _ => Ok(Vec::new()),
        }
    }

    pub async fn create_event_rule(&self, payload: &Value) -> Result<Value, AppError> {
        self.request_json(Method::POST, "api/extras/event-rules/", None, Some(payload))
            .await
    }

    pub async fn update_event_rule(
        &self,
        event_rule_id: impl ToString,
        payload: &Value,
    ) -> Result<Value, AppError> {
        let path = format!(
            "api/extras/event-rules/{}/",
            event_rule_id.to_string().trim_matches('/')
        );
        self.request_json(Method::PATCH, &path, None, Some(payload))
            .await
    }

    pub async fn delete_event_rule(&self, event_rule_id: impl ToString) -> Result<(), AppError> {
        let path = format!(
            "api/extras/event-rules/{}/",
            event_rule_id.to_string().trim_matches('/')
        );
        let _ = self.request_json(Method::DELETE, &path, None, None).await?;
        Ok(())
    }

    pub async fn get_object_type_key(&self, object_type_id: i64) -> Result<String, AppError> {
        let path = format!("api/extras/object-types/{}/", object_type_id);
        let response = self.request_json(Method::GET, &path, None, None).await?;
        let map = response.as_object().ok_or_else(|| {
            AppError::External("NetBox object type response must be a JSON object".to_string())
        })?;
        if let Some(value) = map.get("value").and_then(Value::as_str)
            && !value.trim().is_empty()
        {
            return Ok(value.trim().to_string());
        }
        let app_label = map
            .get("app_label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::External("NetBox object type response missing app_label".to_string())
            })?;
        let model = map
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::External("NetBox object type response missing model".to_string())
            })?;
        Ok(format!("{app_label}.{model}"))
    }

    /// List every object type the instance exposes. Event-rule subscriptions
    /// need canonical keys, and the API is paginated.
    pub async fn list_object_types(&self) -> Result<Vec<NetboxObjectType>, AppError> {
        let mut out = Vec::new();
        let mut offset: u64 = 0;
        loop {
            let mut params = HashMap::new();
            params.insert("limit".to_string(), Value::from(100));
            params.insert("offset".to_string(), Value::from(offset));
            let response = self
                .request_json(Method::GET, "api/extras/object-types/", Some(&params), None)
                .await?;
            let results = response
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = response
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or(results.len() as u64);
            for row in &results {
                let map = row.as_object().ok_or_else(|| {
                    AppError::External("NetBox object type row must be a JSON object".to_string())
                })?;
                let app_label = map
                    .get("app_label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AppError::External(
                            "NetBox object type response missing app_label".to_string(),
                        )
                    })?;
                let model = map
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AppError::External(
                            "NetBox object type response missing model".to_string(),
                        )
                    })?;
                let key = map
                    .get("value")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{app_label}.{model}"));
                out.push(NetboxObjectType {
                    id: map.get("id").and_then(Value::as_i64).unwrap_or(0),
                    key,
                    app_label: app_label.to_string(),
                    model: model.to_string(),
                });
            }
            offset += results.len() as u64;
            if results.is_empty() || offset >= total {
                break;
            }
        }
        Ok(out)
    }

    /// Read the schema from its canonical endpoint; YAML also accepts JSON.
    pub async fn get_schema(&self) -> Result<Value, AppError> {
        let path = "api/schema/";
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let response = self
            .client
            .request(
                Method::GET,
                Url::parse(&url)
                    .map_err(|e| AppError::Validation(format!("Invalid NetBox URL: {}", e)))?,
            )
            .send()
            .await
            .map_err(|e| {
                AppError::External(format!("NetBox request failed: {}", e.without_url()))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::External(format!(
                "NetBox returned status {}",
                status
            )));
        }

        let body = response.text().await.map_err(|e| {
            AppError::External(format!(
                "Failed to read NetBox response: {}",
                e.without_url()
            ))
        })?;
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }

        serde_yml::from_str::<Value>(&body)
            .map_err(|_| AppError::External("Invalid NetBox schema".into()))
    }
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}
