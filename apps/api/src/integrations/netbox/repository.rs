use std::collections::HashMap;
use std::sync::Arc;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::errors::AppError;
use crate::integrations::netbox::{NetboxClient, NetboxResourceDescriptor, NetboxResourceRegistry};

const DEFAULT_BULK_CHUNK_SIZE: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetboxPage {
    pub count: usize,
    pub results: Vec<Value>,
    pub next: Option<String>,
    pub previous: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkFailure {
    pub index: usize,
    pub id: Option<Value>,
    pub message: String,
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkResult {
    pub succeeded_ids: Vec<Value>,
    pub failed: Vec<BulkFailure>,
}

pub struct NetboxRepository {
    client: NetboxClient,
    registry: Arc<NetboxResourceRegistry>,
    bulk_chunk_size: usize,
}

impl NetboxRepository {
    pub fn new(client: NetboxClient, registry: Arc<NetboxResourceRegistry>) -> Self {
        Self {
            client,
            registry,
            bulk_chunk_size: DEFAULT_BULK_CHUNK_SIZE,
        }
    }

    pub async fn create_webhook(&self, payload: &Value) -> Result<Value, AppError> {
        self.client.create_webhook(payload).await
    }

    pub async fn update_webhook(
        &self,
        webhook_id: impl ToString,
        payload: &Value,
    ) -> Result<Value, AppError> {
        self.client.update_webhook(webhook_id, payload).await
    }

    pub async fn delete_webhook(&self, webhook_id: impl ToString) -> Result<(), AppError> {
        self.client.delete_webhook(webhook_id).await
    }

    fn descriptor(&self, resource_key: &str) -> Result<&NetboxResourceDescriptor, AppError> {
        self.registry.get_descriptor(resource_key).ok_or_else(|| {
            AppError::Validation(format!(
                "Resource '{}' not found in NetBox registry",
                resource_key
            ))
        })
    }

    fn json_number(value: usize) -> Value {
        Value::Number(Number::from(value as u64))
    }

    fn value_to_scalar_string(value: &Value) -> Option<String> {
        match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            Value::Bool(value) => Some(value.to_string()),
            _ => None,
        }
    }

    fn extract_id(value: &Value) -> Option<Value> {
        match value {
            Value::Object(map) => map.get("id").cloned(),
            _ => None,
        }
    }

    fn extract_error_message(value: &Value) -> String {
        if let Value::Object(map) = value {
            for key in ["message", "error", "detail"] {
                if let Some(Value::String(message)) = map.get(key) {
                    return message.clone();
                }
            }
            if let Some(errors) = map.get("errors") {
                return errors.to_string();
            }
        }
        value.to_string()
    }

    fn extract_error_details(value: &Value) -> Option<Value> {
        if let Value::Object(map) = value {
            if let Some(details) = map.get("details") {
                return Some(details.clone());
            }
            if let Some(errors) = map.get("errors") {
                return Some(errors.clone());
            }
        }
        None
    }

    fn normalize_payload(value: Value) -> Result<Map<String, Value>, AppError> {
        match value {
            Value::Object(map) => Ok(map),
            _ => Err(AppError::Validation(
                "NetBox payload must be a JSON object".to_string(),
            )),
        }
    }

    fn validate_filter_keys(
        descriptor: &NetboxResourceDescriptor,
        filters: &HashMap<String, Value>,
    ) -> Result<(), AppError> {
        let extra_params = [
            "brief",
            "fields",
            "exclude",
            "depth",
            "render_config_context",
            "ordering",
        ];

        let mut invalid: Vec<String> = Vec::new();
        for key in filters.keys() {
            if extra_params.contains(&key.as_str()) {
                continue;
            }
            if !descriptor.filter_params.contains_key(key) {
                invalid.push(key.clone());
            }
        }

        if invalid.is_empty() {
            Ok(())
        } else {
            invalid.sort();
            Err(AppError::Validation(format!(
                "Unknown filter parameter(s) for '{}': {}",
                descriptor.resource_key,
                invalid.join(", ")
            )))
        }
    }

    pub async fn get(
        &self,
        resource_key: &str,
        resource_id: impl ToString,
        params: Option<HashMap<String, Value>>,
    ) -> Result<Value, AppError> {
        let descriptor = self.descriptor(resource_key)?;
        if !descriptor.supports_operation("get") {
            return Err(AppError::Validation(format!(
                "Resource '{}' does not support get operation",
                resource_key
            )));
        }
        let path = descriptor.get_detail_path(&resource_id.to_string());
        let response = self
            .client
            .request_json(Method::GET, &path, params.as_ref(), None)
            .await?;
        if response.is_null() {
            return Ok(Value::Null);
        }
        if response.is_object() {
            return Ok(response);
        }
        if let Value::Array(values) = response
            && let Some(first) = values.into_iter().next()
        {
            return Ok(first);
        }
        Ok(Value::Null)
    }

    pub async fn list(
        &self,
        resource_key: &str,
        limit: Option<usize>,
        offset: Option<usize>,
        filters: Option<HashMap<String, Value>>,
        ordering: Option<String>,
    ) -> Result<NetboxPage, AppError> {
        let descriptor = self.descriptor(resource_key)?;
        if !descriptor.supports_operation("list") {
            return Err(AppError::Validation(format!(
                "Resource '{}' does not support list operation",
                resource_key
            )));
        }

        let mut params: HashMap<String, Value> = HashMap::new();
        if let Some(limit) = limit {
            params.insert("limit".to_string(), Self::json_number(limit));
        }
        if let Some(offset) = offset {
            params.insert("offset".to_string(), Self::json_number(offset));
        }

        let mut ordering_value = ordering;
        if let Some(mut filters) = filters {
            filters.retain(|_, value| !value.is_null());
            if let Some(ordering_override) = filters.remove("ordering")
                && ordering_value.is_none()
            {
                ordering_value = Self::value_to_scalar_string(&ordering_override);
            }
            Self::validate_filter_keys(descriptor, &filters)?;
            for (key, value) in filters {
                params.insert(key, value);
            }
        }

        if let Some(ordering) = ordering_value
            && descriptor.ordering_supported
            && !ordering.is_empty()
        {
            params.insert("ordering".to_string(), Value::String(ordering));
        }

        let response = self
            .client
            .request_json(Method::GET, &descriptor.list_path, Some(&params), None)
            .await?;
        match response {
            Value::Object(map) => Ok(NetboxPage {
                count: map
                    .get("count")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as usize,
                results: map
                    .get("results")
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default(),
                next: map
                    .get("next")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string()),
                previous: map
                    .get("previous")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string()),
            }),
            Value::Array(items) => Ok(NetboxPage {
                count: items.len(),
                results: items,
                next: None,
                previous: None,
            }),
            _ => Ok(NetboxPage {
                count: 0,
                results: Vec::new(),
                next: None,
                previous: None,
            }),
        }
    }

    pub async fn create(&self, resource_key: &str, data: Value) -> Result<Value, AppError> {
        let descriptor = self.descriptor(resource_key)?;
        if !descriptor.supports_operation("create") {
            return Err(AppError::Validation(format!(
                "Resource '{}' does not support create operation",
                resource_key
            )));
        }
        let payload = Value::Object(Self::normalize_payload(data)?);
        self.client
            .request_json(Method::POST, &descriptor.list_path, None, Some(&payload))
            .await
    }

    pub async fn update(
        &self,
        resource_key: &str,
        resource_id: impl ToString,
        data: Value,
    ) -> Result<Value, AppError> {
        let descriptor = self.descriptor(resource_key)?;
        if !descriptor.supports_operation("update") {
            return Err(AppError::Validation(format!(
                "Resource '{}' does not support update operation",
                resource_key
            )));
        }
        let payload = Value::Object(Self::normalize_payload(data)?);
        let path = descriptor.get_detail_path(&resource_id.to_string());
        self.client
            .request_json(Method::PATCH, &path, None, Some(&payload))
            .await
    }

    pub async fn delete(
        &self,
        resource_key: &str,
        resource_id: impl ToString,
    ) -> Result<(), AppError> {
        let descriptor = self.descriptor(resource_key)?;
        if !descriptor.supports_operation("delete") {
            return Err(AppError::Validation(format!(
                "Resource '{}' does not support delete operation",
                resource_key
            )));
        }
        let path = descriptor.get_detail_path(&resource_id.to_string());
        self.client
            .request_json(Method::DELETE, &path, None, None)
            .await?;
        Ok(())
    }

    pub async fn bulk_create(
        &self,
        resource_key: &str,
        inputs: Vec<Value>,
    ) -> Result<BulkResult, AppError> {
        let descriptor = self.descriptor(resource_key)?;

        if inputs.is_empty() {
            return Ok(BulkResult {
                succeeded_ids: Vec::new(),
                failed: Vec::new(),
            });
        }

        // Fallback: NetBox 4.x exposes `bulk_update`/`bulk_delete` on most
        // resources but only single `create` (POST per row). When bulk_create
        // is absent, fan out to per-record POSTs so the sync engine works
        // uniformly without forcing every resource into a bulk-capable list.
        if !descriptor.supports_operation("bulk_create") {
            if !descriptor.supports_operation("create") {
                return Err(AppError::Validation(format!(
                    "Resource '{}' does not support create operations",
                    resource_key
                )));
            }
            let mut succeeded_ids: Vec<Value> = Vec::new();
            let mut failed: Vec<BulkFailure> = Vec::new();
            for (index, input) in inputs.into_iter().enumerate() {
                match self.create(resource_key, input).await {
                    Ok(value) => {
                        if let Some(id) = value.get("id").cloned() {
                            succeeded_ids.push(id);
                        }
                    }
                    Err(error) => {
                        failed.push(BulkFailure {
                            index,
                            id: None,
                            message: error.to_string(),
                            details: None,
                        });
                    }
                }
            }
            return Ok(BulkResult {
                succeeded_ids,
                failed,
            });
        }

        let chunk_size = self.bulk_chunk_size.max(1);
        let mut succeeded_ids = Vec::new();
        let mut failed = Vec::new();

        for (chunk_idx, chunk) in inputs.chunks(chunk_size).enumerate() {
            let result = self.bulk_create_native(descriptor, chunk).await?;
            let offset = chunk_idx * chunk_size;
            succeeded_ids.extend(result.succeeded_ids);
            for failure in result.failed {
                failed.push(BulkFailure {
                    index: failure.index + offset,
                    id: failure.id,
                    message: failure.message,
                    details: failure.details,
                });
            }
        }

        Ok(BulkResult {
            succeeded_ids,
            failed,
        })
    }

    async fn bulk_create_native(
        &self,
        descriptor: &NetboxResourceDescriptor,
        inputs: &[Value],
    ) -> Result<BulkResult, AppError> {
        let payload = Value::Array(inputs.to_vec());
        let response = self
            .client
            .request_json(Method::POST, &descriptor.list_path, None, Some(&payload))
            .await?;
        Ok(Self::parse_bulk_response(response, inputs))
    }

    pub async fn bulk_update(
        &self,
        resource_key: &str,
        inputs: Vec<Value>,
    ) -> Result<BulkResult, AppError> {
        let descriptor = self.descriptor(resource_key)?;
        if !descriptor.supports_operation("bulk_update") {
            return Err(AppError::Validation(format!(
                "Resource '{}' does not support bulk update operations",
                resource_key
            )));
        }

        if inputs.is_empty() {
            return Ok(BulkResult {
                succeeded_ids: Vec::new(),
                failed: Vec::new(),
            });
        }

        let chunk_size = self.bulk_chunk_size.max(1);
        let mut succeeded_ids = Vec::new();
        let mut failed = Vec::new();

        for (chunk_idx, chunk) in inputs.chunks(chunk_size).enumerate() {
            let result = self.bulk_update_native(descriptor, chunk).await?;
            let offset = chunk_idx * chunk_size;
            succeeded_ids.extend(result.succeeded_ids);
            for failure in result.failed {
                failed.push(BulkFailure {
                    index: failure.index + offset,
                    id: failure.id,
                    message: failure.message,
                    details: failure.details,
                });
            }
        }

        Ok(BulkResult {
            succeeded_ids,
            failed,
        })
    }

    async fn bulk_update_native(
        &self,
        descriptor: &NetboxResourceDescriptor,
        inputs: &[Value],
    ) -> Result<BulkResult, AppError> {
        let payload = Value::Array(inputs.to_vec());
        let response = self
            .client
            .request_json(Method::PATCH, &descriptor.list_path, None, Some(&payload))
            .await?;

        if let Value::Array(items) = &response
            && items.is_empty()
        {
            let succeeded_ids = inputs
                .iter()
                .filter_map(Self::extract_id)
                .collect::<Vec<_>>();
            return Ok(BulkResult {
                succeeded_ids,
                failed: Vec::new(),
            });
        }

        Ok(Self::parse_bulk_response(response, inputs))
    }

    fn parse_bulk_response(response: Value, inputs: &[Value]) -> BulkResult {
        let mut succeeded_ids = Vec::new();
        let mut failed = Vec::new();

        match response {
            Value::Array(items) => {
                for (idx, item) in items.into_iter().enumerate() {
                    if let Some(id) = Self::extract_id(&item) {
                        succeeded_ids.push(id);
                    } else {
                        failed.push(BulkFailure {
                            index: idx,
                            id: None,
                            message: Self::extract_error_message(&item),
                            details: Self::extract_error_details(&item),
                        });
                    }
                }
            }
            Value::Object(map) => {
                if let Some(Value::Array(results)) = map.get("results") {
                    for (idx, item) in results.iter().enumerate() {
                        if let Some(id) = Self::extract_id(item) {
                            succeeded_ids.push(id);
                        } else {
                            failed.push(BulkFailure {
                                index: idx,
                                id: None,
                                message: Self::extract_error_message(item),
                                details: Self::extract_error_details(item),
                            });
                        }
                    }
                }
                if let Some(Value::Array(errors)) = map.get("errors") {
                    for error in errors {
                        let idx = error
                            .get("index")
                            .and_then(|value| value.as_u64())
                            .or_else(|| error.get("idx").and_then(|value| value.as_u64()))
                            .unwrap_or(0) as usize;
                        failed.push(BulkFailure {
                            index: idx,
                            id: None,
                            message: Self::extract_error_message(error),
                            details: Self::extract_error_details(error),
                        });
                    }
                }
                if succeeded_ids.is_empty() && failed.is_empty() {
                    for (idx, input) in inputs.iter().enumerate() {
                        failed.push(BulkFailure {
                            index: idx,
                            id: Self::extract_id(input),
                            message: "NetBox bulk response was empty".to_string(),
                            details: None,
                        });
                    }
                }
            }
            _ => {
                for (idx, input) in inputs.iter().enumerate() {
                    failed.push(BulkFailure {
                        index: idx,
                        id: Self::extract_id(input),
                        message: "NetBox bulk response was invalid".to_string(),
                        details: None,
                    });
                }
            }
        }

        BulkResult {
            succeeded_ids,
            failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::integrations::netbox::{FilterParam, NetboxResourceDescriptor};

    fn descriptor_with_filters() -> NetboxResourceDescriptor {
        NetboxResourceDescriptor {
            resource_key: "dcim.sites".to_string(),
            list_path: "dcim/sites/".to_string(),
            detail_path_template: "dcim/sites/{id}/".to_string(),
            operations_supported: ["list".to_string()].into_iter().collect(),
            filter_params: [(
                "name".to_string(),
                FilterParam {
                    param_type: "string".to_string(),
                    description: "Name filter".to_string(),
                },
            )]
            .into_iter()
            .collect(),
            ordering_supported: true,
            fields_meta: Vec::new(),
            changelog_message_supported: false,
        }
    }

    #[test]
    fn parse_bulk_response_array_tracks_failures() {
        let response = json!([
            { "id": 1, "name": "ok" },
            { "error": "bad" }
        ]);
        let inputs = vec![json!({"id": 1}), json!({"id": 2})];
        let result = NetboxRepository::parse_bulk_response(response, &inputs);
        assert_eq!(result.succeeded_ids.len(), 1);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].index, 1);
    }

    #[test]
    fn parse_bulk_response_object_handles_results_and_errors() {
        let response = json!({
            "results": [
                { "id": 10 },
                { "message": "failed" }
            ],
            "errors": [
                { "index": 1, "message": "failed" }
            ]
        });
        let inputs = vec![json!({"id": 10}), json!({"id": 11})];
        let result = NetboxRepository::parse_bulk_response(response, &inputs);
        assert!(result.succeeded_ids.iter().any(|id| id == &json!(10)));
        assert!(!result.failed.is_empty());
    }

    #[test]
    fn validate_filter_keys_rejects_unknown_params() {
        let descriptor = descriptor_with_filters();
        let filters = HashMap::from([("unknown".to_string(), json!("value"))]);
        let error =
            NetboxRepository::validate_filter_keys(&descriptor, &filters).expect_err("unknown");
        assert!(
            error.to_string().contains("Unknown filter parameter"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_filter_keys_allows_known_and_extra_params() {
        let descriptor = descriptor_with_filters();
        let filters = HashMap::from([
            ("name".to_string(), json!("site-a")),
            ("ordering".to_string(), json!("name")),
            ("fields".to_string(), json!("name")),
        ]);
        NetboxRepository::validate_filter_keys(&descriptor, &filters).expect("valid");
    }
}
