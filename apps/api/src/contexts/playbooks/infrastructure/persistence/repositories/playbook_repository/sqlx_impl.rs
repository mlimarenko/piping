use super::callback_tx::{
    CallbackUsage, append_run_event_tx, append_run_group_event_tx, append_run_group_events_tx,
    authorize_group_callback_tx, authorize_group_completion_callback_tx, authorize_run_callback_tx,
    charge_group_callback_quota_tx, charge_run_callback_quota_tx, complete_run_group_batch_tx,
    complete_run_tx,
};
use super::cancel_tx::{
    append_cancel_finalized_events_tx, audit_terminal_runs_tx, record_canceled_run_metrics,
    request_run_group_cancel_tx, revoke_terminal_callback_tokens_tx, rollup_run_groups_tx,
};
use super::event_rows::{artifact_from_row, job_token_from_row, run_event_from_row};
use super::preview::{
    preview_dynamic_target_set, preview_static_target_set, preview_target_selector,
    target_belongs_to_target_set,
};
use super::row_mappers::{
    assignment_from_row, execution_environment_from_row, read, run_from_row, run_group_from_row,
    schedule_from_row, service_principal_from_row, target_preview_item_from_row,
    target_set_from_row, target_set_member_from_row,
};
use super::run_metadata::{
    launch_options_metadata, run_credential_decision_from_metadata, run_credential_metadata,
    run_timeout_policy_metadata, run_timeout_seconds_for_target_count,
};
use super::scheduling::apply_scheduled_run_concurrency_policy;
use super::selector::{prune_role_from_selector, run_event_host};
use super::sql_strings::{
    ARTIFACT_COLUMNS, ASSIGNMENT_COLUMNS, ASSIGNMENT_COLUMNS_A, EXECUTION_ENVIRONMENT_COLUMNS,
    JOB_TOKEN_COLUMNS, RUN_COLUMNS, RUN_EVENT_COLUMNS, RUN_GROUP_COLUMNS, SCHEDULE_COLUMNS,
    SERVICE_PRINCIPAL_COLUMNS, TARGET_PREVIEW_BASE_SQL, TARGET_SET_COLUMNS,
};
use super::target_set_lock;
use super::target_set_summary;
use super::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunGroupBatchRecord,
    CompleteRunRecord, CompletedRunGroupBatchRecord, CompletedRunRecord, CreateArtifactRecord,
    CreateAssignmentRecord, CreateExecutionEnvironmentRecord, CreateJobTemplateRecord,
    CreateJobTokenRecord, CreateProjectRecord, CreateRunEventRecord, CreateRunGroupEventRecord,
    CreateRunGroupRecord, CreateScheduleRecord,
    CreateTargetSetRecord, CreatedRunGroupRecord, DueScheduleRecord, FinalizeRunGroupCancelRecord,
    FinalizedRunGroupCancelRecord, LeaseQueuedRunsRecord, LeaseScheduleOccurrencesRecord,
    LeasedScheduleOccurrenceRecord, PLAYBOOK_ASSIGNMENT_COLUMNS, PLAYBOOK_RUN_COLUMNS,
    PLAYBOOK_RUN_GROUP_COLUMNS, PLAYBOOK_SCHEDULE_COLUMNS,
    PLAYBOOK_TARGET_SET_COLUMNS, PlaybookAssignmentListFilter, PlaybookRepository,
    PlaybookRunListFilter, PlaybookScheduleListFilter,
    PlaybookTargetSetListFilter, ReleaseScheduleOccurrenceRecord,
    RunGroupWithRuns, SetActiveRunTimeoutsRecord, SqlxPlaybookRepository, StartLeasedRunsRecord,
    StartedRunRecord, TargetAssignmentRecord, TargetAssignmentsRecord, TimedOutRunsRecord,
    TimeoutStuckRunsRecord, UpdateAssignmentRecord, UpdateScheduleRecord, UpdateTargetSetRecord,
    page_result,
};
use crate::contexts::audit::domain::AuditEmitterKind;
use crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupEventListFilter;
use crate::contexts::playbooks::domain::{
    InventorySelection, JobTokenAudience, PlaybookArtifact, PlaybookAssignment,
    PlaybookCancelOutcome, PlaybookExecutionEnvironment, PlaybookJobToken, PlaybookRun,
    PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup, PlaybookRunStatus, PlaybookSchedule,
    PlaybookServicePrincipal, PlaybookTargetPreviewItem, PlaybookTargetSelectorPreview,
    PlaybookTargetSet, PlaybookTargetSetListItem, PlaybookTargetSetMember, PlaybookTargetSetMode,
    PlaybookTargetSetPreview, PlaybookTargetSetUsage,
};
use crate::contexts::secrets::application::{
    PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE, PLAYBOOK_PROJECT_SCM_CREDENTIAL_FIELD_KEY,
};
use crate::contexts::secrets::domain::SECRET_KIND_NAMED;
use crate::contexts::shared::delete_guard::{
    BlockerTableMetadata, delete_record_guarded_in_transaction, no_blocker_metadata,
};
use crate::contexts::shared::list_query::{
    WhereState, push_order_by, push_search_ilike, push_string_in, push_uuid_in, search_pattern,
};
use crate::contexts::shared::pagination::{PageResult, clamp_limit, clamp_offset};
use crate::errors::{
    AppError, DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup,
    classify_sqlx_error_owned,
};
use async_trait::async_trait;
use piping_contracts::playbook_runner::{ConsoleAck, ConsolePage, RunnerConsoleBatch};
use serde_json::Value;
use serde_json::json;
use sqlx::{Postgres, QueryBuilder, Row};
use std::collections::HashMap;
use uuid::Uuid;

/// Display metadata for the shared FK delete guard: assignments are the only
/// delete-blocking foreign key into a target set, and the UI deep-links them.
fn target_set_blocker_metadata(table: &str) -> BlockerTableMetadata {
    match table {
        "playbook_assignment" => BlockerTableMetadata {
            entity_key: Some("playbook_assignment"),
            ..BlockerTableMetadata::default()
        },
        _ => BlockerTableMetadata::default(),
    }
}

/// One DELETE_BLOCKED report from the set's identity references. Job templates
/// and schedules hold the reference inside JSON, so no foreign key can block
/// the delete for them; reporting them here keeps a single 409 complete.
fn target_set_usage_blockers(usage: PlaybookTargetSetUsage) -> DeleteBlockedDetails {
    let groups = [
        ("playbook_job_template", "job_template", usage.job_templates),
        ("playbook_assignment", "assignment", usage.assignments),
        ("playbook_schedule", "schedule", usage.schedules),
    ];
    let mut blockers = Vec::new();
    let mut total_count = 0_i64;
    for (table, entity_key, references) in groups {
        if references.is_empty() {
            continue;
        }
        let count = references.len() as i64;
        total_count += count;
        let examples = references
            .iter()
            .take(3)
            .map(|reference| DeleteBlockedExample {
                id: Some(reference.id),
                label: reference
                    .name
                    .clone()
                    .unwrap_or_else(|| reference.id.to_string()),
            })
            .collect();
        blockers.push(DeleteBlockedGroup {
            table: table.to_string(),
            entity_key: Some(entity_key.to_string()),
            count,
            examples,
        });
    }
    blockers.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.table.cmp(&right.table))
    });
    DeleteBlockedDetails {
        total_count,
        blockers,
    }
}

#[async_trait]
impl PlaybookRepository for SqlxPlaybookRepository {
    async fn seed_system_catalog(
        &self,
        record: super::SeedSystemCatalogRecord,
    ) -> Result<super::SeededSystemCatalog, AppError> {
        super::catalog_seed::seed_system_catalog(&self.pool, record).await
    }

    async fn get_project_revision(
        &self,
        project_id: Uuid,
        revision_id: Uuid,
    ) -> Result<Option<crate::contexts::playbooks::domain::ProjectRevision>, AppError> {
        super::workspace_revision::get(&self.pool, project_id, revision_id).await
    }

    async fn change_project_revision(
        &self,
        project_id: Uuid,
        base_revision_id: Uuid,
        changes: Vec<crate::contexts::playbooks::domain::RevisionChange>,
        created_by: Option<Uuid>,
    ) -> Result<crate::contexts::playbooks::domain::ProjectRevision, AppError> {
        super::workspace_revision::change(
            &self.pool,
            project_id,
            base_revision_id,
            changes,
            created_by,
        )
        .await
    }

    async fn import_project_revision(
        &self,
        project_id: Uuid,
        base_revision_id: Option<Uuid>,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
        source_commit_oid: String,
        entries: Vec<crate::contexts::playbooks::domain::ProjectEntry>,
        created_by: Option<Uuid>,
    ) -> Result<crate::contexts::playbooks::domain::ProjectRevision, AppError> {
        super::workspace_revision::import(
            &self.pool,
            project_id,
            base_revision_id,
            expected_updated_at,
            source_commit_oid,
            entries,
            created_by,
        )
        .await
    }

