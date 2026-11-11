use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetSetFacetKind {
    Region,
    Site,
    Tenant,
    Role,
    Platform,
    Cluster,
    DeviceType,
    Tag,
    Status,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TargetSetFacet {
    pub id: String,
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TargetSetFacetPage {
    pub items: Vec<TargetSetFacet>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}
