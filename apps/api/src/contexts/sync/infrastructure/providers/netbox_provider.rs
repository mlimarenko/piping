use std::collections::HashMap;

use serde_json::Value;

use crate::errors::AppError;
use crate::integrations::netbox::{BulkFailure, BulkResult, NetboxRepository};

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SyncProvider: Send + Sync {
    async fn fetch_inventory(&self, resource_key: &str) -> Result<Vec<Value>, AppError>;
    async fn fetch_existing_records(&self, resource_key: &str) -> Result<Vec<Value>, AppError>;
    async fn upsert_records(
        &self,
        resource_key: &str,
        creates: Vec<Value>,
        updates: Vec<Value>,
    ) -> Result<BulkResult, AppError>;
    async fn delete_records(
        &self,
        resource_key: &str,
        deletes: Vec<Value>,
    ) -> Result<BulkResult, AppError>;
}

/// Provider implementation (NetBox adapter)
pub struct NetboxProvider {
    repository: NetboxRepository,
    page_size: usize,
}

impl NetboxProvider {
    pub fn new(repository: NetboxRepository, page_size: usize) -> Self {
        Self {
            repository,
            page_size: page_size.max(1),
        }
    }
}

impl SyncProvider for NetboxProvider {
    async fn fetch_inventory(&self, resource_key: &str) -> Result<Vec<Value>, AppError> {
        self.fetch_all_records(resource_key).await
    }

    async fn fetch_existing_records(&self, resource_key: &str) -> Result<Vec<Value>, AppError> {
        self.fetch_all_records(resource_key).await
    }

    async fn upsert_records(
        &self,
        resource_key: &str,
        creates: Vec<Value>,
        updates: Vec<Value>,
    ) -> Result<BulkResult, AppError> {
        let mut result = BulkResult {
            succeeded_ids: Vec::new(),
            failed: Vec::new(),
        };

        if !creates.is_empty() {
            let created_result = self.repository.bulk_create(resource_key, creates).await?;
            result.succeeded_ids.extend(created_result.succeeded_ids);
            result.failed.extend(created_result.failed);
        }

        if !updates.is_empty() {
            let updated_result = self.repository.bulk_update(resource_key, updates).await?;
            result.succeeded_ids.extend(updated_result.succeeded_ids);
            result.failed.extend(updated_result.failed);
        }

        Ok(result)
    }

    async fn delete_records(
        &self,
        resource_key: &str,
        deletes: Vec<Value>,
    ) -> Result<BulkResult, AppError> {
        let mut result = BulkResult {
            succeeded_ids: Vec::new(),
            failed: Vec::new(),
        };

        for (index, record) in deletes.into_iter().enumerate() {
            let id_value = extract_delete_id(&record);
            let Some(id_value) = id_value else {
                result.failed.push(BulkFailure {
                    index,
                    id: None,
                    message: "Delete record missing id".to_string(),
                    details: Some(record),
                });
                continue;
            };

            match id_value_to_string(&id_value) {
                Some(id) => match self.repository.delete(resource_key, id).await {
                    Ok(()) => result.succeeded_ids.push(id_value),
                    Err(error) => result.failed.push(BulkFailure {
                        index,
                        id: Some(id_value),
                        message: error.to_string(),
                        details: None,
                    }),
                },
                None => result.failed.push(BulkFailure {
                    index,
                    id: Some(id_value),
                    message: "Delete id is not a scalar".to_string(),
                    details: None,
                }),
            }
        }

        Ok(result)
    }
}

impl NetboxProvider {
    async fn fetch_all_records(&self, resource_key: &str) -> Result<Vec<Value>, AppError> {
        let mut results = Vec::new();
        let mut offset = 0usize;
        loop {
            let page = self
                .repository
                .list(
                    resource_key,
                    Some(self.page_size),
                    Some(offset),
                    Some(HashMap::new()),
                    None,
                )
                .await?;

            results.extend(page.results);
            if offset + self.page_size >= page.count {
                break;
            }
            offset += self.page_size;
        }

        Ok(results)
    }
}

fn extract_delete_id(record: &Value) -> Option<Value> {
    match record {
        Value::Object(map) => map.get("id").cloned(),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Some(record.clone()),
        _ => None,
    }
}

fn id_value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}
