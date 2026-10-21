use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Cluster domain model — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Cluster {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub cluster_type_id: Option<Uuid>,
    pub cluster_group_id: Option<Uuid>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}

/// Hydrated list row — includes cluster type / group / site / tenant names
/// resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterListView {
    pub record: Cluster,
    pub cluster_type_name: Option<String>,
    pub cluster_group_name: Option<String>,
    pub site_name: Option<String>,
    pub tenant_name: Option<String>,
}
