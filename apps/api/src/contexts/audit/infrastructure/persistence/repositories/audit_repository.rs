use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::shared::list_query::{ListColumn, SortSpec};
use crate::errors::AppError;

pub const AUDIT_EVENT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("created_at", "created_at", "audit.events.columns.createdAt"),
    ListColumn::enum_filter(
        "event_type",
        "event_type",
        "event_type",
        "audit.events.columns.eventType",
    ),
    ListColumn::sortable("actor", "actor_id", "audit.events.columns.actor"),
    ListColumn::enum_filter(
        "target_type",
        "target_type",
        "target_type",
        "audit.events.columns.targetType",
    ),
    ListColumn::enum_filter(
        "result",
        "COALESCE(metadata ->> 'decision', metadata ->> 'status', metadata ->> 'result', '')",
        "COALESCE(metadata ->> 'decision', metadata ->> 'status', metadata ->> 'result', '')",
        "audit.events.columns.result",
    ),
    ListColumn::sortable(
        "summary",
        "COALESCE(metadata ->> 'summary', event_type)",
        "audit.events.columns.summary",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct AuditEventFilter {
    pub query: Option<String>,
    pub event_type: Vec<String>,
    pub actor_kind: Vec<String>,
    pub actor_id: Vec<Uuid>,
    pub target_type: Vec<String>,
    pub target_id: Vec<Uuid>,
    pub result: Vec<String>,
    pub mcp_tool: Vec<String>,
    pub mcp_tool_group: Vec<String>,
    pub mcp_protocol: Vec<String>,
    pub mcp_risk: Vec<String>,
    pub created_from: Option<DateTime<Utc>>,
    pub created_to: Option<DateTime<Utc>>,
    pub sort: Option<SortSpec>,
}

#[async_trait]
pub trait AuditRepository: Send + Sync {
    async fn insert(&self, event: AuditEvent) -> Result<(), AppError>;
    async fn list_events(
        &self,
        filter: AuditEventFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<AuditEvent>, AppError>;
    async fn count_events(&self, filter: AuditEventFilter) -> Result<u64, AppError>;
}
