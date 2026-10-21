//! REST handlers for `/inventory/templates`.
//!
//! All SQL and business logic live behind `TemplateService`; this module only
//! parses requests, delegates, and shapes JSON responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TemplateListQuery {
    pub status: Option<String>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TemplateRunListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub status: Option<String>,
    pub template_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InventoryTemplatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub variables: Option<Vec<Value>>,
    pub steps: Option<Vec<Value>>,
    pub is_system: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TemplateRunPayload {
    /// Run inputs by variable key. A non-JSON reference input can be exactly
    /// `{"resource":"device_type","id":"<UUID>"}`; each consuming field projects
    /// it to its UUID destination. Literal names remain literal; JSON variables
    /// are never interpreted as selections. See inventory-template-references.json.
    pub variables: Option<Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryTemplateListResponse {
    pub items: Vec<InventoryTemplateResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryTemplateResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub is_system: bool,
    pub owner_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub variables: Vec<Value>,
    pub steps: Vec<Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryTemplateRunResponse {
    pub id: Uuid,
    pub template_id: Uuid,
    pub requested_by_user_id: Option<Uuid>,
    pub status: String,
    pub input_variables: Value,
    pub template_snapshot: Value,
    pub summary: Value,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryTemplateRunListResponse {
    pub items: Vec<InventoryTemplateRunResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryTemplateRunStepResponse {
    pub id: Uuid,
    pub run_id: Uuid,
    pub step_key: String,
    pub status: String,
    pub created_resource_ids: Value,
    pub resolved_payload: Value,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub created_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewRunResponse {
    pub template_id: Uuid,
    pub variables: Value,
    pub steps: Vec<Value>,
}

pub mod handlers;

pub use handlers::*;
