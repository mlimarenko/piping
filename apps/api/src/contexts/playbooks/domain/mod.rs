//! Playbook domain models and their public re-exports.
pub mod catalog;
pub use catalog::{
    BuiltinCatalog, BuiltinDefinitionField, BuiltinJobLookup, BuiltinUnavailableReason,
    CATALOG_API_VERSION, CATALOG_DEFAULT_ROOT, CATALOG_JOB_NAME_PREFIX, CATALOG_MANIFEST_FILE,
    CATALOG_PROJECT_DESCRIPTION, CATALOG_PROJECT_DISPLAY_NAME, CATALOG_PROJECT_NAME, CatalogEntry,
    CatalogLaunchOptions, CatalogManifest, CatalogProvenance, CatalogTemplateDefinition,
    LoadedSystemCatalog, LoadedSystemTemplate, builtin_definition_change, catalog_entry_id, job_id,
    project_id, revision_id, validate_manifest,
};
pub mod lifecycle_audit;
pub use lifecycle_audit::{
    automation_audit_state, automation_execution_outcome, automation_lifecycle_event,
};
pub mod parameters;
pub mod playbook;
pub mod target_set_facets;
pub mod workspace;
pub use parameters::{
    ParameterOverrides, ResolvedParameters, ResolvedVarsSchema, SECRET_REFERENCE_FORMAT,
    VarsSchemaSource, compile_vars_schema, resolve_parameters, resolve_vars_schema,
    validate_parameter_declarations,
};
pub use target_set_facets::{TargetSetFacet, TargetSetFacetKind, TargetSetFacetPage};
pub use workspace::*;

pub use playbook::{
    JobTokenAudience, PlaybookArtifact, PlaybookAssignment, PlaybookAssignmentScopeKind,
    PlaybookCancelOutcome, PlaybookConnection, PlaybookExecutionEnvironment, PlaybookJobToken,
    PlaybookJobTokenStatus, PlaybookRun, PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup,
    PlaybookRunStatus, PlaybookRunTriggerKind, PlaybookSchedule, PlaybookScheduleConcurrencyPolicy,
    PlaybookScheduleMisfirePolicy, PlaybookServicePrincipal, PlaybookServicePrincipalStatus,
    PlaybookTargetPreviewItem, PlaybookTargetSelectorPreview, PlaybookTargetSet,
    PlaybookTargetSetListItem, PlaybookTargetSetMember, PlaybookTargetSetMode,
    PlaybookTargetSetPreview, PlaybookTargetSetUsage, PlaybookTargetSetUsageRef,
    SCHEDULE_OWNER_ACCESS_REVOKED, parse_assignment_scope_kind, parse_cancel_outcome,
    parse_job_token_status, parse_playbook_connection, parse_run_event_kind, parse_run_status,
    parse_run_trigger_kind, parse_schedule_concurrency_policy, parse_schedule_misfire_policy,
    parse_service_principal_status, parse_target_set_mode,
};
