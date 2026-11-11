use crate::contexts::playbooks::domain::{
    LaunchSpec, PlaybookAssignment, PlaybookExecutionEnvironment, PlaybookRun, PlaybookRunGroup,
    PlaybookSchedule, PlaybookServicePrincipal, PlaybookTargetPreviewItem, PlaybookTargetSet,
    PlaybookTargetSetMember, ScheduleTargets, parse_assignment_scope_kind, parse_cancel_outcome,
    parse_run_status, parse_run_trigger_kind, parse_schedule_concurrency_policy,
    parse_schedule_misfire_policy, parse_service_principal_status, parse_target_set_mode,
};
use crate::errors::AppError;
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row};

pub(super) fn read<T>(row: &PgRow, column: &'static str) -> Result<T, AppError>
where
    for<'r> T: sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(column)
        .map_err(|error| AppError::Database(format!("Failed to read {column}: {error}")))
}

pub(super) fn execution_environment_from_row(
    row: &PgRow,
) -> Result<PlaybookExecutionEnvironment, AppError> {
    Ok(PlaybookExecutionEnvironment {
        id: read(row, "id")?,
        name: read(row, "name")?,
        description: read(row, "description")?,
        image: read(row, "image")?,
        pull_policy: read(row, "pull_policy")?,
        runner_group: read(row, "runner_group")?,
        enabled: read(row, "enabled")?,
        env: read(row, "env")?,
        settings: read(row, "settings")?,
        metadata: read(row, "metadata")?,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
    })
}

pub(super) fn target_set_from_row(row: &PgRow) -> Result<PlaybookTargetSet, AppError> {
    let mode: String = read(row, "mode")?;
    Ok(PlaybookTargetSet {
        id: read(row, "id")?,
        name: read(row, "name")?,
        mode: parse_target_set_mode(&mode)?,
        entity_kind: read(row, "entity_kind")?,
        selector: read(row, "selector")?,
        members: None,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
    })
}

pub(super) fn target_set_member_from_row(row: &PgRow) -> Result<PlaybookTargetSetMember, AppError> {
    Ok(PlaybookTargetSetMember {
        target_kind: read(row, "target_kind")?,
        target_id: read(row, "target_id")?,
        source_kind: read(row, "source_kind")?,
        source_id: read(row, "source_id")?,
        sort_order: read(row, "sort_order")?,
    })
}

pub(super) fn target_preview_item_from_row(
    row: &PgRow,
) -> Result<PlaybookTargetPreviewItem, AppError> {
    Ok(PlaybookTargetPreviewItem {
        target_kind: read(row, "target_kind")?,
        target_id: read(row, "target_id")?,
        source_kind: read(row, "source_kind")?,
        source_id: read(row, "source_id")?,
        name: read(row, "name")?,
        address: read(row, "address")?,
        address_bound: read(row, "address_bound")?,
        status: read(row, "status")?,
        site_id: read(row, "site_id")?,
        site_name: read(row, "site_name")?,
        region_id: read(row, "region_id")?,
        region_name: read(row, "region_name")?,
        tenant_id: read(row, "tenant_id")?,
        tenant_name: read(row, "tenant_name")?,
        role_id: read(row, "role_id")?,
        role_name: read(row, "role_name")?,
        platform_id: read(row, "platform_id")?,
        platform_name: read(row, "platform_name")?,
        device_type_id: read(row, "device_type_id")?,
        device_type_name: read(row, "device_type_name")?,
        cluster_id: read(row, "cluster_id")?,
        cluster_name: read(row, "cluster_name")?,
    })
}

pub(super) fn assignment_from_row(row: &PgRow) -> Result<PlaybookAssignment, AppError> {
    let scope_kind: String = read(row, "scope_kind")?;
    Ok(PlaybookAssignment {
        id: read(row, "id")?,
        job_template_id: read(row, "job_template_id")?,
        target_set_id: read(row, "target_set_id")?,
        name: read(row, "name")?,
        scope_kind: parse_assignment_scope_kind(&scope_kind)?,
        scope_id: read(row, "scope_id")?,
        priority: read(row, "priority")?,
        enabled: read(row, "enabled")?,
        visible: read(row, "visible")?,
        executable: read(row, "executable")?,
        quick_action_enabled: read(row, "quick_action_enabled")?,
        default_input: read(row, "default_input")?,
        default_metadata: read(row, "default_metadata")?,
        default_timeout_seconds: read(row, "default_timeout_seconds")?,
        default_target_limit: read(row, "default_target_limit")?,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
        metadata: read(row, "metadata")?,
    })
}