    async fn fork_project(
        &self,
        project_id: Uuid,
        revision_id: Uuid,
        name: String,
        created_by: Option<Uuid>,
    ) -> Result<crate::contexts::playbooks::domain::Project, AppError> {
        super::workspace_revision::fork(&self.pool, project_id, revision_id, name, created_by).await
    }
    async fn list_execution_environments(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookExecutionEnvironment>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!(
            "SELECT {EXECUTION_ENVIRONMENT_COLUMNS} FROM playbook_execution_environment ORDER BY name ASC, id LIMIT $1 OFFSET $2"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(execution_environment_from_row)
            .collect::<Result<_, _>>()?;
        let total =
            sqlx::query_scalar("SELECT COUNT(*)::bigint FROM playbook_execution_environment")
                .fetch_one(&self.pool)
                .await
                .map_err(classify_sqlx_error_owned)?;
        page_result(
            items,
            total,
            limit,
            offset,
            "Playbook execution environment",
        )
    }

    async fn get_execution_environment(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookExecutionEnvironment>, AppError> {
        let sql = format!(
            "SELECT {EXECUTION_ENVIRONMENT_COLUMNS} FROM playbook_execution_environment WHERE id = $1"
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(execution_environment_from_row).transpose()
    }

    async fn create_execution_environment(
        &self,
        record: CreateExecutionEnvironmentRecord,
    ) -> Result<PlaybookExecutionEnvironment, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_execution_environment (
                name, description, image, pull_policy, runner_group, enabled,
                env, settings, metadata, created_by_user_id, updated_by_user_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
            RETURNING {EXECUTION_ENVIRONMENT_COLUMNS}
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.name)
            .bind(record.description)
            .bind(record.image)
            .bind(record.pull_policy)
            .bind(record.runner_group)
            .bind(record.enabled)
            .bind(record.env)
            .bind(record.settings)
            .bind(record.metadata)
            .bind(record.user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        execution_environment_from_row(&row)
    }

    async fn list_projects(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<crate::contexts::playbooks::domain::Project>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = "SELECT * FROM playbook_project ORDER BY name ASC, id LIMIT $1 OFFSET $2";
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(super::workspace_revision::project_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM playbook_project")
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook project")
    }

    async fn get_project(
        &self,
        id: Uuid,
    ) -> Result<Option<crate::contexts::playbooks::domain::Project>, AppError> {
        let row = sqlx::query("SELECT * FROM playbook_project WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref()
            .map(super::workspace_revision::project_from_row)
            .transpose()
    }

    async fn create_project(
        &self,
        record: CreateProjectRecord,
    ) -> Result<crate::contexts::playbooks::domain::Project, AppError> {
        super::workspace_revision::create_project(&self.pool, record).await
    }

    async fn list_job_templates(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<crate::contexts::playbooks::domain::JobTemplate>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = "SELECT * FROM playbook_job_template ORDER BY name ASC, id LIMIT $1 OFFSET $2";
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(super::workspace_revision::template_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM playbook_job_template")
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook job template")
    }

    async fn get_job_template(
        &self,
        id: Uuid,
    ) -> Result<Option<crate::contexts::playbooks::domain::JobTemplate>, AppError> {
        let sql = "SELECT * FROM playbook_job_template WHERE id = $1";
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref()
            .map(super::workspace_revision::template_from_row)
            .transpose()
    }

    async fn create_job_template(
        &self,
        record: CreateJobTemplateRecord,
    ) -> Result<crate::contexts::playbooks::domain::JobTemplate, AppError> {
        super::workspace_revision::create_template(&self.pool, record).await
    }

    async fn list_job_templates_for_project(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<crate::contexts::playbooks::domain::JobTemplate>, AppError> {
        let rows = sqlx::query(
            "SELECT * FROM playbook_job_template WHERE project_id = $1 ORDER BY name ASC, id",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        rows.iter()
            .map(super::workspace_revision::template_from_row)
            .collect()
    }

    async fn update_execution_environment(
        &self,
        id: Uuid,
        record: CreateExecutionEnvironmentRecord,
    ) -> Result<Option<PlaybookExecutionEnvironment>, AppError> {
        let sql = format!(
            r#"
            UPDATE playbook_execution_environment
            SET name = $2, description = $3, image = $4, pull_policy = $5,
                runner_group = $6, enabled = $7, env = $8, settings = $9,
                metadata = $10, updated_by_user_id = COALESCE($11, updated_by_user_id),
                updated_at = now()
            WHERE id = $1
            RETURNING {EXECUTION_ENVIRONMENT_COLUMNS}
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(record.name)
            .bind(record.description)
            .bind(record.image)
            .bind(record.pull_policy)
            .bind(record.runner_group)
            .bind(record.enabled)
            .bind(record.env)
            .bind(record.settings)
            .bind(record.metadata)
            .bind(record.user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(execution_environment_from_row).transpose()
    }

    async fn delete_execution_environment(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM playbook_execution_environment WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn count_execution_environment_references(&self, id: Uuid) -> Result<i64, AppError> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM playbook_job_template WHERE execution_environment_id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Ok(count)
    }

    async fn update_project(
        &self,
        id: Uuid,
        record: CreateProjectRecord,
    ) -> Result<Option<crate::contexts::playbooks::domain::Project>, AppError> {
        let secret = super::workspace_revision::project_secret(&record.source);
        let source = serde_json::to_value(record.source)?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let row = sqlx::query("UPDATE playbook_project SET name=$2, description=$3, current_revision_id=CASE WHEN source->>'kind'='git' AND (source->>'url' IS DISTINCT FROM $4->>'url' OR source->>'reference' IS DISTINCT FROM $4->>'reference') THEN NULL ELSE current_revision_id END, source=$4, updated_by_user_id=$5, updated_at=now() WHERE id=$1 AND source->>'kind'=$4->>'kind' RETURNING *")
            .bind(id).bind(record.name).bind(record.description).bind(source).bind(record.user_id)
            .fetch_optional(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if row.is_some() {
            reconcile_project_credential_binding_tx(&mut tx, id, secret).await?;
        }
        let project = row
            .as_ref()
            .map(super::workspace_revision::project_from_row)
            .transpose()?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(project)
    }

    async fn delete_project(&self, id: Uuid) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        // Lock the project row before sweeping its bindings. A concurrent
        // `update_project` updates this row before reconciling its usage
        // binding, so without the lock the sweep can miss that not-yet-visible
        // insert and the project is still deleted, stranding an orphan
        // `secret_usage_binding` that permanently blocks the named secret.
        // Mirrors `delete_alert_source`.
        let project_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM playbook_project WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if project_id.is_none() {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Ok(false);
        }

        delete_project_usage_bindings_tx(&mut tx, id).await?;
        let result = sqlx::query("DELETE FROM playbook_project WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn count_job_templates_for_project(&self, id: Uuid) -> Result<i64, AppError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM playbook_job_template WHERE project_id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(classify_sqlx_error_owned)?;
        Ok(count)
    }

    async fn update_job_template(
        &self,
        id: Uuid,
        record: CreateJobTemplateRecord,
    ) -> Result<Option<crate::contexts::playbooks::domain::JobTemplate>, AppError> {
        super::workspace_revision::update_template(&self.pool, id, record).await
    }

    async fn delete_job_template(&self, id: Uuid) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let template_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM playbook_job_template WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if template_id.is_none() {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Ok(false);
        }
        super::workspace_revision::delete_template_secret_usage_tx(&mut tx, id).await?;
        let deleted = delete_record_guarded_in_transaction(
            &mut tx,
            "playbook_job_template",
            id,
            no_blocker_metadata,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(deleted > 0)
    }

    async fn create_target_set(
        &self,
        record: CreateTargetSetRecord,
        preview_limit: i64,
    ) -> Result<PlaybookTargetSetPreview, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let sql = format!(
            r#"
            INSERT INTO playbook_target_set (
                name, mode, entity_kind, selector, created_by_user_id, updated_by_user_id
            )
            VALUES ($1, $2, 'connection_target', $3, $4, $4)
            RETURNING {TARGET_SET_COLUMNS}
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.name)
            .bind(record.mode.as_str())
            .bind(record.selector)
            .bind(record.user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let target_set = target_set_from_row(&row)?;

        for member in record.members {
            sqlx::query(
                r#"
                INSERT INTO playbook_target_set_member (
                    target_set_id, target_kind, target_id, source_kind, source_id,
                    sort_order, added_by_user_id
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (target_set_id, target_kind, target_id, source_kind, source_id)
                DO UPDATE SET sort_order = EXCLUDED.sort_order
                "#,
            )
            .bind(target_set.id)
            .bind(member.target_kind)
            .bind(member.target_id)
            .bind(member.source_kind)
            .bind(member.source_id)
            .bind(member.sort_order)
            .bind(record.user_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        if target_set.mode == PlaybookTargetSetMode::Static {
            preview_static_target_set(&self.pool, target_set, preview_limit).await
        } else {
            preview_dynamic_target_set(&self.pool, target_set, preview_limit).await
        }
    }

    async fn update_target_set(
        &self,
        record: UpdateTargetSetRecord,
    ) -> Result<Option<PlaybookTargetSet>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let sql = format!(
            r#"
            UPDATE playbook_target_set
            SET name = $1, mode = $2, entity_kind = $3, selector = $4,
                updated_by_user_id = $5, updated_at = now()
            WHERE id = $6
            RETURNING {TARGET_SET_COLUMNS}
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(&record.name)
            .bind(record.mode.as_str())
            .bind(&record.entity_kind)
            .bind(&record.selector)
            .bind(record.user_id)
            .bind(record.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let target_set = target_set_from_row(&row)?;

        if let Some(members) = record.members {
            sqlx::query("DELETE FROM playbook_target_set_member WHERE target_set_id = $1")
                .bind(target_set.id)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            for member in members {
                sqlx::query(
                    r#"
                    INSERT INTO playbook_target_set_member (
                        target_set_id, target_kind, target_id, source_kind, source_id,
                        sort_order, added_by_user_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (target_set_id, target_kind, target_id, source_kind, source_id)
                    DO UPDATE SET sort_order = EXCLUDED.sort_order
                    "#,
                )
                .bind(target_set.id)
                .bind(member.target_kind)
                .bind(member.target_id)
                .bind(member.source_kind)
                .bind(member.source_id)
                .bind(member.sort_order)
                .bind(record.user_id)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            }
        }

        if target_set.mode == PlaybookTargetSetMode::Static {
            let member_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM playbook_target_set_member WHERE target_set_id = $1",
            )
            .bind(target_set.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            if member_count == 0 {
                tx.rollback().await.map_err(classify_sqlx_error_owned)?;
                return Err(AppError::Validation(
                    "Static target set requires at least one member".to_string(),
                ));
            }
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.target_set_counts.invalidate(record.id);
        Ok(Some(target_set))
    }

    async fn delete_target_set(&self, id: Uuid) -> Result<bool, AppError> {
        // Job templates and schedules reference a target set inside JSON, not
        // through a foreign key, so the catalog-driven guard cannot see them.
        // The delete lock makes the usage read and the DELETE one unit: a
        // reference writer holds `FOR SHARE` until its commit, so its row is in
        // this snapshot, and no new writer can pass its existence check after
        // the DELETE. The guarded delete stays the backstop for assignments
        // (ON DELETE RESTRICT) and any future FK.
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        if !target_set_lock::lock_target_set_for_delete_tx(&mut tx, id).await? {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Ok(false);
        }
        let usage = target_set_summary::usage_by_target_set_tx(&mut tx, &[id]).await?;
        let details = target_set_usage_blockers(usage.into_values().next().unwrap_or_default());
        if details.total_count > 0 {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Err(AppError::DeleteBlocked(details));
        }
        let deleted = delete_record_guarded_in_transaction(
            &mut tx,
            "playbook_target_set",
            id,
            target_set_blocker_metadata,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.target_set_counts.invalidate(id);
        Ok(deleted > 0)
    }

    async fn prune_deleted_role_from_target_sets(
        &self,
        role_id: Uuid,
        role_name: &str,
    ) -> Result<u64, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let rows = sqlx::query(
            r#"
            SELECT id, selector
            FROM playbook_target_set
            WHERE selector ? 'role' OR selector ? 'role_ids'
            "#,
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let mut updated = 0;
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
            let selector: Value = row.try_get("selector").map_err(classify_sqlx_error_owned)?;
            let pruned = prune_role_from_selector(&selector, role_id, role_name);
            if pruned == selector {
                continue;
            }
            let result = sqlx::query(
                r#"
                UPDATE playbook_target_set
                SET selector = $2, updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(pruned)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            self.target_set_counts.invalidate(id);
            updated += result.rows_affected();
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(updated)
    }

    async fn list_target_sets(
        &self,
        filter: &PlaybookTargetSetListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookTargetSetListItem>, i64), AppError> {
        let sql = format!(
            "SELECT {TARGET_SET_COLUMNS}, COUNT(*) OVER() AS total_count FROM playbook_target_set"
        );
        let mut builder = QueryBuilder::new(sql);
        let mut where_state = WhereState::new();
        let modes: Vec<String> = filter
            .mode
            .iter()
            .map(|mode| mode.as_str().to_string())
            .collect();
        push_string_in(&mut builder, &mut where_state, "mode", &modes);
        let search = search_pattern(filter.search.as_deref());
        push_search_ilike(&mut builder, &mut where_state, &["name"], search.as_deref());
        push_order_by(
            &mut builder,
            PLAYBOOK_TARGET_SET_COLUMNS,
            filter.sort.as_ref(),
            "updated_at DESC, name ASC",
        );
        if filter.sort.is_some() {
            builder.push(", id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        // The window function repeats the unpaged match count on every row; an
        // empty page means there is nothing to count.
        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|error| {
                AppError::Database(format!("Failed to read target set total_count: {error}"))
            })?,
            None => 0,
        };
        let target_sets = rows
            .iter()
            .map(target_set_from_row)
            .collect::<Result<Vec<_>, _>>()?;

        let ids: Vec<Uuid> = target_sets.iter().map(|target_set| target_set.id).collect();
        let mut usage = target_set_summary::usage_by_target_set(&self.pool, &ids).await?;
        let counts =
            target_set_summary::resolved_counts(&self.pool, &self.target_set_counts, &target_sets)
                .await;
        let user_ids: Vec<Uuid> = target_sets
            .iter()
            .flat_map(|target_set| [target_set.created_by_user_id, target_set.updated_by_user_id])
            .flatten()
            .collect();
        let usernames = target_set_summary::usernames_by_id(&self.pool, &user_ids).await?;

        let items = target_sets
            .into_iter()
            .map(|target_set| {
                let created_by_username = target_set
                    .created_by_user_id
                    .and_then(|id| usernames.get(&id).cloned());
                let updated_by_username = target_set
                    .updated_by_user_id
                    .and_then(|id| usernames.get(&id).cloned());
                let resolved_count = counts.get(&target_set.id).copied().flatten();
                let target_set_usage = usage.remove(&target_set.id).unwrap_or_default();
                PlaybookTargetSetListItem {
                    target_set,
                    resolved_count,
                    usage: target_set_usage,
                    created_by_username,
                    updated_by_username,
                }
            })
            .collect();
        Ok((items, total))
    }

    async fn get_target_set(&self, id: Uuid) -> Result<Option<PlaybookTargetSet>, AppError> {
        let sql = format!("SELECT {TARGET_SET_COLUMNS} FROM playbook_target_set WHERE id = $1");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        target_set_from_row(&row).map(Some)
    }

    async fn list_target_set_members(
        &self,
        id: Uuid,
    ) -> Result<Vec<PlaybookTargetSetMember>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT target_kind, target_id, source_kind, source_id, sort_order
            FROM playbook_target_set_member
            WHERE target_set_id = $1
            ORDER BY sort_order ASC, added_at ASC
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        rows.iter().map(target_set_member_from_row).collect()
    }

    async fn preview_target_set(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetSetPreview>, AppError> {
        let sql = format!("SELECT {TARGET_SET_COLUMNS} FROM playbook_target_set WHERE id = $1");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        let target_set = target_set_from_row(&row)?;
        if target_set.mode == PlaybookTargetSetMode::Static {
            preview_static_target_set(&self.pool, target_set, limit)
                .await
                .map(Some)
        } else {
            preview_dynamic_target_set(&self.pool, target_set, limit)
                .await
                .map(Some)
        }
    }

    async fn resolve_target_selector(
        &self,
        selector: &Value,
        limit: i64,
    ) -> Result<Vec<PlaybookTargetPreviewItem>, AppError> {
        super::preview::resolve_target_selector(&self.pool, selector, limit).await
    }

    async fn list_target_set_facets(
        &self,
        kind: crate::contexts::playbooks::domain::TargetSetFacetKind,
        selector: &Value,
        search: &str,
        limit: i64,
        offset: i64,
    ) -> Result<crate::contexts::playbooks::domain::TargetSetFacetPage, AppError> {
        super::target_set_facets::list_target_set_facets(
            &self.pool, kind, selector, search, limit, offset,
        )
        .await
    }

    async fn preview_target_selector(
        &self,
        selector: &Value,
        limit: i64,
    ) -> Result<PlaybookTargetSelectorPreview, AppError> {
        preview_target_selector(&self.pool, selector, limit).await
    }

    async fn create_assignment(
        &self,
        record: CreateAssignmentRecord,
    ) -> Result<PlaybookAssignment, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_assignment (
                job_template_id, target_set_id, name, scope_kind,
                scope_id, priority, enabled, visible, executable, quick_action_enabled,
                default_input, default_metadata, default_timeout_seconds, default_target_limit,
                created_by_user_id, updated_by_user_id, metadata
            )
            SELECT
                template.id,
                target_set.id,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13,
                $14,
                $15,
                $15,
                $16
            FROM playbook_job_template AS template
            JOIN playbook_target_set AS target_set
              ON target_set.id = $2
            WHERE template.id = $1
            RETURNING {ASSIGNMENT_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.job_template_id)
            .bind(record.target_set_id)
            .bind(record.name)
            .bind(record.scope_kind.as_str())
            .bind(record.scope_id)
            .bind(record.priority)
            .bind(record.enabled)
            .bind(record.visible)
            .bind(record.executable)
            .bind(record.quick_action_enabled)
            .bind(record.default_input)
            .bind(record.default_metadata)
            .bind(record.default_timeout_seconds)
            .bind(record.default_target_limit)
            .bind(record.user_id)
            .bind(record.metadata)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound(
                "Job template or target set not found".to_string(),
            ));
        };
        assignment_from_row(&row)
    }

    async fn get_assignment(&self, id: Uuid) -> Result<Option<PlaybookAssignment>, AppError> {
        let sql = format!("SELECT {ASSIGNMENT_COLUMNS} FROM playbook_assignment WHERE id = $1");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        assignment_from_row(&row).map(Some)
    }

    async fn update_assignment(
        &self,
        record: UpdateAssignmentRecord,
    ) -> Result<Option<PlaybookAssignment>, AppError> {
        let replace_name = record.name.is_some();
        let name = record.name.flatten();
        let replace_default_timeout_seconds = record.default_timeout_seconds.is_some();
        let default_timeout_seconds = record.default_timeout_seconds.flatten();
        let replace_default_target_limit = record.default_target_limit.is_some();
        let default_target_limit = record.default_target_limit.flatten();

        let sql = format!(
            r#"
            WITH existing AS (
                SELECT *
                FROM playbook_assignment
                WHERE id = $1
            ),
            candidate AS (
                SELECT
                    COALESCE($2, job_template_id) AS job_template_id,
                    COALESCE($3, target_set_id) AS target_set_id,
                    CASE WHEN $4 THEN $5 ELSE name END AS name,
                    $6 AS scope_kind,
                    $7 AS scope_id,
                    COALESCE($8, priority) AS priority,
                    COALESCE($9, enabled) AS enabled,
                    COALESCE($10, visible) AS visible,
                    COALESCE($11, executable) AS executable,
                    COALESCE($12, quick_action_enabled) AS quick_action_enabled,
                    COALESCE($13, default_input) AS default_input,
                    COALESCE($14, default_metadata) AS default_metadata,
                    CASE WHEN $15 THEN $16 ELSE default_timeout_seconds END AS default_timeout_seconds,
                    CASE WHEN $17 THEN $18 ELSE default_target_limit END AS default_target_limit,
                    COALESCE($19, metadata) AS metadata
                FROM existing
            ),
            validated AS (
                SELECT candidate.*
                FROM candidate
                JOIN playbook_job_template AS template
                  ON template.id = candidate.job_template_id
                JOIN playbook_target_set AS target_set
                  ON target_set.id = candidate.target_set_id
            ),
            updated AS (
                UPDATE playbook_assignment
                SET job_template_id = validated.job_template_id,
                    target_set_id = validated.target_set_id,
                    name = validated.name,
                    scope_kind = validated.scope_kind,
                    scope_id = validated.scope_id,
                    priority = validated.priority,
                    enabled = validated.enabled,
                    visible = validated.visible,
                    executable = validated.executable,
                    quick_action_enabled = validated.quick_action_enabled,
                    default_input = validated.default_input,
                    default_metadata = validated.default_metadata,
                    default_timeout_seconds = validated.default_timeout_seconds,
                    default_target_limit = validated.default_target_limit,
                    metadata = validated.metadata,
                    updated_by_user_id = COALESCE($20, updated_by_user_id),
                    updated_at = NOW()
                FROM validated
                WHERE playbook_assignment.id = $1
                RETURNING playbook_assignment.*
            )
            SELECT {ASSIGNMENT_COLUMNS}
            FROM updated
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.id)
            .bind(record.job_template_id)
            .bind(record.target_set_id)
            .bind(replace_name)
            .bind(name)
            .bind(record.scope_kind.as_str())
            .bind(record.scope_id)
            .bind(record.priority)
            .bind(record.enabled)
            .bind(record.visible)
            .bind(record.executable)
            .bind(record.quick_action_enabled)
            .bind(record.default_input)
            .bind(record.default_metadata)
            .bind(replace_default_timeout_seconds)
            .bind(default_timeout_seconds)
            .bind(replace_default_target_limit)
            .bind(default_target_limit)
            .bind(record.metadata)
            .bind(record.user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(assignment_from_row).transpose()
    }

    async fn delete_assignment(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM playbook_assignment WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_assignments(
        &self,
        filter: &PlaybookAssignmentListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookAssignment>, i64), AppError> {
        let sql = format!(
            "SELECT {ASSIGNMENT_COLUMNS}, COUNT(*) OVER() AS total_count FROM playbook_assignment"
        );
        let mut builder = QueryBuilder::new(sql);
        let mut where_state = WhereState::new();
        push_uuid_in(
            &mut builder,
            &mut where_state,
            "job_template_id",
            &filter.job_template_id,
        );
        push_uuid_in(
            &mut builder,
            &mut where_state,
            "target_set_id",
            &filter.target_set_id,
        );
        if !filter.scope_kind.is_empty() {
            where_state.and(&mut builder);
            builder.push("scope_kind IN (");
            {
                let mut separated = builder.separated(", ");
                for kind in &filter.scope_kind {
                    separated.push_bind(kind.as_str());
                }
            }
            builder.push(")");
        }
        if !filter.enabled.is_empty() {
            where_state.and(&mut builder);
            builder.push("enabled IN (");
            {
                let mut separated = builder.separated(", ");
                for enabled in &filter.enabled {
                    separated.push_bind(*enabled);
                }
            }
            builder.push(")");
        }
        if !filter.quick_action_enabled.is_empty() {
            where_state.and(&mut builder);
            builder.push("quick_action_enabled IN (");
            {
                let mut separated = builder.separated(", ");
                for quick_action_enabled in &filter.quick_action_enabled {
                    separated.push_bind(*quick_action_enabled);
                }
            }
            builder.push(")");
        }
        push_order_by(
            &mut builder,
            PLAYBOOK_ASSIGNMENT_COLUMNS,
            filter.sort.as_ref(),
            "enabled DESC, visible DESC, executable DESC, scope_kind ASC, priority DESC, updated_at DESC, id ASC",
        );
        if filter.sort.is_some() {
            builder.push(", id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        // The window function repeats the unpaged match count on every row; an
        // empty page means there is nothing to count.
        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|error| {
                AppError::Database(format!("Failed to read assignment total_count: {error}"))
            })?,
            None => 0,
        };
        let items = rows
            .iter()
            .map(assignment_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn list_target_assignments(
        &self,
        target_id: Uuid,
        limit: i64,
    ) -> Result<Option<TargetAssignmentsRecord>, AppError> {
        let target_sql = format!(
            r#"
            {TARGET_PREVIEW_BASE_SQL}
            WHERE connection_targets.target_id = $1
            ORDER BY connection_targets.source_kind ASC, connection_targets.name ASC
            LIMIT 1
            "#
        );
        let Some(target_row) = sqlx::query(sqlx::AssertSqlSafe(target_sql))
            .bind(target_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        let target = target_preview_item_from_row(&target_row)?;
        if target.address.is_none() || !target.address_bound {
            return Ok(Some(TargetAssignmentsRecord {
                target,
                items: Vec::new(),
            }));
        }

        let sql = format!(
            r#"
            SELECT {ASSIGNMENT_COLUMNS_A}
            FROM playbook_assignment a
            JOIN playbook_job_template template ON template.id = a.job_template_id
            WHERE a.enabled = TRUE
              AND a.visible = TRUE
              AND a.executable = TRUE
              AND (
                  (a.scope_kind = 'global' AND a.scope_id IS NULL)
                  OR (a.scope_kind = 'connection_target' AND a.scope_id = $1)
                  OR (a.scope_kind = 'role' AND a.scope_id = $2)
                  OR (a.scope_kind = 'site' AND a.scope_id = $3)
                  OR (a.scope_kind = 'region' AND a.scope_id = $4)
                  OR (a.scope_kind = 'tenant' AND a.scope_id = $5)
                  OR (a.scope_kind = 'cluster' AND a.scope_id = $6)
                  OR (a.scope_kind = 'platform' AND a.scope_id = $7)
              )
            ORDER BY a.priority DESC, a.quick_action_enabled DESC, a.updated_at DESC, a.id ASC
            "#
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(target.target_id)
            .bind(target.role_id)
            .bind(target.site_id)
            .bind(target.region_id)
            .bind(target.tenant_id)
            .bind(target.cluster_id)
            .bind(target.platform_id)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;

        let effective_limit = usize::try_from(limit).unwrap_or(0);
        let mut items = Vec::new();
        for row in rows {
            if items.len() >= effective_limit {
                break;
            }
            let assignment = assignment_from_row(&row)?;
            let Some(target_set) = self.get_target_set(assignment.target_set_id).await? else {
                continue;
            };
            if !target_belongs_to_target_set(&self.pool, &target_set, &target).await? {
                continue;
            }
            if let Some(template) = self.get_job_template(assignment.job_template_id).await? {
                items.push(TargetAssignmentRecord {
                    assignment,
                    template,
                });
            }
        }
        Ok(Some(TargetAssignmentsRecord { target, items }))
    }

    async fn create_schedule(
        &self,
        record: CreateScheduleRecord,
    ) -> Result<PlaybookSchedule, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        if let Some(target_set_id) = target_set_lock::schedule_target_set_id(&record.targets) {
            target_set_lock::lock_target_sets_for_reference_tx(&mut tx, &[target_set_id]).await?;
        }
        let sql = format!(
            r#"
            INSERT INTO playbook_schedule (
                template_id, targets, overrides, owner_user_id, name,
                cron_expression, timezone, misfire_policy, concurrency_policy,
                enabled, next_run_at, created_by_user_id, updated_by_user_id, metadata
            )
            SELECT
                template.id,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $12,
                $13
            FROM playbook_job_template AS template
            WHERE template.id = $1
            RETURNING {SCHEDULE_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.template_id)
            .bind(serde_json::to_value(record.targets)?)
            .bind(serde_json::to_value(&record.overrides)?)
            .bind(record.owner_user_id)
            .bind(record.name)
            .bind(record.cron_expression)
            .bind(record.timezone)
            .bind(record.misfire_policy.as_str())
            .bind(record.concurrency_policy.as_str())
            .bind(record.enabled)
            .bind(record.next_run_at)
            .bind(record.user_id)
            .bind(record.metadata)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Err(AppError::NotFound("Job template not found".to_string()));
        };
        let schedule = schedule_from_row(&row)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(schedule)
    }

    async fn get_service_principal(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError> {
        let sql = format!(
            "SELECT {SERVICE_PRINCIPAL_COLUMNS} FROM playbook_service_principal WHERE id = $1"
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(service_principal_from_row)
            .transpose()
    }

    async fn get_schedule(&self, id: Uuid) -> Result<Option<PlaybookSchedule>, AppError> {
        let sql = format!("SELECT {SCHEDULE_COLUMNS} FROM playbook_schedule WHERE id = $1");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(schedule_from_row)
            .transpose()
    }

    async fn list_schedules(
        &self,
        filter: &PlaybookScheduleListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookSchedule>, i64), AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            "SELECT {SCHEDULE_COLUMNS}, COUNT(*) OVER() AS total_count FROM playbook_schedule"
        ));
        let mut where_state = WhereState::new();
        push_uuid_in(
            &mut builder,
            &mut where_state,
            "template_id",
            &filter.template_id,
        );
        let concurrency: Vec<String> = filter
            .concurrency_policy
            .iter()
            .map(|policy| policy.as_str().to_string())
            .collect();
        push_string_in(
            &mut builder,
            &mut where_state,
            "concurrency_policy",
            &concurrency,
        );
        // Multi-value `enabled` IN filter (matches any).
        if !filter.enabled.is_empty() {
            where_state.and(&mut builder);
            builder.push("enabled IN (");
            let mut separated = builder.separated(", ");
            for enabled in &filter.enabled {
                separated.push_bind(*enabled);
            }
            builder.push(")");
        }
        push_order_by(
            &mut builder,
            PLAYBOOK_SCHEDULE_COLUMNS,
            filter.sort.as_ref(),
            "enabled DESC, next_run_at ASC NULLS LAST, updated_at DESC, name ASC",
        );
        if filter.sort.is_some() {
            builder.push(", id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        // The window function repeats the unpaged match count on every row; an
        // empty page means there is nothing to count.
        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|error| {
                AppError::Database(format!("Failed to read schedule total_count: {error}"))
            })?,
            None => 0,
        };
        let items = rows
            .iter()
            .map(schedule_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn list_due_schedules(&self, limit: i64) -> Result<Vec<DueScheduleRecord>, AppError> {
        let sql = format!(
            r#"
            SELECT {SCHEDULE_COLUMNS}, clock_timestamp() AS database_now
            FROM playbook_schedule AS schedule
            WHERE schedule.enabled = TRUE
              AND schedule.next_run_at IS NOT NULL
              AND schedule.next_run_at <= clock_timestamp()
            ORDER BY schedule.next_run_at ASC, schedule.id ASC
            LIMIT $1
            "#
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        rows.iter()
            .map(|row| {
                Ok(DueScheduleRecord {
                    schedule: schedule_from_row(row)?,
                    database_now: read(row, "database_now")?,
                })
            })
            .collect()
    }

    async fn lease_schedule_occurrences(
        &self,
        record: LeaseScheduleOccurrencesRecord,
    ) -> Result<Vec<LeasedScheduleOccurrenceRecord>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let leased_rows = sqlx::query(
            r#"
            WITH candidate AS (
                SELECT occurrence.schedule_id, occurrence.scheduled_for
                FROM playbook_schedule_occurrence AS occurrence
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM playbook_run_group AS run_group
                    WHERE run_group.schedule_id = occurrence.schedule_id
                      AND run_group.scheduled_for = occurrence.scheduled_for
                )
                  AND (
                    occurrence.lease_owner IS NULL
                    OR occurrence.lease_expires_at <= clock_timestamp()
                  )
                ORDER BY occurrence.scheduled_for ASC, occurrence.schedule_id ASC
                LIMIT $1
                FOR UPDATE OF occurrence SKIP LOCKED
            )
            UPDATE playbook_schedule_occurrence AS occurrence
            SET lease_owner = $2,
                lease_expires_at = clock_timestamp() + make_interval(secs => $3::double precision),
                leased_at = clock_timestamp(),
                updated_at = clock_timestamp()
            FROM candidate
            WHERE occurrence.schedule_id = candidate.schedule_id
              AND occurrence.scheduled_for = candidate.scheduled_for
            RETURNING occurrence.schedule_id, occurrence.scheduled_for, occurrence.fired_at
            "#,
        )
        .bind(record.limit)
        .bind(&record.lease_owner)
        .bind(record.lease_ttl_seconds)
        .fetch_all(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let mut occurrences = Vec::with_capacity(record.limit as usize);
        for occurrence_row in &leased_rows {
            let schedule_id: Uuid = read(occurrence_row, "schedule_id")?;
            let schedule_sql =
                format!("SELECT {SCHEDULE_COLUMNS} FROM playbook_schedule WHERE id = $1");
            let schedule_row = sqlx::query(sqlx::AssertSqlSafe(schedule_sql))
                .bind(schedule_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            occurrences.push(LeasedScheduleOccurrenceRecord {
                schedule: schedule_from_row(&schedule_row)?,
                scheduled_for: read(occurrence_row, "scheduled_for")?,
                fired_at: read(occurrence_row, "fired_at")?,
            });
        }

        let remaining = (record.limit - i64::try_from(occurrences.len()).unwrap_or(record.limit))
            .max(0) as usize;
        for advance in record.advances.into_iter().take(remaining) {
            let lock_sql = format!(
                r#"
                SELECT {SCHEDULE_COLUMNS}
                FROM playbook_schedule AS schedule
                WHERE schedule.id = $1
                  AND schedule.enabled = TRUE
                  AND schedule.next_run_at = $2
                  AND schedule.next_run_at <= clock_timestamp()
                FOR UPDATE OF schedule SKIP LOCKED
                "#
            );
            let Some(locked_schedule_row) = sqlx::query(sqlx::AssertSqlSafe(lock_sql))
                .bind(advance.schedule_id)
                .bind(advance.scheduled_for)
                .fetch_optional(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?
            else {
                continue;
            };
            let locked_schedule = schedule_from_row(&locked_schedule_row)?;
            let scheduled_for = locked_schedule.next_run_at.ok_or_else(|| {
                AppError::Database("Locked due schedule has no occurrence cursor".to_string())
            })?;

            let Some(occurrence_row) = sqlx::query(
                r#"
                INSERT INTO playbook_schedule_occurrence (
                    schedule_id, scheduled_for, fired_at, lease_owner,
                    lease_expires_at, leased_at, created_at, updated_at
                )
                VALUES (
                    $1, $2, clock_timestamp(), $3,
                    clock_timestamp() + make_interval(secs => $4::double precision),
                    clock_timestamp(), clock_timestamp(), clock_timestamp()
                )
                ON CONFLICT (schedule_id, scheduled_for) DO NOTHING
                RETURNING scheduled_for, fired_at
                "#,
            )
            .bind(advance.schedule_id)
            .bind(scheduled_for)
            .bind(&record.lease_owner)
            .bind(record.lease_ttl_seconds)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
            else {
                continue;
            };

            let update_sql = format!(
                r#"
                UPDATE playbook_schedule
                SET last_run_at = $2,
                    next_run_at = $3,
                    enabled = CASE WHEN $3::timestamptz IS NULL THEN FALSE ELSE enabled END,
                    updated_at = clock_timestamp()
                WHERE id = $1
                  AND next_run_at = $4
                RETURNING {SCHEDULE_COLUMNS}
                "#
            );
            let schedule_row = sqlx::query(sqlx::AssertSqlSafe(update_sql))
                .bind(advance.schedule_id)
                .bind(read::<chrono::DateTime<chrono::Utc>>(
                    &occurrence_row,
                    "fired_at",
                )?)
                .bind(advance.next_run_at)
                .bind(scheduled_for)
                .fetch_optional(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?
                .ok_or_else(|| {
                    AppError::Database(
                        "Schedule cursor changed while its occurrence was claimed".to_string(),
                    )
                })?;
            occurrences.push(LeasedScheduleOccurrenceRecord {
                schedule: schedule_from_row(&schedule_row)?,
                scheduled_for: read(&occurrence_row, "scheduled_for")?,
                fired_at: read(&occurrence_row, "fired_at")?,
            });
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(occurrences)
    }

    async fn release_schedule_occurrence(
        &self,
        record: ReleaseScheduleOccurrenceRecord,
    ) -> Result<bool, AppError> {
        let result = sqlx::query(
            r#"
            UPDATE playbook_schedule_occurrence AS occurrence
            SET lease_owner = NULL,
                lease_expires_at = NULL,
                leased_at = NULL,
                updated_at = clock_timestamp()
            WHERE occurrence.schedule_id = $1
              AND occurrence.scheduled_for = $2
              AND occurrence.lease_owner = $3
              AND NOT EXISTS (
                  SELECT 1
                  FROM playbook_run_group AS run_group
                  WHERE run_group.schedule_id = occurrence.schedule_id
                    AND run_group.scheduled_for = occurrence.scheduled_for
              )
            "#,
        )
        .bind(record.schedule_id)
        .bind(record.scheduled_for)
        .bind(&record.lease_owner)
        .execute(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_schedule(
        &self,
        record: UpdateScheduleRecord,
    ) -> Result<Option<PlaybookSchedule>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        if let Some(targets) = &record.targets {
            if let Some(target_set_id) = target_set_lock::schedule_target_set_id(targets) {
                target_set_lock::lock_target_sets_for_reference_tx(&mut tx, &[target_set_id])
                    .await?;
            }
        }
        let targets = record.targets.map(serde_json::to_value).transpose()?;
        let overrides = record.overrides.map(serde_json::to_value).transpose()?;
        // `chk_playbook_schedule_enabled_due` requires a non-null cursor whenever
        // the row ends up enabled. The service rejects enabling without a usable
        // `next_run_at`, so here we only COALESCE the provided value over the
        // existing one.
        let sql = format!(
            r#"
            UPDATE playbook_schedule
            SET targets = COALESCE($2, targets),
                overrides = COALESCE($3, overrides),
                name = COALESCE($4, name),
                cron_expression = COALESCE($5, cron_expression),
                timezone = COALESCE($6, timezone),
                misfire_policy = COALESCE($7, misfire_policy),
                concurrency_policy = COALESCE($8, concurrency_policy),
                enabled = COALESCE($9, enabled),
                next_run_at = COALESCE($10, next_run_at),
                metadata = COALESCE($11, metadata),
                updated_by_user_id = COALESCE($12, updated_by_user_id),
                updated_at = NOW()
            WHERE id = $1
            RETURNING {SCHEDULE_COLUMNS}
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.id)
            .bind(targets)
            .bind(overrides)
            .bind(record.name)
            .bind(record.cron_expression)
            .bind(record.timezone)
            .bind(
                record
                    .misfire_policy
                    .map(|policy| policy.as_str().to_string()),
            )
            .bind(
                record
                    .concurrency_policy
                    .map(|policy| policy.as_str().to_string()),
            )
            .bind(record.enabled)
            .bind(record.next_run_at)
            .bind(record.metadata)
            .bind(record.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let schedule = row.as_ref().map(schedule_from_row).transpose()?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(schedule)
    }

    async fn delete_schedule(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM playbook_schedule WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn pause_schedule_owner_authority(
        &self,
        id: Uuid,
        reason: &str,
    ) -> Result<bool, AppError> {
        let result = sqlx::query(
            r#"
            UPDATE playbook_schedule
            SET enabled = false,
                metadata = metadata || jsonb_build_object(
                    'owner_migration_paused', true,
                    'owner_migration_pause_reason', $2::text
                ),
                updated_at = NOW()
            WHERE id = $1 AND enabled
            "#,
        )
        .bind(id)
        .bind(reason)
        .execute(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn create_run_group(
        &self,
        record: CreateRunGroupRecord,
    ) -> Result<CreatedRunGroupRecord, AppError> {
        let template_id = record.launch_spec.template_id;
        let spec = record.launch_spec;
        let total_runs = i32::try_from(spec.targets.len()).map_err(|_| {
            AppError::Validation("Resolved target list is too large for one run group".to_string())
        })?;
        let metadata = record.metadata;
        let credential_decision = run_credential_decision_from_metadata(&metadata)?;
        let launch_options = launch_options_metadata(&metadata);
        let base_timeout_seconds = i32::try_from(spec.options.timeout_seconds)
            .map_err(|_| AppError::Validation("Launch timeout is out of range".to_string()))?;
        let run_timeout_seconds = run_timeout_seconds_for_target_count(
            &metadata,
            base_timeout_seconds,
            spec.targets.len(),
        )?;
        let timeout_policy = run_timeout_policy_metadata(
            &metadata,
            base_timeout_seconds,
            spec.targets.len(),
            run_timeout_seconds,
        )?;
        let mut group_metadata = metadata.as_object().cloned().unwrap_or_default();
        group_metadata.insert("timeout_policy".to_string(), timeout_policy.clone());
        // C3 snapshot is server-authored: strip any caller-supplied value (anti-
        // forgery) and write only the trusted snapshot from the REST gate. The
        // worker (fail-closed) trusts `run_group.metadata.secret_env_authorization`,
        // so it must never originate from launch metadata.
        group_metadata.remove("secret_env_authorization");
        if let Some(authorization) = &record.secret_env_authorization {
            group_metadata.insert(
                "secret_env_authorization".to_string(),
                authorization.clone(),
            );
        }
        // Always overwrite this reserved field: only the authorizer can grant
        // a record context, including when its authorized set is empty.
        group_metadata.insert(
            "ssh_record_contexts".to_string(),
            serde_json::to_value(&record.ssh_record_contexts)?,
        );
        let group_metadata = Value::Object(group_metadata);
        // The group's target-set reference is metadata only: it records which
        // managed inventory produced this launch and never drives target
        // resolution here. Native launches keep it NULL.
        let target_set_id = match &spec.inventory {
            InventorySelection::Managed { target_set_id } => Some(*target_set_id),
            InventorySelection::Native { .. } => None,
        };
        let launch_spec = serde_json::to_value(&spec)?;

        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        apply_scheduled_run_concurrency_policy(&mut tx, record.schedule_id, template_id).await?;
        let group_sql = format!(
            r#"
            INSERT INTO playbook_run_group (
                template_id, launch_spec, target_set_id, schedule_id, scheduled_for,
                requested_by_user_id, service_principal_id, trigger_kind, status,
                total_runs, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', $9, $10)
            RETURNING {RUN_GROUP_COLUMNS}
            "#
        );
        let group_row = sqlx::query(sqlx::AssertSqlSafe(group_sql))
            .bind(spec.template_id)
            .bind(&launch_spec)
            .bind(target_set_id)
            .bind(record.schedule_id)
            .bind(record.scheduled_for)
            .bind(record.requested_by_user_id)
            .bind(record.service_principal_id)
            .bind(record.trigger_kind.as_str())
            .bind(total_runs)
            .bind(&group_metadata)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let run_group = run_group_from_row(&group_row)?;

        let credential_metadata = run_credential_metadata(
            record.requested_by_user_id,
            record.service_principal_id,
            &credential_decision,
        );
        let runs = if spec.targets.is_empty() {
            Vec::new()
        } else {
            struct RunInsertInput {
                ord: i64,
                id: Uuid,
                hostname: String,
                target_id: Option<Uuid>,
                source_id: Option<Uuid>,
                target_name: String,
                metadata: Value,
            }
            let mut run_inputs = Vec::with_capacity(spec.targets.len());
            for (index, target) in spec.targets.iter().enumerate() {
                let hostname = target.hostname.trim().to_string();
                if hostname.is_empty() {
                    return Err(AppError::Validation(
                        "Resolved target hostname must not be empty".to_string(),
                    ));
                }
                let mut run_metadata = serde_json::Map::new();
                run_metadata.insert(
                    "target".to_string(),
                    json!({
                        "target_id": target.target_id,
                        "source_id": target.source_id,
                    }),
                );
                run_metadata.insert("credential".to_string(), credential_metadata.clone());
                run_metadata.insert("timeout_policy".to_string(), timeout_policy.clone());
                run_metadata.insert(
                    "execution_policy".to_string(),
                    json!({
                        "runtime": "ansible",
                        "credential_resolver": "connection_service.resolve_ssh_execution_launch",
                        "credential_resolution": "resolve_at_worker_start",
                        "secret_material": "runtime_only"
                    }),
                );
                if let Some(options) = launch_options.clone() {
                    run_metadata.insert("launch_options".to_string(), options);
                }
                run_inputs.push(RunInsertInput {
                    ord: i64::try_from(index).map_err(|_| {
                        AppError::Validation(
                            "Resolved target list is too large to preserve run order".to_string(),
                        )
                    })?,
                    id: Uuid::now_v7(),
                    hostname: hostname.clone(),
                    target_id: target.target_id,
                    source_id: target.source_id,
                    target_name: hostname,
                    metadata: Value::Object(run_metadata),
                });
            }
            let mut run_insert = QueryBuilder::<Postgres>::new(
                r#"
                WITH run_input (
                    ord, id, hostname, target_id, source_id, target_name, metadata
                ) AS (
                "#,
            );
            run_insert.push_values(&run_inputs, |mut row, input| {
                row.push_bind(input.ord)
                    .push_bind(input.id)
                    .push_bind(input.hostname.clone())
                    .push_bind(input.target_id)
                    .push_bind(input.source_id)
                    .push_bind(input.target_name.clone())
                    .push_bind(input.metadata.clone());
            });
            run_insert.push(
                r#"
                ),
                inserted AS (
                    INSERT INTO playbook_run (
                        id, run_group_id, hostname, target_kind, target_id,
                        source_kind, source_id, target_name, target_address,
                        target_address_bound, credential_account_kind, credential_account_id,
                        credential_account_entry_id, credential_merge_policy, credential_resolution,
                        status, timeout_seconds, metadata
                    )
                    SELECT
                        run_input.id,
                "#,
            );
            run_insert
                .push_bind(run_group.id)
                .push(
                    r#",
                        run_input.hostname,
                        CASE
                            WHEN run_input.target_id IS NULL THEN NULL
                            ELSE 'connection_target'
                        END,
                        run_input.target_id,
                        CASE
                            WHEN run_input.source_id IS NULL THEN NULL
                            WHEN EXISTS (
                                SELECT 1 FROM public.dcim_device AS device
                                WHERE device.id = run_input.source_id
                            ) THEN 'device'
                            WHEN EXISTS (
                                SELECT 1 FROM public.virtualization_virtualmachine AS vm
                                WHERE vm.id = run_input.source_id
                            ) THEN 'virtual_machine'
                            ELSE NULL
                        END,
                        run_input.source_id,
                        run_input.target_name,
                        NULL,
                        FALSE,
                "#,
                )
                .push_bind(credential_decision.account_kind.clone())
                .push(", ")
                .push_bind(credential_decision.account_id)
                .push(", ")
                .push_bind(credential_decision.account_entry_id.clone())
                .push(", ")
                .push_bind(credential_decision.merge_policy.clone())
                .push(", ")
                .push_bind(credential_decision.resolution.clone())
                .push(", 'queued', ")
                .push_bind(run_timeout_seconds)
                .push(
                    r#",
                        run_input.metadata
                    FROM run_input
                    RETURNING
                "#,
                );
            run_insert.push(RUN_COLUMNS);
            run_insert.push(
                r#"
                )
                SELECT inserted.*
                FROM inserted
                JOIN run_input ON run_input.id = inserted.id
                ORDER BY run_input.ord
                "#,
            );
            let run_rows = run_insert
                .build()
                .fetch_all(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            let runs = run_rows
                .iter()
                .map(run_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            if runs.len() != spec.targets.len() {
                return Err(AppError::Database(
                    "Not every resolved target received a playbook run".to_string(),
                ));
            }
            runs
        };
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(CreatedRunGroupRecord { run_group, runs })
    }

    async fn get_run(&self, run_id: Uuid) -> Result<Option<PlaybookRun>, AppError> {
        let sql = format!("SELECT {RUN_COLUMNS} FROM playbook_run WHERE id = $1");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(run_from_row)
            .transpose()
    }

    async fn list_runs(
        &self,
        filter: &PlaybookRunListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRun>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let push_where = |builder: &mut QueryBuilder<Postgres>| {
            let mut where_state = WhereState::new();
            if !filter.template_id.is_empty() {
                where_state.and(builder);
                builder.push(
                    "run_group_id IN (SELECT id FROM playbook_run_group WHERE template_id IN (",
                );
                let mut separated = builder.separated(", ");
                for template_id in &filter.template_id {
                    separated.push_bind(*template_id);
                }
                builder.push("))");
            }
            if !filter.status.is_empty() {
                where_state.and(builder);
                builder.push("status IN (");
                let mut separated = builder.separated(", ");
                for status in &filter.status {
                    separated.push_bind(status.as_str());
                }
                builder.push(")");
            }
        };

        let mut builder =
            QueryBuilder::<Postgres>::new(format!("SELECT {RUN_COLUMNS} FROM playbook_run"));
        push_where(&mut builder);
        push_order_by(
            &mut builder,
            PLAYBOOK_RUN_COLUMNS,
            filter.sort.as_ref(),
            "queued_at DESC, id DESC",
        );
        if filter.sort.is_some() {
            builder.push(", id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(i64::from(limit));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(offset));
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows.iter().map(run_from_row).collect::<Result<_, _>>()?;

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*)::bigint FROM playbook_run");
        push_where(&mut count_builder);
        let total = count_builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook run")
    }

    async fn list_run_groups(
        &self,
        filter: &PlaybookRunListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookRunGroup>, i64), AppError> {
        let sql = format!(
            "SELECT {RUN_GROUP_COLUMNS}, COUNT(*) OVER() AS total_count FROM playbook_run_group"
        );
        let mut builder = QueryBuilder::new(sql);
        let mut where_state = WhereState::new();
        push_uuid_in(
            &mut builder,
            &mut where_state,
            "template_id",
            &filter.template_id,
        );
        let status: Vec<String> = filter
            .status
            .iter()
            .map(|status| status.as_str().to_string())
            .collect();
        push_string_in(&mut builder, &mut where_state, "status", &status);
        push_order_by(
            &mut builder,
            PLAYBOOK_RUN_GROUP_COLUMNS,
            filter.sort.as_ref(),
            "created_at DESC, id DESC",
        );
        if filter.sort.is_some() {
            builder.push(", id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        // The window function repeats the unpaged match count on every row; an
        // empty page means there is nothing to count.
        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|error| {
                AppError::Database(format!("Failed to read run group total_count: {error}"))
            })?,
            None => 0,
        };
        let items = rows
            .iter()
            .map(run_group_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn get_run_group_with_runs(
        &self,
        run_group_id: Uuid,
        limit: i64,
    ) -> Result<Option<RunGroupWithRuns>, AppError> {
        let group_sql = format!("SELECT {RUN_GROUP_COLUMNS} FROM playbook_run_group WHERE id = $1");
        let Some(group_row) = sqlx::query(sqlx::AssertSqlSafe(group_sql))
            .bind(run_group_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        let run_group = run_group_from_row(&group_row)?;
        let runs_sql = format!(
            r#"
            SELECT {RUN_COLUMNS}
            FROM playbook_run
            WHERE run_group_id = $1
            ORDER BY queued_at ASC, id ASC
            LIMIT $2
            "#
        );
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(runs_sql))
            .bind(run_group_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(RunGroupWithRuns { run_group, runs }))
    }

    async fn request_run_cancel(
        &self,
        run_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
        deadline_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<PlaybookRun>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_sql = format!("SELECT {RUN_COLUMNS} FROM playbook_run WHERE id = $1");
        let Some(run_row) = sqlx::query(sqlx::AssertSqlSafe(run_sql))
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let existing = run_from_row(&run_row)?;
        if existing.status.is_terminal() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(Some(existing));
        }
        let mut audit_events = Vec::new();
        let (detail, newly_canceled_runs) = request_run_group_cancel_tx(
            &mut tx,
            &mut audit_events,
            existing.run_group_id,
            i64::MAX,
            now,
            deadline_at,
        )
        .await?
        .ok_or_else(|| AppError::Database("Playbook run group not found".to_string()))?;
        let run = detail
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| {
                AppError::Database("Playbook run disappeared during cancel".to_string())
            })?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        record_canceled_run_metrics(&newly_canceled_runs);
        Ok(Some(run))
    }

    async fn request_run_group_cancel(
        &self,
        run_group_id: Uuid,
        limit: i64,
        now: chrono::DateTime<chrono::Utc>,
        deadline_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<RunGroupWithRuns>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let mut audit_events = Vec::new();
        let requested = request_run_group_cancel_tx(
            &mut tx,
            &mut audit_events,
            run_group_id,
            limit,
            now,
            deadline_at,
        )
        .await?;
        let Some((detail, newly_canceled_runs)) = requested else {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        record_canceled_run_metrics(&newly_canceled_runs);
        Ok(Some(detail))
    }

    async fn list_pending_run_group_cancels(
        &self,
        limit: i64,
    ) -> Result<Vec<PlaybookRunGroup>, AppError> {
        let sql = format!(
            r#"
            SELECT {RUN_GROUP_COLUMNS}
            FROM playbook_run_group
            WHERE cancel_requested_at IS NOT NULL
              AND cancel_outcome IS NULL
              AND dispatch_id IS NOT NULL
              AND status IN ('dispatching', 'running')
            ORDER BY cancel_requested_at ASC, id ASC
            LIMIT $1
            "#
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        rows.iter().map(run_group_from_row).collect()
    }

    async fn finalize_run_group_cancel(
        &self,
        record: FinalizeRunGroupCancelRecord,
    ) -> Result<Option<FinalizedRunGroupCancelRecord>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let group_sql =
            format!("SELECT {RUN_GROUP_COLUMNS} FROM playbook_run_group WHERE id = $1 FOR UPDATE");
        let Some(group_row) = sqlx::query(sqlx::AssertSqlSafe(group_sql))
            .bind(record.run_group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let group = run_group_from_row(&group_row)?;
        if group.dispatch_id != Some(record.dispatch_id)
            || group.cancel_requested_at.is_none()
            || group.cancel_outcome.is_some()
            || group.status.is_terminal()
        {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        }
        if record.outcome == PlaybookCancelOutcome::DeadlineExpired
            && group
                .cancel_deadline_at
                .is_none_or(|deadline| record.now < deadline)
        {
            return Err(AppError::Conflict(
                "Playbook cancellation deadline has not expired".to_string(),
            ));
        }

        sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET cancel_outcome = $3,
                updated_at = $4
            WHERE id = $1 AND dispatch_id = $2 AND cancel_outcome IS NULL
            "#,
        )
        .bind(record.run_group_id)
        .bind(record.dispatch_id)
        .bind(record.outcome.as_str())
        .bind(record.now)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let update_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'canceled',
                cancel_outcome = $2,
                finished_at = $3,
                lease_expires_at = NULL,
                error_message = COALESCE(error_message, $4)
            WHERE run_group_id = $1
              AND cancel_requested_at IS NOT NULL
              AND status IN ('leased', 'dispatching', 'running')
            RETURNING {RUN_COLUMNS}
            "#
        );
        let message = match record.outcome {
            PlaybookCancelOutcome::RunnerConfirmed => {
                "Runner confirmed the dispatched process stopped"
            }
            PlaybookCancelOutcome::DeadlineExpired => {
                "Cancellation deadline expired without runner confirmation"
            }
            PlaybookCancelOutcome::NotDispatched => {
                return Err(AppError::Validation(
                    "A dispatched cancellation cannot resolve as not_dispatched".to_string(),
                ));
            }
        };
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(update_sql))
            .bind(record.run_group_id)
            .bind(record.outcome.as_str())
            .bind(record.now)
            .bind(message)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let canceled_runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let mut audit_events = Vec::new();
        append_cancel_finalized_events_tx(
            &mut tx,
            &mut audit_events,
            &canceled_runs,
            record.outcome,
            message,
        )
        .await?;
        let run_group = rollup_run_groups_tx(
            &mut tx,
            &mut audit_events,
            &[record.run_group_id],
            record.now,
            Some(AuditEmitterKind::Worker),
        )
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Database("Playbook run group not found".to_string()))?;
        audit_terminal_runs_tx(
            &mut tx,
            &mut audit_events,
            &run_group,
            &canceled_runs,
            AuditEmitterKind::Worker,
            record.now,
        )
        .await?;
        // The deadline path terminalizes whatever the runner never confirmed,
        // so the group's callback token must be revoked here too.
        revoke_terminal_callback_tokens_tx(&mut tx, &[record.run_group_id], record.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        record_canceled_run_metrics(&canceled_runs);
        Ok(Some(FinalizedRunGroupCancelRecord {
            run_group,
            canceled_runs,
        }))
    }

    async fn create_job_token(
        &self,
        record: CreateJobTokenRecord,
    ) -> Result<PlaybookJobToken, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_job_token (
                id, run_id, target_kind, target_id, source_kind, source_id,
                token_hash, token_prefix, scopes, expires_at, metadata
            )
            SELECT
                $1,
                run.id,
                run.target_kind,
                run.target_id,
                run.source_kind,
                run.source_id,
                $3,
                $4,
                $5,
                $6,
                $7::jsonb || jsonb_build_object(
                    'audience', $8::text,
                    'owner_user_id', run_group.requested_by_user_id
                )
            FROM playbook_run AS run
            JOIN playbook_run_group AS run_group ON run_group.id = run.run_group_id
            WHERE run.id = $2
            RETURNING {JOB_TOKEN_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.id)
            .bind(record.run_id)
            .bind(record.token_hash)
            .bind(record.token_prefix)
            .bind(record.scopes)
            .bind(record.expires_at)
            .bind(record.metadata)
            .bind(record.audience.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound("Playbook run not found".to_string()));
        };
        job_token_from_row(&row)
    }

    async fn validate_active_job_token(
        &self,
        token_id: Uuid,
        token_hash: &str,
        audience: JobTokenAudience,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<PlaybookJobToken, AppError> {
        let sql = format!(
            r#"
            UPDATE playbook_job_token
            SET last_used_at = $3
            WHERE id = $1
              AND token_hash = $2
              AND status = 'active'
              AND revoked_at IS NULL
              AND expires_at > $3
              AND metadata->>'audience' = $4
              AND EXISTS (
                  SELECT 1
                  FROM playbook_run AS run
                  JOIN playbook_run_group AS run_group ON run_group.id = run.run_group_id
                  WHERE run.id = playbook_job_token.run_id
                    AND run.target_kind = playbook_job_token.target_kind
                    AND run.target_id = playbook_job_token.target_id
                    AND run.source_kind = playbook_job_token.source_kind
                    AND run.source_id = playbook_job_token.source_id
                    AND run.status IN ('leased', 'dispatching', 'running')
                    AND run.finished_at IS NULL
                    AND run_group.status IN ('leased', 'dispatching', 'running')
                    AND run_group.finished_at IS NULL
                    AND ($4 <> 'sandbox' OR EXISTS (
                        SELECT 1 FROM identity_user AS owner
                        WHERE owner.id = run_group.requested_by_user_id
                          AND owner.status = 'active'
                          AND owner.id::text = playbook_job_token.metadata->>'owner_user_id'
                    ))
              )
            RETURNING {JOB_TOKEN_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(token_id)
            .bind(token_hash)
            .bind(now)
            .bind(audience.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::Auth("TOKEN_INVALID".to_string()));
        };
        job_token_from_row(&row)
    }

    async fn create_artifact_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateArtifactRecord,
        audience: JobTokenAudience,
    ) -> Result<PlaybookArtifact, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_group_id =
            authorize_run_callback_tx(&mut tx, &auth, record.run_id, "artifact:create", audience)
                .await?;
        let usage = CallbackUsage {
            artifact_count: 1,
            content_size_bytes: record.content_size_bytes,
            request_count: 1,
            ..CallbackUsage::default()
        };
        charge_run_callback_quota_tx(&mut tx, record.run_id, run_group_id, auth.now, usage).await?;
        charge_group_callback_quota_tx(&mut tx, run_group_id, auth.now, usage).await?;
        let sql = format!(
            r#"
            INSERT INTO playbook_artifact (
                run_id, artifact_type, name, content_type, content_json,
                content_text, content_sha256, content_size_bytes
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING {ARTIFACT_COLUMNS}
            "#
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(record.run_id)
            .bind(record.artifact_type)
            .bind(record.name)
            .bind(record.content_type)
            .bind(record.content_json)
            .bind(record.content_text)
            .bind(record.content_sha256)
            .bind(record.content_size_bytes)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let artifact = artifact_from_row(&row)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(artifact)
    }

    async fn list_artifacts(
        &self,
        run_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookArtifact>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!(
            r#"
            SELECT {ARTIFACT_COLUMNS}
            FROM playbook_artifact
            WHERE run_id = $1
            ORDER BY created_at ASC, id ASC
            LIMIT $2 OFFSET $3
            "#
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(run_id)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(artifact_from_row)
            .collect::<Result<_, _>>()?;
        let total =
            sqlx::query_scalar("SELECT COUNT(*)::bigint FROM playbook_artifact WHERE run_id = $1")
                .bind(run_id)
                .fetch_one(&self.pool)
                .await
                .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook run artifact")
    }

    async fn append_run_event_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateRunEventRecord,
        content_size_bytes: i64,
    ) -> Result<PlaybookRunEvent, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_group_id = authorize_run_callback_tx(
            &mut tx,
            &auth,
            record.run_id,
            "event:append",
            JobTokenAudience::Supervisor,
        )
        .await?;
        let usage = CallbackUsage {
            event_count: 1,
            content_size_bytes,
            request_count: 1,
            ..CallbackUsage::default()
        };
        charge_run_callback_quota_tx(&mut tx, record.run_id, run_group_id, auth.now, usage).await?;
        charge_group_callback_quota_tx(&mut tx, run_group_id, auth.now, usage).await?;
        let event = append_run_event_tx(&mut tx, record).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(event)
    }

    async fn append_run_group_event_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateRunGroupEventRecord,
        content_size_bytes: i64,
    ) -> Result<PlaybookRunEvent, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        authorize_group_callback_tx(&mut tx, &auth, record.run_group_id).await?;
        let group_usage = CallbackUsage {
            event_count: 1,
            content_size_bytes,
            request_count: 1,
            ..CallbackUsage::default()
        };
        charge_group_callback_quota_tx(&mut tx, record.run_group_id, auth.now, group_usage).await?;
        if let Some(run_id) = record.run_id {
            charge_run_callback_quota_tx(
                &mut tx,
                run_id,
                record.run_group_id,
                auth.now,
                CallbackUsage {
                    event_count: 1,
                    content_size_bytes,
                    request_count: 1,
                    ..CallbackUsage::default()
                },
            )
            .await?;
        }
        let event = append_run_group_event_tx(&mut tx, record).await?;
        revoke_terminal_callback_tokens_tx(&mut tx, &[event.run_group_id], auth.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(event)
    }

    async fn append_run_group_event_batch(
        &self,
        batch_id: Uuid,
        records: Vec<CreateRunGroupEventRecord>,
    ) -> Result<AppendRunGroupEventBatchOutcome, AppError> {
        let run_group_id = records
            .first()
            .map(|record| record.run_group_id)
            .ok_or_else(|| {
                AppError::Validation("event batch must contain at least one event".to_string())
            })?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let claimed_batch_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO playbook_run_group_event_batch (run_group_id, batch_id)
            VALUES ($1, $2)
            ON CONFLICT (run_group_id, batch_id) DO NOTHING
            RETURNING batch_id
            "#,
        )
        .bind(run_group_id)
        .bind(batch_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        if claimed_batch_id.is_none() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(AppendRunGroupEventBatchOutcome::AlreadyIngested);
        }

        let mut audit_events = Vec::new();
        let events = append_run_group_events_tx(
            &mut tx,
            &mut audit_events,
            records,
            AuditEmitterKind::Worker,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(AppendRunGroupEventBatchOutcome::Created(events))
    }

    async fn append_run_group_event_batch_with_callback(
        &self,
        auth: CallbackTokenAuth,
        batch_id: Uuid,
        records: Vec<(CreateRunGroupEventRecord, i64)>,
    ) -> Result<AppendRunGroupEventBatchOutcome, AppError> {
        let run_group_id = records
            .first()
            .map(|(record, _)| record.run_group_id)
            .ok_or_else(|| {
                AppError::Validation("event batch must contain at least one event".to_string())
            })?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        authorize_group_callback_tx(&mut tx, &auth, run_group_id).await?;
        charge_group_callback_quota_tx(
            &mut tx,
            run_group_id,
            auth.now,
            CallbackUsage {
                request_count: 1,
                ..CallbackUsage::default()
            },
        )
        .await?;
        let claimed_batch_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO playbook_run_group_event_batch (run_group_id, batch_id)
            VALUES ($1, $2)
            ON CONFLICT (run_group_id, batch_id) DO NOTHING
            RETURNING batch_id
            "#,
        )
        .bind(run_group_id)
        .bind(batch_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if claimed_batch_id.is_none() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(AppendRunGroupEventBatchOutcome::AlreadyIngested);
        }

        let event_count = i64::try_from(records.len())
            .map_err(|_| AppError::Validation("event batch is too large".to_string()))?;
        let content_size_bytes = records.iter().try_fold(0_i64, |total, (_, size)| {
            total.checked_add(*size).ok_or_else(|| {
                AppError::Validation("event batch byte size is too large".to_string())
            })
        })?;
        charge_group_callback_quota_tx(
            &mut tx,
            run_group_id,
            auth.now,
            CallbackUsage {
                event_count,
                content_size_bytes,
                ..CallbackUsage::default()
            },
        )
        .await?;

        let mut per_run_usage = HashMap::<Uuid, CallbackUsage>::new();
        for (record, size) in &records {
            if let Some(run_id) = record.run_id {
                let usage = per_run_usage.entry(run_id).or_default();
                usage.event_count += 1;
                usage.request_count = 1;
                usage.content_size_bytes =
                    usage.content_size_bytes.checked_add(*size).ok_or_else(|| {
                        AppError::Validation("per-run event byte size is too large".to_string())
                    })?;
            }
        }
        for (run_id, usage) in per_run_usage {
            charge_run_callback_quota_tx(&mut tx, run_id, run_group_id, auth.now, usage).await?;
        }

        let mut audit_events = Vec::new();
        let events = append_run_group_events_tx(
            &mut tx,
            &mut audit_events,
            records.into_iter().map(|(record, _)| record).collect(),
            AuditEmitterKind::Api,
        )
        .await?;
        revoke_terminal_callback_tokens_tx(&mut tx, &[run_group_id], auth.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(AppendRunGroupEventBatchOutcome::Created(events))
    }

    async fn count_run_group_runs(
        &self,
        run_group_id: Uuid,
        run_ids: &[Uuid],
    ) -> Result<i64, AppError> {
        if run_ids.is_empty() {
            return Ok(0);
        }
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)::bigint
            FROM playbook_run
            WHERE run_group_id = $1
              AND id = ANY($2)
            "#,
        )
        .bind(run_group_id)
        .bind(run_ids)
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)
    }

    async fn list_run_group_events(
        &self,
        run_group_id: Uuid,
        filter: PlaybookRunGroupEventListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRunEvent>, AppError> {
        let PlaybookRunGroupEventListFilter {
            run_id,
            host,
            after_sequence,
        } = filter;
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!(
            r#"
            SELECT {RUN_EVENT_COLUMNS}
            FROM playbook_run_event
            WHERE run_group_id = $1
              AND (
                ($2::uuid IS NULL AND $3::text IS NULL)
                OR ($2::uuid IS NOT NULL AND $3::text IS NULL AND (run_id = $2 OR run_id IS NULL))
                OR ($2::uuid IS NULL AND $3::text IS NOT NULL AND (host = $3 OR host IS NULL))
                OR ($2::uuid IS NOT NULL AND $3::text IS NOT NULL AND (run_id = $2 OR run_id IS NULL) AND (host = $3 OR host IS NULL))
              )
              AND ($4::bigint IS NULL OR event_sequence > $4)
            ORDER BY event_sequence ASC
            LIMIT $5 OFFSET $6
            "#
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(run_group_id)
            .bind(run_id)
            .bind(host.as_deref())
            .bind(after_sequence)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(run_event_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)::bigint
            FROM playbook_run_event
            WHERE run_group_id = $1
              AND (
                ($2::uuid IS NULL AND $3::text IS NULL)
                OR ($2::uuid IS NOT NULL AND $3::text IS NULL AND (run_id = $2 OR run_id IS NULL))
                OR ($2::uuid IS NULL AND $3::text IS NOT NULL AND (host = $3 OR host IS NULL))
                OR ($2::uuid IS NOT NULL AND $3::text IS NOT NULL AND (run_id = $2 OR run_id IS NULL) AND (host = $3 OR host IS NULL))
              )
              AND ($4::bigint IS NULL OR event_sequence > $4)
            "#,
        )
        .bind(run_group_id)
        .bind(run_id)
        .bind(host.as_deref())
        .bind(after_sequence)
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook run-group event")
    }

    async fn list_run_events(
        &self,
        run_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRunEvent>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        // The projection joins `selected_run`, which also carries `id` and
        // `run_group_id`: qualify every column so the reference stays unambiguous.
        let columns = RUN_EVENT_COLUMNS
            .split(',')
            .map(|column| format!("playbook_run_event.{}", column.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            r#"
            WITH selected_run AS (
                SELECT id, run_group_id
                FROM playbook_run
                WHERE id = $1
            )
            SELECT {columns}
            FROM playbook_run_event
            JOIN selected_run ON selected_run.run_group_id = playbook_run_event.run_group_id
            WHERE playbook_run_event.run_id = selected_run.id
               OR playbook_run_event.run_id IS NULL
            ORDER BY playbook_run_event.event_sequence ASC
            LIMIT $2 OFFSET $3
            "#
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(run_id)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(run_event_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar(
            r#"
            WITH selected_run AS (
                SELECT id, run_group_id
                FROM playbook_run
                WHERE id = $1
            )
            SELECT COUNT(*)::bigint
            FROM playbook_run_event
            JOIN selected_run ON selected_run.run_group_id = playbook_run_event.run_group_id
            WHERE playbook_run_event.run_id = selected_run.id
               OR playbook_run_event.run_id IS NULL
            "#,
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook run event")
    }

    async fn append_console_batch_with_callback(
        &self,
        group_id: Uuid,
        batch: RunnerConsoleBatch,
        auth: CallbackTokenAuth,
    ) -> Result<ConsoleAck, AppError> {
        super::console::append_console_batch(&self.pool, group_id, batch, auth).await
    }

    async fn list_console(
        &self,
        group_id: Uuid,
        from_sequence: u64,
        limit: i64,
        dispatch_id: Option<Uuid>,
    ) -> Result<ConsolePage, AppError> {
        super::console::list_console(&self.pool, group_id, from_sequence, limit, dispatch_id).await
    }

    async fn lease_queued_groups(
        &self,
        record: LeaseQueuedRunsRecord,
    ) -> Result<Vec<PlaybookRunGroup>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('playbook_run_lease'))")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let group_sql = format!(
            r#"
            WITH active_groups AS (
                SELECT run_group_id, COUNT(*)::bigint AS active_count
                FROM playbook_run
                WHERE cancel_requested_at IS NULL
                  AND status IN ('leased', 'dispatching', 'running')
                  AND NOT (status = 'leased' AND lease_expires_at <= $3)
                GROUP BY run_group_id
            ), active_group_total AS (
                SELECT COUNT(*)::bigint AS total
                FROM active_groups
                WHERE active_count > 0
            ), candidate AS (
                SELECT
                    run_group.id,
                    COALESCE(active_groups.active_count, 0)::bigint AS active_count,
                    ROW_NUMBER() OVER (
                        ORDER BY COALESCE(run_group.started_at, run_group.created_at) ASC,
                                 run_group.id ASC
                    )::bigint AS group_rank
                FROM playbook_run_group AS run_group
                LEFT JOIN active_groups ON active_groups.run_group_id = run_group.id
                WHERE run_group.cancel_requested_at IS NULL
                  AND run_group.finished_at IS NULL
                  AND run_group.status IN ('queued', 'leased')
                  AND (
                    run_group.status = 'queued'
                    OR run_group.lease_expires_at IS NULL
                    OR run_group.lease_expires_at <= $3
                    OR run_group.lease_owner = $2
                  )
                  AND COALESCE(active_groups.active_count, 0) < $6
            ), eligible_groups AS (
                SELECT candidate.id
                FROM candidate
                CROSS JOIN active_group_total
                WHERE candidate.active_count > 0
                   OR candidate.group_rank <= GREATEST($5 - active_group_total.total, 0)
                ORDER BY candidate.group_rank
                LIMIT $1
            ), leased_groups AS (
                UPDATE playbook_run_group AS run_group
                SET status = 'leased',
                    lease_owner = $2,
                    lease_expires_at = $4,
                    leased_at = $3,
                    started_at = COALESCE(run_group.started_at, $3),
                    updated_at = $3
                FROM eligible_groups
                WHERE run_group.id = eligible_groups.id
                RETURNING run_group.*
            )
            SELECT * FROM leased_groups ORDER BY created_at ASC, id ASC
            "#
        );
        let group_rows = sqlx::query(sqlx::AssertSqlSafe(group_sql))
            .bind(record.limit)
            .bind(&record.lease_owner)
            .bind(record.now)
            .bind(record.lease_expires_at)
            .bind(record.max_concurrent_groups)
            .bind(record.max_concurrent_runs_per_group)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let groups = group_rows
            .iter()
            .map(run_group_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if groups.is_empty() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(groups);
        }

        let group_ids = groups.iter().map(|group| group.id).collect::<Vec<_>>();
        // No RETURNING here: the leased member rows are never decoded, and an
        // unqualified projection is ambiguous once `candidate` is in scope.
        // PostgreSQL rejects that ambiguity while analyzing the statement, so it
        // fails even when a group has no run rows at all (native launches).
        let run_sql = r#"
            WITH active_runs AS (
                SELECT run_group_id, COUNT(*)::bigint AS active_count
                FROM playbook_run
                WHERE run_group_id = ANY($1)
                  AND status IN ('leased', 'dispatching', 'running')
                  AND NOT (status = 'leased' AND lease_expires_at <= $3)
                GROUP BY run_group_id
            ), candidate AS (
                SELECT
                    run.id,
                    ROW_NUMBER() OVER (
                        PARTITION BY run.run_group_id
                        ORDER BY run.queued_at ASC, run.id ASC
                    )::bigint AS queued_rank,
                    COALESCE(active_runs.active_count, 0)::bigint AS active_count
                FROM playbook_run AS run
                LEFT JOIN active_runs ON active_runs.run_group_id = run.run_group_id
                WHERE run.run_group_id = ANY($1)
                  AND run.cancel_requested_at IS NULL
                  AND (
                    run.status = 'queued'
                    OR (run.status = 'leased' AND run.lease_expires_at <= $3)
                  )
            )
            UPDATE playbook_run AS run
            SET status = 'leased',
                lease_owner = $2,
                lease_expires_at = $4,
                leased_at = $3,
                error_message = NULL
            FROM candidate
            WHERE run.id = candidate.id
              AND candidate.queued_rank <= GREATEST($5 - candidate.active_count, 0)
            "#;
        sqlx::query(run_sql)
            .bind(&group_ids)
            .bind(&record.lease_owner)
            .bind(record.now)
            .bind(record.lease_expires_at)
            .bind(record.max_concurrent_runs_per_group)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(groups)
    }

    async fn start_leased_runs(
        &self,
        record: StartLeasedRunsRecord,
    ) -> Result<Vec<StartedRunRecord>, AppError> {
        if record.run_ids.len() != record.job_tokens.len() {
            return Err(AppError::Validation(
                "start batch must provide one job token per run".to_string(),
            ));
        }
        if record.run_ids.len() != record.sandbox_job_tokens.len() {
            return Err(AppError::Validation(
                "start batch must provide one sandbox job token per run".to_string(),
            ));
        }
        let expected_count = i64::try_from(record.run_ids.len()).map_err(|_| {
            AppError::Validation("start batch is too large to validate".to_string())
        })?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;

        let group_sql =
            format!("SELECT {RUN_GROUP_COLUMNS} FROM playbook_run_group WHERE id = $1 FOR UPDATE");
        let Some(group_row) = sqlx::query(sqlx::AssertSqlSafe(group_sql))
            .bind(record.run_group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound(
                "Playbook run group not found".to_string(),
            ));
        };
        let group = run_group_from_row(&group_row)?;
        if group.lease_owner.as_deref() != Some(record.lease_owner.as_str()) {
            return Err(AppError::Conflict(
                "Playbook run group is not leased by this worker".to_string(),
            ));
        }
        if group.cancel_requested_at.is_some() {
            return Err(AppError::Conflict(
                "Playbook run group is canceled".to_string(),
            ));
        }
        if group
            .lease_expires_at
            .map_or(true, |expires_at| expires_at <= record.now)
        {
            return Err(AppError::Conflict(
                "Playbook run group lease expired before start".to_string(),
            ));
        }
        if !matches!(
            group.status,
            PlaybookRunStatus::Queued | PlaybookRunStatus::Leased
        ) {
            return Err(AppError::Conflict(
                "Playbook run group is not queued or leased".to_string(),
            ));
        }

        let runs = if record.run_ids.is_empty() {
            Vec::new()
        } else {
            let run_sql = format!(
                r#"
                UPDATE playbook_run
                SET status = 'dispatching',
                    started_at = COALESCE(started_at, $4),
                    lease_expires_at = NULL,
                    error_message = NULL,
                    attempt = $5
                WHERE id = ANY($1)
                  AND run_group_id = $2
                  AND lease_owner = $3
                  AND status = 'leased'
                  AND lease_expires_at > $4
                  AND cancel_requested_at IS NULL
                RETURNING {RUN_COLUMNS}
                "#
            );
            let run_rows = sqlx::query(sqlx::AssertSqlSafe(run_sql))
                .bind(&record.run_ids)
                .bind(record.run_group_id)
                .bind(&record.lease_owner)
                .bind(record.now)
                .bind(record.dispatch_attempt)
                .fetch_all(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            if i64::try_from(run_rows.len()).unwrap_or(i64::MAX) != expected_count {
                return Err(AppError::Conflict(
                    "Some playbook runs are not leased by this worker, were canceled, or leases expired"
                        .to_string(),
                ));
            }
            run_rows
                .iter()
                .map(run_from_row)
                .collect::<Result<Vec<_>, _>>()?
        };

        let group_lease_expires_at = runs
            .iter()
            .map(|run| run.timeout_seconds.max(1))
            .max()
            .and_then(|seconds| {
                record
                    .now
                    .checked_add_signed(chrono::Duration::seconds(i64::from(seconds)))
            });
        let group_update = sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET status = 'dispatching',
                lease_owner = $2,
                lease_expires_at = COALESCE($3, lease_expires_at),
                leased_at = COALESCE(leased_at, $1),
                started_at = COALESCE(started_at, $1),
                dispatch_id = $4,
                dispatch_attempt = $5,
                updated_at = $1
            WHERE id = $6
              AND status IN ('queued', 'leased')
              AND dispatch_attempt < $5
              AND lease_owner = $2
              AND cancel_requested_at IS NULL
              AND lease_expires_at > $1
            "#,
        )
        .bind(record.now)
        .bind(&record.lease_owner)
        .bind(group_lease_expires_at)
        .bind(record.dispatch_id)
        .bind(record.dispatch_attempt)
        .bind(record.run_group_id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if group_update.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Playbook run group dispatch attempt changed concurrently".to_string(),
            ));
        }

        let events = runs
            .iter()
            .map(|run| CreateRunGroupEventRecord {
                run_group_id: record.run_group_id,
                run_id: Some(run.id),
                host: Some(run.hostname.clone()),
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatching".to_string(),
                message: Some("Run dispatch prepared by worker lease owner".to_string()),
                payload: serde_json::json!({
                    "lease_owner": record.lease_owner,
                    "dispatch_id": record.dispatch_id,
                    "dispatch_attempt": record.dispatch_attempt,
                }),
            })
            .collect::<Vec<_>>();
        let mut audit_events = Vec::new();
        append_run_group_events_tx(&mut tx, &mut audit_events, events, AuditEmitterKind::Worker)
            .await?;

        let group_token_insert = sqlx::query(
            r#"
            INSERT INTO playbook_run_group_callback_token (
                id, run_group_id, token_hash, token_prefix, expires_at, metadata
            )
            SELECT $1, run_group.id, $3, $4, $5, $6
            FROM playbook_run_group AS run_group
            WHERE run_group.id = $2
              AND run_group.status = 'dispatching'
              AND run_group.finished_at IS NULL
            "#,
        )
        .bind(record.group_callback_token.id)
        .bind(record.run_group_id)
        .bind(&record.group_callback_token.token_hash)
        .bind(&record.group_callback_token.token_prefix)
        .bind(record.group_callback_token.expires_at)
        .bind(&record.group_callback_token.metadata)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if group_token_insert.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Playbook run group is not active for callback token issuance".to_string(),
            ));
        }

        let mut token_records = record.job_tokens.clone();
        token_records.extend(record.sandbox_job_tokens.iter().cloned());
        let mut tokens_by_run_id = HashMap::with_capacity(token_records.len());
        let mut sandbox_tokens_by_run_id = HashMap::with_capacity(token_records.len());
        if !token_records.is_empty() {
            let mut token_insert = QueryBuilder::<Postgres>::new(
                r#"
                WITH token_input (
                    id, run_id, token_hash, token_prefix, scopes, expires_at, audience, metadata
                ) AS (
                "#,
            );
            token_insert.push_values(&token_records, |mut row, token| {
                row.push_bind(token.id)
                    .push_bind(token.run_id)
                    .push_bind(token.token_hash.clone())
                    .push_bind(token.token_prefix.clone())
                    .push_bind(token.scopes.clone())
                    .push_bind(token.expires_at)
                    .push_bind(token.audience.as_str())
                    .push_bind(token.metadata.clone());
            });
            token_insert.push(
                r#"
                )
                INSERT INTO playbook_job_token (
                    id, run_id, target_kind, target_id, source_kind, source_id,
                    token_hash, token_prefix, scopes, expires_at, metadata
                )
                SELECT
                    token_input.id,
                    run.id,
                    run.target_kind,
                    run.target_id,
                    run.source_kind,
                    run.source_id,
                    token_input.token_hash,
                    token_input.token_prefix,
                    token_input.scopes,
                    token_input.expires_at,
                    token_input.metadata || jsonb_build_object(
                        'audience', token_input.audience,
                        'owner_user_id', run_group.requested_by_user_id
                    )
                FROM token_input
                JOIN playbook_run AS run ON run.id = token_input.run_id
                JOIN playbook_run_group AS run_group ON run_group.id = run.run_group_id
                RETURNING
                "#,
            );
            token_insert.push(JOB_TOKEN_COLUMNS);
            let token_rows = token_insert
                .build()
                .fetch_all(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            if i64::try_from(token_rows.len()).unwrap_or(i64::MAX)
                != expected_count.saturating_mul(2)
            {
                return Err(AppError::Database(
                    "Not every started run received a supervisor and a sandbox job token"
                        .to_string(),
                ));
            }
            for row in token_rows {
                let token = job_token_from_row(&row)?;
                let bucket = match token.audience {
                    JobTokenAudience::Supervisor => &mut tokens_by_run_id,
                    JobTokenAudience::Sandbox => &mut sandbox_tokens_by_run_id,
                };
                bucket.insert(token.run_id, token);
            }
            if record
                .run_ids
                .iter()
                .any(|run_id| !sandbox_tokens_by_run_id.contains_key(run_id))
            {
                return Err(AppError::Database(
                    "Started run is missing its sandbox job token".to_string(),
                ));
            }
        }

        let mut runs_by_id = runs
            .into_iter()
            .map(|run| (run.id, run))
            .collect::<HashMap<_, _>>();
        let mut started = Vec::with_capacity(record.run_ids.len());
        for run_id in &record.run_ids {
            let run = runs_by_id.remove(run_id).ok_or_else(|| {
                AppError::Database("Started run missing from batch result".to_string())
            })?;
            let job_token = tokens_by_run_id.remove(run_id).ok_or_else(|| {
                AppError::Database("Job token missing from batch result".to_string())
            })?;
            started.push(StartedRunRecord { run, job_token });
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(started)
    }

    async fn mark_run_group_running(
        &self,
        run_group_id: Uuid,
        lease_owner: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<PlaybookRunGroup>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let group_sql = format!(
            r#"
            UPDATE playbook_run_group
            SET status = 'running', updated_at = $2
            WHERE id = $1 AND lease_owner = $3 AND status = 'dispatching'
            RETURNING {RUN_GROUP_COLUMNS}
            "#
        );
        let Some(group_row) = sqlx::query(sqlx::AssertSqlSafe(group_sql))
            .bind(run_group_id)
            .bind(now)
            .bind(lease_owner)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let group = run_group_from_row(&group_row)?;

        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'running'
            WHERE run_group_id = $1
              AND lease_owner = $2
              AND status = 'dispatching'
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(run_sql))
            .bind(run_group_id)
            .bind(lease_owner)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let events = runs
            .iter()
            .map(|run| CreateRunGroupEventRecord {
                run_group_id,
                run_id: Some(run.id),
                host: Some(run.hostname.clone()),
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatched".to_string(),
                message: Some("Ansible job dispatched; waiting for runner events".to_string()),
                payload: serde_json::json!({ "lease_owner": lease_owner }),
            })
            .collect::<Vec<_>>();
        let mut audit_events = Vec::new();
        append_run_group_events_tx(&mut tx, &mut audit_events, events, AuditEmitterKind::Worker)
            .await?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(Some(group))
    }

    async fn set_active_run_timeouts(
        &self,
        record: SetActiveRunTimeoutsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        if record.run_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET timeout_seconds = $3
            WHERE id = ANY($1)
              AND lease_owner = $2
              AND status IN ('dispatching', 'running')
              AND finished_at IS NULL
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(run_sql))
            .bind(&record.run_ids)
            .bind(&record.lease_owner)
            .bind(record.timeout_seconds)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;

        if !runs.is_empty() {
            let mut group_ids = runs.iter().map(|run| run.run_group_id).collect::<Vec<_>>();
            group_ids.sort_unstable();
            group_ids.dedup();
            sqlx::query(
                r#"
                UPDATE playbook_run_group
                SET lease_expires_at = $3,
                    updated_at = $4
                WHERE id = ANY($1)
                  AND lease_owner = $2
                  AND status IN ('leased', 'dispatching', 'running')
                "#,
            )
            .bind(group_ids)
            .bind(&record.lease_owner)
            .bind(record.lease_expires_at)
            .bind(record.now)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(runs)
    }

    async fn requeue_run_group(
        &self,
        run_group_id: Uuid,
        lease_owner: &str,
        retry_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let group_update = sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET status = 'leased',
                dispatch_id = NULL,
                lease_expires_at = $3,
                leased_at = $4,
                updated_at = $4
            WHERE id = $1
              AND lease_owner = $2
              AND status = 'dispatching'
            "#,
        )
        .bind(run_group_id)
        .bind(lease_owner)
        .bind(retry_at)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if group_update.rows_affected() == 0 {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(false);
        }

        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'leased',
                lease_expires_at = $3,
                leased_at = $4,
                started_at = NULL,
                error_message = $5
            WHERE run_group_id = $1
              AND lease_owner = $2
              AND status = 'dispatching'
              AND cancel_requested_at IS NULL
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(run_sql))
            .bind(run_group_id)
            .bind(lease_owner)
            .bind(retry_at)
            .bind(now)
            .bind(reason)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;

        // Receipt lookup returned 404, so deleting this attempt's tokens cannot
        // strand a ghost execution. Ambiguous failures never enter this path.
        let requeued_ids = runs.iter().map(|run| run.id).collect::<Vec<_>>();
        if !requeued_ids.is_empty() {
            sqlx::query("DELETE FROM playbook_job_token WHERE run_id = ANY($1)")
                .bind(&requeued_ids)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
        }
        sqlx::query("DELETE FROM playbook_run_group_callback_token WHERE run_group_id = $1")
            .bind(run_group_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;

        let events = runs
            .iter()
            .map(|run| CreateRunGroupEventRecord {
                run_group_id,
                run_id: Some(run.id),
                host: Some(run.hostname.clone()),
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatch_requeued".to_string(),
                message: Some(reason.to_string()),
                payload: serde_json::json!({
                    "lease_owner": lease_owner,
                    "attempt": run.attempt,
                    "retry_at": retry_at,
                }),
            })
            .collect::<Vec<_>>();
        let mut audit_events = Vec::new();
        append_run_group_events_tx(&mut tx, &mut audit_events, events, AuditEmitterKind::Worker)
            .await?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(true)
    }

    async fn complete_run(
        &self,
        record: CompleteRunRecord,
    ) -> Result<CompletedRunRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let mut audit_events = Vec::new();
        let completed = complete_run_tx(
            &mut tx,
            &mut audit_events,
            &record,
            AuditEmitterKind::Worker,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(completed)
    }

    async fn complete_run_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunRecord,
    ) -> Result<CompletedRunRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_group_id = authorize_run_callback_tx(
            &mut tx,
            &auth,
            record.run_id,
            "status:update",
            JobTokenAudience::Supervisor,
        )
        .await?;
        let usage = CallbackUsage {
            request_count: 1,
            ..CallbackUsage::default()
        };
        charge_run_callback_quota_tx(&mut tx, record.run_id, run_group_id, auth.now, usage).await?;
        charge_group_callback_quota_tx(&mut tx, run_group_id, auth.now, usage).await?;
        let mut audit_events = Vec::new();
        let completed =
            complete_run_tx(&mut tx, &mut audit_events, &record, AuditEmitterKind::Api).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(completed)
    }

    async fn complete_run_group_batch(
        &self,
        record: CompleteRunGroupBatchRecord,
    ) -> Result<CompletedRunGroupBatchRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let mut audit_events = Vec::new();
        let completed = complete_run_group_batch_tx(
            &mut tx,
            &mut audit_events,
            &record,
            AuditEmitterKind::Worker,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(completed)
    }

    async fn complete_run_group_batch_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunGroupBatchRecord,
    ) -> Result<CompletedRunGroupBatchRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        authorize_group_completion_callback_tx(&mut tx, &auth, record.run_group_id).await?;
        let dispatch_id = record.dispatch_id.ok_or_else(|| {
            AppError::Validation(
                "completion callback requires the supervisor dispatch id".to_string(),
            )
        })?;
        let dispatch_attempt = record.dispatch_attempt.ok_or_else(|| {
            AppError::Validation(
                "completion callback requires the supervisor dispatch attempt".to_string(),
            )
        })?;
        let console_next_sequence = record.console_next_sequence.ok_or_else(|| {
            AppError::Validation(
                "completion callback requires the final console sequence".to_string(),
            )
        })?;
        let replay = super::console::seal_console_prefix_tx(
            &mut tx,
            record.run_group_id,
            dispatch_id,
            dispatch_attempt,
            console_next_sequence,
        )
        .await?;
        if !replay {
            charge_group_callback_quota_tx(
                &mut tx,
                record.run_group_id,
                auth.now,
                CallbackUsage {
                    request_count: 1,
                    ..CallbackUsage::default()
                },
            )
            .await?;
            for run in &record.runs {
                charge_run_callback_quota_tx(
                    &mut tx,
                    run.run_id,
                    record.run_group_id,
                    auth.now,
                    CallbackUsage {
                        request_count: 1,
                        ..CallbackUsage::default()
                    },
                )
                .await?;
            }
        }
        let mut audit_events = Vec::new();
        let completed =
            complete_run_group_batch_tx(&mut tx, &mut audit_events, &record, AuditEmitterKind::Api)
                .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(completed)
    }

    async fn timeout_stuck_runs(
        &self,
        record: TimeoutStuckRunsRecord,
    ) -> Result<TimedOutRunsRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        // Fail runs that exceeded their per-run timeout while still in a
        // non-terminal worker state. `start_leased_run` nulls
        // `lease_expires_at` for `running` rows, so lease reclaim alone never
        // frees a crashed run — this reaper is the only path that does.
        let run_sql = format!(
            r#"
            WITH candidate AS (
                SELECT id
                FROM playbook_run
                WHERE status IN ('dispatching', 'running')
                  AND finished_at IS NULL
                  AND cancel_requested_at IS NULL
                  -- Clock defensively: every reachable transition into these
                  -- states sets `started_at`, so COALESCE picks it and the
                  -- behaviour is unchanged. The fallbacks exist so a row that
                  -- somehow reaches a worker state without one is still
                  -- reapable instead of hanging forever — `started_at IS NOT
                  -- NULL` made that row permanently invisible to the only
                  -- path that can terminalise it.
                  AND COALESCE(started_at, leased_at, queued_at)
                      + make_interval(secs => GREATEST(timeout_seconds, 1)) < $1
                ORDER BY COALESCE(started_at, leased_at, queued_at) ASC, id ASC
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            )
            UPDATE playbook_run AS run
            SET status = 'timed_out',
                finished_at = $1,
                lease_expires_at = NULL,
                error_message = COALESCE(
                    run.error_message,
                    'Run exceeded timeout_seconds and was reaped by the executor'
                )
            WHERE run.id IN (SELECT id FROM candidate)
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(run_sql))
            .bind(record.now)
            .bind(record.limit)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if runs.is_empty() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(TimedOutRunsRecord {
                runs,
                run_groups: Vec::new(),
            });
        }
        for run in &runs {
            append_run_event_tx(
                &mut tx,
                CreateRunEventRecord {
                    run_id: run.id,
                    host: run_event_host(run),
                    event_kind: PlaybookRunEventKind::Result,
                    event_type: "run_timed_out".to_string(),
                    message: Some(
                        "Run exceeded timeout_seconds and was reaped by the executor".to_string(),
                    ),
                    payload: serde_json::json!({
                        "status": PlaybookRunStatus::TimedOut.as_str(),
                        "timeout_seconds": run.timeout_seconds,
                    }),
                },
            )
            .await?;
        }
        let mut group_ids = runs.iter().map(|run| run.run_group_id).collect::<Vec<_>>();
        group_ids.sort_unstable();
        group_ids.dedup();
        let mut audit_events = Vec::new();
        let run_groups = rollup_run_groups_tx(
            &mut tx,
            &mut audit_events,
            &group_ids,
            record.now,
            Some(AuditEmitterKind::Worker),
        )
        .await?;
        for group in &run_groups {
            let group_runs = runs
                .iter()
                .filter(|run| run.run_group_id == group.id)
                .cloned()
                .collect::<Vec<_>>();
            audit_terminal_runs_tx(
                &mut tx,
                &mut audit_events,
                group,
                &group_runs,
                AuditEmitterKind::Worker,
                record.now,
            )
            .await?;
        }
        revoke_terminal_callback_tokens_tx(&mut tx, &group_ids, record.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.fanout_recorded_events(audit_events);
        Ok(TimedOutRunsRecord { runs, run_groups })
    }
}

/// Verify and key-share-lock the named secret while a project transaction
/// creates or rebinds its canonical usage row. The lock closes the race with
/// named-secret deletion: deletion takes `FOR UPDATE` before counting usages.
pub(super) async fn ensure_named_secret_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    secret_id: Uuid,
) -> Result<(), AppError> {
    let named_secret = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM secrets_variable
        WHERE id = $1
          AND kind = $2
          AND is_protected = false
        FOR KEY SHARE
        "#,
    )
    .bind(secret_id)
    .bind(SECRET_KIND_NAMED)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    if named_secret.is_none() {
        return Err(AppError::Validation(
            "credential_id must identify an existing named secret".to_string(),
        ));
    }

    Ok(())
}

/// Reconcile the one canonical SCM-credential binding for a playbook project.
/// Both the project mutation and this projection run in the same transaction,
/// so a rejected reference cannot strand a partially-written project or
/// binding.
pub(super) async fn reconcile_project_credential_binding_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    project_id: Uuid,
    credential_secret_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(secret_id) = credential_secret_id {
        ensure_named_secret_tx(tx, secret_id).await?;
        sqlx::query(
            r#"
            INSERT INTO secret_usage_binding
                (consumer_type, consumer_id, field_key,
                 owner_scope_type, owner_scope_id, secret_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (consumer_type, consumer_id, field_key)
            DO UPDATE
            SET owner_scope_type = EXCLUDED.owner_scope_type,
                owner_scope_id = EXCLUDED.owner_scope_id,
                secret_id = EXCLUDED.secret_id,
                updated_at = now()
            "#,
        )
        .bind(PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE)
        .bind(project_id)
        .bind(PLAYBOOK_PROJECT_SCM_CREDENTIAL_FIELD_KEY)
        .bind(PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE)
        .bind(project_id)
        .bind(secret_id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    } else {
        sqlx::query(
            r#"
            DELETE FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            "#,
        )
        .bind(PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE)
        .bind(project_id)
        .bind(PLAYBOOK_PROJECT_SCM_CREDENTIAL_FIELD_KEY)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    Ok(())
}

/// Remove every secret usage projection owned by a deleted playbook project.
/// Deleting by consumer rather than a single field prevents a future
/// project-owned field from permanently blocking a named secret after its
/// project is gone.
async fn delete_project_usage_bindings_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
        "#,
    )
    .bind(PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE)
    .bind(project_id)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    Ok(())
}
