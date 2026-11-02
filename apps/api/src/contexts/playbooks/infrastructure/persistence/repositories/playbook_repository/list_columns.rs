use crate::contexts::playbooks::domain::{
    PlaybookAssignmentScopeKind, PlaybookRunStatus, PlaybookScheduleConcurrencyPolicy,
    PlaybookServicePrincipalStatus, PlaybookTargetSetMode,
};
use crate::contexts::shared::list_query::{ListColumn, SortSpec};
use uuid::Uuid;

/// Declarative sort columns for playbook-run lists.
///
/// `duration` maps to the computed `finished_at - started_at` interval. The
/// `template` key remains intentionally absent: `playbook_run` carries no
/// denormalized template name, so name-ordering would require a `playbook` join.
pub const PLAYBOOK_RUN_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "started_at",
        "started_at",
        "playbooks.runs.fields.started_at",
    ),
    ListColumn::enum_filter("status", "status", "status", "playbooks.runs.fields.status"),
    ListColumn::sortable(
        "duration",
        "(finished_at - started_at)",
        "playbooks.runs.fields.duration",
    ),
    ListColumn::sortable("total_runs", "total_runs", "playbooks.page.fields.targets"),
];

/// Declarative sort columns for playbook run-group lists.
pub const PLAYBOOK_RUN_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "created_at",
        "created_at",
        "playbooks.service_principals.fields.created_at",
    ),
    ListColumn::fk_filter(
        "playbook",
        "playbook_id",
        "playbook_id",
        "playbook_id",
        "playbooks",
        "playbooks.runs.fields.playbook",
    ),
    ListColumn::sortable(
        "started_at",
        "started_at",
        "playbooks.runs.fields.started_at",
    ),
    ListColumn::enum_filter("status", "status", "status", "playbooks.runs.fields.status"),
    ListColumn::sortable(
        "duration",
        "(finished_at - started_at)",
        "playbooks.runs.fields.duration",
    ),
    ListColumn::sortable("total_runs", "total_runs", "playbooks.page.fields.targets"),
    ListColumn::sortable(
        "succeeded_runs",
        "succeeded_runs",
        "playbooks.runs.status.succeeded",
    ),
    ListColumn::sortable("failed_runs", "failed_runs", "playbooks.runs.status.failed"),
    ListColumn::sortable(
        "canceled_runs",
        "canceled_runs",
        "playbooks.runs.status.canceled",
    ),
    ListColumn::sortable(
        "timeout_runs",
        "timeout_runs",
        "playbooks.runs.status.timeout",
    ),
];

/// Declarative sort/filter columns for playbook assignment lists.
pub const PLAYBOOK_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "playbooks.assignments.fields.name"),
    ListColumn::fk_filter(
        "playbook",
        "playbook_id",
        "playbook_id",
        "playbook_id",
        "playbook",
        "playbooks.assignments.fields.playbook",
    ),
    ListColumn::fk_filter(
        "target_set",
        "target_set_id",
        "target_set_id",
        "target_set_id",
        "playbook_target_set",
        "playbooks.assignments.fields.target_set",
    ),
    ListColumn::enum_filter(
        "scope_kind",
        "scope_kind",
        "scope_kind",
        "playbooks.assignments.fields.scope",
    ),
    ListColumn::sortable(
        "priority",
        "priority",
        "playbooks.assignments.fields.priority",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "playbooks.assignments.fields.enabled",
    ),
    ListColumn::enum_filter(
        "quick_action_enabled",
        "quick_action_enabled",
        "quick_action_enabled::text",
        "playbooks.assignments.fields.quick_action",
    ),
];

/// Declarative sort/filter columns for playbook target-set lists.
pub const PLAYBOOK_TARGET_SET_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "playbooks.target_sets.fields.name"),
    ListColumn::enum_filter("mode", "mode", "mode", "playbooks.target_sets.fields.mode"),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "playbooks.target_sets.fields.updated_at",
    ),
];

/// Declarative sort columns for playbook-schedule lists.
pub const PLAYBOOK_SCHEDULE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "playbooks.schedules.fields.name"),
    ListColumn::fk_filter(
        "playbook",
        "playbook_id",
        "playbook_id",
        "playbook_id",
        "playbook",
        "playbooks.schedules.fields.playbook",
    ),
    ListColumn::sortable(
        "playbook_id",
        "playbook_id",
        "playbooks.schedules.fields.playbook",
    ),
    ListColumn::fk_filter(
        "target_set",
        "target_set_id",
        "target_set_id",
        "target_set_id",
        "playbook_target_set",
        "playbooks.schedules.fields.target_set",
    ),
    ListColumn::sortable(
        "target_set_id",
        "target_set_id",
        "playbooks.schedules.fields.target_set",
    ),
    ListColumn::sortable("cron", "cron_expression", "playbooks.schedules.fields.cron"),
    ListColumn::sortable(
        "cron_expression",
        "cron_expression",
        "playbooks.schedules.fields.cron",
    ),
    ListColumn::sortable(
        "timezone",
        "timezone",
        "playbooks.schedules.fields.timezone",
    ),
    ListColumn::enum_filter_with_param(
        "concurrency",
        "concurrency_policy",
        "concurrency_policy",
        "concurrency_policy",
        "playbooks.schedules.fields.concurrency",
    ),
    ListColumn::sortable(
        "concurrency_policy",
        "concurrency_policy",
        "playbooks.schedules.fields.concurrency",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "playbooks.schedules.fields.enabled",
    ),
];

/// Declarative sort columns for playbook service-principal lists.
pub const PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "playbooks.service_principals.fields.name"),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "playbooks.service_principals.fields.status",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "playbooks.service_principals.fields.created_at",
    ),
];

/// List parameters for playbook runs: the existing `playbook_id` filter, a
/// multi-value `status` IN filter, and an allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct PlaybookRunListFilter {
    pub playbook_id: Vec<Uuid>,
    /// Multi-value `status` IN filter (matches any).
    pub status: Vec<PlaybookRunStatus>,
    pub sort: Option<SortSpec>,
}

/// List parameters for playbook target sets: a multi-value `mode` IN filter and
/// an allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct PlaybookTargetSetListFilter {
    pub mode: Vec<PlaybookTargetSetMode>,
    pub sort: Option<SortSpec>,
}

/// List parameters for playbook assignments: FK filters and an allowlisted sort
/// directive.
#[derive(Debug, Clone, Default)]
pub struct PlaybookAssignmentListFilter {
    pub playbook_id: Vec<Uuid>,
    pub target_set_id: Vec<Uuid>,
    pub scope_kind: Vec<PlaybookAssignmentScopeKind>,
    pub enabled: Vec<bool>,
    pub quick_action_enabled: Vec<bool>,
    pub sort: Option<SortSpec>,
}

/// List parameters for playbook schedules: the existing `playbook_id` filter, a
/// target-set FK filter, multi-value status filters, and an allowlisted sort
/// directive.
#[derive(Debug, Clone, Default)]
pub struct PlaybookScheduleListFilter {
    pub playbook_id: Vec<Uuid>,
    pub target_set_id: Vec<Uuid>,
    pub concurrency_policy: Vec<PlaybookScheduleConcurrencyPolicy>,
    /// Multi-value `enabled` IN filter (matches any).
    pub enabled: Vec<bool>,
    pub sort: Option<SortSpec>,
}

/// List parameters for playbook service principals: a multi-value `status` IN
/// filter and an allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct PlaybookServicePrincipalListFilter {
    /// Multi-value `status` IN filter (matches any).
    pub status: Vec<PlaybookServicePrincipalStatus>,
    pub sort: Option<SortSpec>,
}
