use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

/// Org-wide (instance singleton) branding / white-label settings.
///
/// Backed by the single-row `instance_branding` table (`id = 1`). `theme` is an
/// opaque palette config owned by the frontend; the backend stores it verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct InstanceBranding {
    pub app_name: String,
    pub logo_data_url: Option<String>,
    pub favicon_data_url: Option<String>,
    pub theme: Value,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

/// Mutable branding fields supplied by an admin upsert.
#[derive(Debug, Clone)]
pub struct InstanceBrandingUpdate {
    pub app_name: String,
    pub logo_data_url: Option<String>,
    pub favicon_data_url: Option<String>,
    pub theme: Value,
    pub updated_by: Option<Uuid>,
}