pub(super) fn schedule_from_row(row: &PgRow) -> Result<PlaybookSchedule, AppError> {
    let misfire_policy: String = read(row, "misfire_policy")?;
    let concurrency_policy: String = read(row, "concurrency_policy")?;
    let targets: ScheduleTargets = serde_json::from_value(read(row, "targets")?)?;
    Ok(PlaybookSchedule {
        id: read(row, "id")?,
        template_id: read(row, "template_id")?,
        targets,
        overrides: serde_json::from_value(read(row, "overrides")?)?,
        owner_user_id: read(row, "owner_user_id")?,
        name: read(row, "name")?,
        cron_expression: read(row, "cron_expression")?,
        timezone: read(row, "timezone")?,
        misfire_policy: parse_schedule_misfire_policy(&misfire_policy)?,
        concurrency_policy: parse_schedule_concurrency_policy(&concurrency_policy)?,
        enabled: read(row, "enabled")?,
        next_run_at: read(row, "next_run_at")?,
        last_run_at: read(row, "last_run_at")?,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
        metadata: read(row, "metadata")?,
    })
}

pub(super) fn service_principal_from_row(
    row: &PgRow,
) -> Result<PlaybookServicePrincipal, AppError> {
    let status: String = read(row, "status")?;
    Ok(PlaybookServicePrincipal {
        id: read(row, "id")?,
        name: read(row, "name")?,
        slug: read(row, "slug")?,
        description: read(row, "description")?,
        status: parse_service_principal_status(&status)?,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
        revoked_at: read(row, "revoked_at")?,
        revoked_by_user_id: read(row, "revoked_by_user_id")?,
        revocation_reason: read(row, "revocation_reason")?,
        metadata: read(row, "metadata")?,
    })
}

pub(super) fn run_group_from_row(row: &PgRow) -> Result<PlaybookRunGroup, AppError> {
    let trigger_kind: String = read(row, "trigger_kind")?;
    let status: String = read(row, "status")?;
    let cancel_outcome = read::<Option<String>>(row, "cancel_outcome")?
        .as_deref()
        .map(parse_cancel_outcome)
        .transpose()?;
    let launch_spec: LaunchSpec = serde_json::from_value(read(row, "launch_spec")?)?;
    Ok(PlaybookRunGroup {
        id: read(row, "id")?,
        template_id: read(row, "template_id")?,
        launch_spec,
        target_set_id: read(row, "target_set_id")?,
        schedule_id: read(row, "schedule_id")?,
        scheduled_for: read(row, "scheduled_for")?,
        requested_by_user_id: read(row, "requested_by_user_id")?,
        service_principal_id: read(row, "service_principal_id")?,
        trigger_kind: parse_run_trigger_kind(&trigger_kind)?,
        status: parse_run_status(&status)?,
        total_runs: read(row, "total_runs")?,
        succeeded_runs: read(row, "succeeded_runs")?,
        failed_runs: read(row, "failed_runs")?,
        canceled_runs: read(row, "canceled_runs")?,
        timeout_runs: read(row, "timeout_runs")?,
        unreported_runs: read(row, "unreported_runs")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
        started_at: read(row, "started_at")?,
        finished_at: read(row, "finished_at")?,
        lease_owner: read(row, "lease_owner")?,
        lease_expires_at: read(row, "lease_expires_at")?,
        leased_at: read(row, "leased_at")?,
        dispatch_id: read(row, "dispatch_id")?,
        dispatch_attempt: read(row, "dispatch_attempt")?,
        cancel_requested_at: read(row, "cancel_requested_at")?,
        cancel_deadline_at: read(row, "cancel_deadline_at")?,
        cancel_outcome,
        metadata: read(row, "metadata")?,
    })
}

pub(super) fn run_from_row(row: &PgRow) -> Result<PlaybookRun, AppError> {
    let status: String = read(row, "status")?;
    let cancel_outcome = read::<Option<String>>(row, "cancel_outcome")?
        .as_deref()
        .map(parse_cancel_outcome)
        .transpose()?;
    Ok(PlaybookRun {
        id: read(row, "id")?,
        run_group_id: read(row, "run_group_id")?,
        hostname: read(row, "hostname")?,
        target_kind: read(row, "target_kind")?,
        target_id: read(row, "target_id")?,
        source_kind: read(row, "source_kind")?,
        source_id: read(row, "source_id")?,
        target_name: read(row, "target_name")?,
        target_address: read(row, "target_address")?,
        target_address_bound: read(row, "target_address_bound")?,
        credential_account_kind: read(row, "credential_account_kind")?,
        credential_account_id: read(row, "credential_account_id")?,
        credential_account_entry_id: read(row, "credential_account_entry_id")?,
        credential_merge_policy: read(row, "credential_merge_policy")?,
        credential_resolution: read(row, "credential_resolution")?,
        status: parse_run_status(&status)?,
        cancel_requested_at: read(row, "cancel_requested_at")?,
        cancel_outcome,
        lease_owner: read(row, "lease_owner")?,
        lease_expires_at: read(row, "lease_expires_at")?,
        queued_at: read(row, "queued_at")?,
        leased_at: read(row, "leased_at")?,
        started_at: read(row, "started_at")?,
        finished_at: read(row, "finished_at")?,
        exit_code: read(row, "exit_code")?,
        error_message: read(row, "error_message")?,
        attempt: read(row, "attempt")?,
        timeout_seconds: read(row, "timeout_seconds")?,
        metadata: read(row, "metadata")?,
    })
}
