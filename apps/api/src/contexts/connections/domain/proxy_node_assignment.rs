use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contexts::connection_config::ConnConfScope;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyNodeAssignment {
    pub id: Uuid,
    pub proxy_node_id: Uuid,
    pub scope_type: ConnConfScope,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetProxyAssignment {
    pub target_id: Uuid,
    pub proxy_node_id: Uuid,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewProxyNodeAssignment {
    pub proxy_node_id: Uuid,
    pub scope_type: ConnConfScope,
    pub scope_id: Option<Uuid>,
    pub weight: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateProxyNodeAssignment {
    pub proxy_node_id: Option<Uuid>,
    pub scope_type: Option<ConnConfScope>,
    pub scope_id: Option<Uuid>,
    pub weight: Option<i32>,
    pub enabled: Option<bool>,
}
