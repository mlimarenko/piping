//! Bulk resource service — orchestrates per-item validation + dispatch for the
//! `/api/v1/inventory/resources/{entity_type}/bulk` endpoint on top of
//! `BulkResourceRepository`. Handlers stay thin.

use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::contexts::inventory::infrastructure::persistence::repositories::{
    BulkColumn, BulkResourceRepository,
};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct BulkResultItem {
    pub id: Option<String>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BulkOperationOutcome {
    pub entity_type: String,
    pub operation: String,
    pub total: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<BulkResultItem>,
}

#[derive(Debug, Clone)]
pub struct BulkOperationInput {
    pub entity_type: String,
    pub operation: String,
    pub items: Vec<Value>,
    pub params: Option<Value>,
}

pub struct BulkResourceService {
    repo: Arc<dyn BulkResourceRepository>,
}

impl BulkResourceService {
    pub fn new(repo: Arc<dyn BulkResourceRepository>) -> Self {
        Self { repo }
    }

    pub async fn execute(
        &self,
        input: BulkOperationInput,
    ) -> Result<BulkOperationOutcome, AppError> {
        if input.items.is_empty() {
            return Err(AppError::Validation("items must not be empty".to_string()));
        }

        let mut results = Vec::with_capacity(input.items.len());
        let mut success_count = 0usize;

        for item in &input.items {
            let item_id = extract_item_id(item);
            match self
                .dispatch(
                    &input.entity_type,
                    &input.operation,
                    item,
                    input.params.as_ref(),
                )
                .await
            {
                Ok(()) => {
                    success_count += 1;
                    results.push(BulkResultItem {
                        id: item_id,
                        success: true,
                        error: None,
                    });
                }
                Err(error) => {
                    results.push(BulkResultItem {
                        id: item_id,
                        success: false,
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        let total = results.len();
        let failure_count = total.saturating_sub(success_count);

        Ok(BulkOperationOutcome {
            entity_type: input.entity_type,
            operation: input.operation,
            total,
            success_count,
            failure_count,
            results,
        })
    }

    async fn dispatch(
        &self,
        entity_type: &str,
        operation: &str,
        item: &Value,
        params: Option<&Value>,
    ) -> Result<(), AppError> {
        let id = extract_required_item_uuid(item)?;
        match operation {
            "delete" => self.repo.delete_by_id(entity_type, id).await,
            "set_status" => {
                let status = params
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AppError::Validation("params.status is required for set_status".to_string())
                    })?;
                self.repo
                    .update_text_column(entity_type, BulkColumn::Status, id, Some(status))
                    .await
            }
            "assign_tenant" => {
                let tenant_id = extract_optional_uuid_param(params, "tenant_id")?;
                self.repo
                    .update_uuid_column(entity_type, BulkColumn::TenantId, id, tenant_id)
                    .await
            }
            "set_role" => {
                let role_id = extract_optional_uuid_param(params, "role_id")?;
                self.repo
                    .update_uuid_column(entity_type, BulkColumn::RoleId, id, role_id)
                    .await
            }
            "set_site" => {
                let site_id = extract_optional_uuid_param(params, "site_id")?;
                self.repo
                    .update_uuid_column(entity_type, BulkColumn::SiteId, id, site_id)
                    .await
            }
            "set_cluster" => {
                let cluster_id = extract_optional_uuid_param(params, "cluster_id")?;
                self.repo
                    .update_uuid_column(entity_type, BulkColumn::ClusterId, id, cluster_id)
                    .await
            }
            "set_device_type" => {
                let device_type_id = extract_optional_uuid_param(params, "device_type_id")?;
                self.repo
                    .update_uuid_column(entity_type, BulkColumn::DeviceTypeId, id, device_type_id)
                    .await
            }
            other => Err(AppError::Validation(format!(
                "Unsupported bulk operation: {other}"
            ))),
        }
    }
}

fn extract_item_id(item: &Value) -> Option<String> {
    item.get("id")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn extract_required_item_uuid(item: &Value) -> Result<Uuid, AppError> {
    let raw = extract_item_id(item)
        .ok_or_else(|| AppError::Validation("Each item must include id".to_string()))?;
    Uuid::parse_str(&raw).map_err(|error| AppError::Validation(format!("Invalid item.id: {error}")))
}

fn extract_optional_uuid_param(
    params: Option<&Value>,
    key: &str,
) -> Result<Option<Uuid>, AppError> {
    let raw = params
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let Some(raw) = raw else {
        return Ok(None);
    };
    Uuid::parse_str(raw)
        .map(Some)
        .map_err(|error| AppError::Validation(format!("Invalid params.{key}: {error}")))
}
