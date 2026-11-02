use crate::contexts::playbooks::domain::{
    Playbook, PlaybookAssignment, PlaybookExecutionEnvironment, PlaybookJobTemplate,
    PlaybookProject, PlaybookRun, PlaybookRunGroup, PlaybookSchedule, PlaybookServicePrincipal,
    PlaybookTargetPreviewItem, PlaybookTargetSet, PlaybookVersion, parse_assignment_scope_kind,
    parse_cancel_outcome, parse_playbook_connection, parse_playbook_language, parse_playbook_risk,
    parse_playbook_runtime, parse_playbook_status, parse_run_status, parse_run_trigger_kind,
    parse_schedule_concurrency_policy, parse_schedule_misfire_policy,
    parse_service_principal_status, parse_target_set_mode,
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

pub(super) fn playbook_from_row(row: &PgRow) -> Result<Playbook, AppError> {
    let status: String = read(row, "status")?;
    let runtime: String = read(row, "runtime")?;
    let risk: String = read(row, "risk")?;
    let connection_default: String = read(row, "connection_default")?;
    Ok(Playbook {
        id: read(row, "id")?,
        slug: read(row, "slug")?,
        name: read(row, "name")?,
        description: read(row, "description")?,
        status: parse_playbook_status(&status)?,
        runtime: parse_playbook_runtime(&runtime)?,
        risk: parse_playbook_risk(&risk)?,
        owner_user_id: read(row, "owner_user_id")?,
        category: read(row, "category")?,
        current_version_id: read(row, "current_version_id")?,
        default_target_set_id: read(row, "default_target_set_id")?,
        source_kind: read(row, "source_kind")?,
        vars_schema: read(row, "vars_schema")?,
        connection_default: parse_playbook_connection(&connection_default)?,
        inventory_rest_enabled: read(row, "inventory_rest_enabled")?,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
        metadata: read(row, "metadata")?,
    })
}

pub(super) fn version_from_row(row: &PgRow) -> Result<PlaybookVersion, AppError> {
    let language: String = read(row, "language")?;
    let runtime: String = read(row, "runtime")?;
    Ok(PlaybookVersion {
        id: read(row, "id")?,
        playbook_id: read(row, "playbook_id")?,
        version: read(row, "version")?,
        language: parse_playbook_language(&language)?,
        runtime: parse_playbook_runtime(&runtime)?,
        entrypoint: read(row, "entrypoint")?,
        content: read(row, "content")?,
        content_sha256: read(row, "content_sha256")?,
        input_schema: read(row, "input_schema")?,
        output_schema: read(row, "output_schema")?,
        token_policy: read(row, "token_policy")?,
        timeout_seconds: read(row, "timeout_seconds")?,
        changelog: read(row, "changelog")?,
        created_by_user_id: read(row, "created_by_user_id")?,
        created_at: read(row, "created_at")?,
        metadata: read(row, "metadata")?,
    })
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

pub(super) fn project_from_row(row: &PgRow) -> Result<PlaybookProject, AppError> {
    Ok(PlaybookProject {
        id: read(row, "id")?,
        name: read(row, "name")?,
        description: read(row, "description")?,
        scm_type: read(row, "scm_type")?,
        scm_url: read(row, "scm_url")?,
        scm_branch: read(row, "scm_branch")?,
        scm_ref: read(row, "scm_ref")?,
        playbook_base_path: read(row, "playbook_base_path")?,
        credential_id: read(row, "credential_id")?,
        default_execution_environment_id: read(row, "default_execution_environment_id")?,
        sync_policy: read(row, "sync_policy")?,
        sync_status: read(row, "sync_status")?,
        last_sync_revision: read(row, "last_sync_revision")?,
        last_sync_error: read(row, "last_sync_error")?,
        last_synced_at: read(row, "last_synced_at")?,
        metadata: read(row, "metadata")?,
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
    })
}

pub(super) fn job_template_from_row(row: &PgRow) -> Result<PlaybookJobTemplate, AppError> {
    Ok(PlaybookJobTemplate {
        id: read(row, "id")?,
        name: read(row, "name")?,
        description: read(row, "description")?,
        playbook_id: read(row, "playbook_id")?,
        project_id: read(row, "project_id")?,
        playbook_path: read(row, "playbook_path")?,
        inventory_target_set_id: read(row, "inventory_target_set_id")?,
        execution_environment_id: read(row, "execution_environment_id")?,
        credential_refs: read(row, "credential_refs")?,
        extra_vars: read(row, "extra_vars")?,
        launch_options: read(row, "launch_options")?,
        ask_variables_on_launch: read(row, "ask_variables_on_launch")?,
        ask_limit_on_launch: read(row, "ask_limit_on_launch")?,
        ask_tags_on_launch: read(row, "ask_tags_on_launch")?,
        ask_skip_tags_on_launch: read(row, "ask_skip_tags_on_launch")?,
        ask_credential_on_launch: read(row, "ask_credential_on_launch")?,
        ask_execution_environment_on_launch: read(row, "ask_execution_environment_on_launch")?,
        enabled: read(row, "enabled")?,
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
        created_by_user_id: read(row, "created_by_user_id")?,
        updated_by_user_id: read(row, "updated_by_user_id")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
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
        playbook_id: read(row, "playbook_id")?,
        target_set_id: read(row, "target_set_id")?,
        default_version_id: read(row, "default_version_id")?,
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
    Ok(PlaybookSchedule {
        id: read(row, "id")?,
        playbook_id: read(row, "playbook_id")?,
        target_set_id: read(row, "target_set_id")?,
        service_principal_id: read(row, "service_principal_id")?,
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
    Ok(PlaybookRunGroup {
        id: read(row, "id")?,
        playbook_id: read(row, "playbook_id")?,
        playbook_version_id: read(row, "playbook_version_id")?,
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
        playbook_id: read(row, "playbook_id")?,
        playbook_version_id: read(row, "playbook_version_id")?,
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
