//! Layout service — orchestrates validation + merging of system/user layouts
//! on top of `LayoutRepository`. Handlers in
//! `api::rest::routes::inventory::layouts` delegate to this service.

use std::sync::Arc;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::contexts::inventory::infrastructure::persistence::repositories::{
    LayoutRepository, SystemLayoutRow,
};
use crate::errors::AppError;

// ---------------------------------------------------------------------------
// Response DTOs (exposed to handlers for JSON envelopes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LayoutProfile {
    pub id: Uuid,
    pub entity_type: String,
    pub layout: Value,
    pub version: i32,
    pub is_active: bool,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct EffectiveUserLayout {
    pub entity_type: String,
    pub source: String,
    pub effective_layout: Value,
    pub system_layout: Option<Value>,
    pub user_layout: Option<Value>,
}

const SUPPORTED_ENTITY_TYPES: &[&str] = &[
    "tenant_groups",
    "tenants",
    "regions",
    "site_groups",
    "sites",
    "cluster_types",
    "cluster_groups",
    "clusters",
    "manufacturers",
    "device_types",
    "device_roles",
    "devices",
    "virtual_machines",
];

fn ensure_supported_entity_type(entity_type: &str) -> Result<(), AppError> {
    if SUPPORTED_ENTITY_TYPES.contains(&entity_type) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "Unsupported entity_type for layout: {entity_type}"
        )))
    }
}

fn into_profile(row: SystemLayoutRow) -> LayoutProfile {
    LayoutProfile {
        id: row.id,
        entity_type: row.entity_type,
        layout: row.layout,
        version: row.version,
        is_active: row.is_active,
        updated_by_user_id: row.updated_by_user_id,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn default_layout() -> Value {
    json!({
        "columns": ["name"],
        "card": {"title": "name", "badges": []},
    })
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct LayoutService {
    repo: Arc<dyn LayoutRepository>,
}

impl LayoutService {
    pub fn new(repo: Arc<dyn LayoutRepository>) -> Self {
        Self { repo }
    }

    pub async fn list_system_layouts(&self) -> Result<Vec<LayoutProfile>, AppError> {
        let rows = self.repo.list_active_system_layouts().await?;
        Ok(rows.into_iter().map(into_profile).collect())
    }

    pub async fn get_system_layout(
        &self,
        entity_type: &str,
    ) -> Result<Option<LayoutProfile>, AppError> {
        Ok(self
            .repo
            .get_active_system_layout(entity_type)
            .await?
            .map(into_profile))
    }

    pub async fn upsert_system_layout(
        &self,
        entity_type: &str,
        layout: &Value,
        updated_by_user_id: Option<Uuid>,
    ) -> Result<LayoutProfile, AppError> {
        ensure_supported_entity_type(entity_type)?;
        self.repo
            .upsert_system_layout(entity_type, layout, updated_by_user_id)
            .await?;
        self.get_system_layout(entity_type)
            .await?
            .ok_or_else(|| AppError::NotFound("System layout not found".to_string()))
    }

    pub async fn get_effective_user_layout(
        &self,
        user_id: Uuid,
        entity_type: &str,
    ) -> Result<EffectiveUserLayout, AppError> {
        let system_layout = self.repo.load_system_layout_json(entity_type).await?;
        let user_layout = self
            .repo
            .load_user_layout_json(user_id, entity_type)
            .await?;

        let (source, effective_layout) = if let Some(layout) = user_layout.clone() {
            ("user".to_string(), layout)
        } else if let Some(layout) = system_layout.clone() {
            ("system".to_string(), layout)
        } else {
            ("default".to_string(), default_layout())
        };

        Ok(EffectiveUserLayout {
            entity_type: entity_type.to_string(),
            source,
            effective_layout,
            system_layout,
            user_layout,
        })
    }

    pub async fn upsert_user_layout(
        &self,
        user_id: Uuid,
        entity_type: &str,
        layout: &Value,
    ) -> Result<EffectiveUserLayout, AppError> {
        ensure_supported_entity_type(entity_type)?;
        self.repo
            .upsert_user_layout(user_id, entity_type, layout)
            .await?;
        self.get_effective_user_layout(user_id, entity_type).await
    }

    pub async fn delete_user_layout(
        &self,
        user_id: Uuid,
        entity_type: &str,
    ) -> Result<(), AppError> {
        self.repo.delete_user_layout(user_id, entity_type).await
    }
}
