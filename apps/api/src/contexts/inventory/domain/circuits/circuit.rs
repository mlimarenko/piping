use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Circuit domain model — NetBox `Circuits > circuits` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Circuit {
    pub id: Uuid,
    /// Circuit ID — human-readable identifier (e.g. provider circuit number).
    pub cid: String,
    /// NetBox `CircuitStatusChoices` (e.g. `active`, `planned`, `provisioning`, `offline`, `decommissioned`).
    pub status: String,
    pub provider_id: Uuid,
    pub type_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub provider_account_id: Option<Uuid>,
    pub termination_a_id: Option<Uuid>,
    pub termination_z_id: Option<Uuid>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    /// Committed data rate in Kbps (must be >= 0).
    pub commit_rate: Option<i32>,
    pub description: String,
    pub comments: String,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
    pub custom_field_data: Option<serde_json::Value>,
}
