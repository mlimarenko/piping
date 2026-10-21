use std::collections::HashMap;

use crate::config::{DEFAULT_HTTP_TIMEOUT, NetboxConfig};
use crate::errors::AppError;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Method, Url};
use serde_json::Value;

/// NetBox API client with token auth and pagination helpers
pub struct NetboxClient {
    base_url: String,
    client: reqwest::Client,
}

impl NetboxClient {
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

        let mut builder = self.client.request(method, url);

        if let Some(body) = body {
            builder = builder.json(body);
        }

        let response = builder.send().await.map_err(|e| {
            AppError::External(format!("NetBox request failed: {}", e.without_url()))
        })?;

        let status = response.status();
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
            return Ok(Value::Null);
        }
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|e| AppError::External(format!("Invalid NetBox JSON: {}", e)))
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
