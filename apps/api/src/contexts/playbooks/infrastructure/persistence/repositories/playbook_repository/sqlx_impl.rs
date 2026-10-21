use super::callback_tx::{
    CallbackUsage, append_run_event_tx, append_run_group_event_tx, append_run_group_events_tx,
    authorize_group_callback_tx, authorize_run_callback_tx, charge_group_callback_quota_tx,
    charge_run_callback_quota_tx, complete_run_group_batch_tx, complete_run_tx,
};
use super::cancel_tx::{
    append_cancel_finalized_events_tx, record_canceled_run_metrics, request_run_group_cancel_tx,
    revoke_terminal_callback_tokens_tx, rollup_run_groups_tx,
};
use super::event_rows::{artifact_from_row, job_token_from_row, run_event_from_row};
use super::preview::{
    preview_dynamic_target_set, preview_static_target_set, target_belongs_to_target_set,
};
use super::row_mappers::{
    assignment_from_row, execution_environment_from_row, job_template_from_row, playbook_from_row,
    project_from_row, read, run_from_row, run_group_from_row, schedule_from_row,
    service_principal_from_row, target_preview_item_from_row, target_set_from_row,
    version_from_row,
};
use super::run_metadata::{
    launch_options_metadata, merge_playbook_launch_defaults, run_connection_from_metadata,
    run_credential_decision_from_metadata, run_credential_metadata, run_timeout_policy_metadata,
    run_timeout_seconds_for_target_count,
};
use super::scheduling::apply_scheduled_run_concurrency_policy;
use super::selector::{
    filter_preview_by_connection_protocol, prune_role_from_selector, run_event_host,
};
use super::sql_strings::{
    ARTIFACT_COLUMNS, ASSIGNMENT_COLUMNS, ASSIGNMENT_COLUMNS_A, EXECUTION_ENVIRONMENT_COLUMNS,
    JOB_TEMPLATE_COLUMNS, JOB_TOKEN_COLUMNS, PLAYBOOK_COLUMNS, PROJECT_SELECT, RUN_COLUMNS,
    RUN_EVENT_COLUMNS, RUN_GROUP_COLUMNS, SCHEDULE_COLUMNS, SERVICE_PRINCIPAL_COLUMNS,
    TARGET_PREVIEW_BASE_SQL, TARGET_SET_COLUMNS, VERSION_COLUMNS,
};
use super::version_lookup::{current_version_for, version_by_id};
use super::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunGroupBatchRecord,
    CompleteRunRecord, CompletedRunGroupBatchRecord, CompletedRunRecord, CreateArtifactRecord,
    CreateAssignmentRecord, CreateExecutionEnvironmentRecord, CreateJobTemplateRecord,
    CreateJobTokenRecord, CreatePlaybookRecord, CreatePlaybookVersionRecord, CreateProjectRecord,
    CreateRunEventRecord, CreateRunGroupEventRecord, CreateRunGroupRecord, CreateScheduleRecord,
    CreateServicePrincipalRecord, CreateTargetSetRecord, CreatedRunGroupRecord, DueScheduleRecord,
    FinalizeRunGroupCancelRecord, FinalizedRunGroupCancelRecord, LeaseQueuedRunsRecord,
    LeaseScheduleOccurrencesRecord, LeasedScheduleOccurrenceRecord, MarkRunRunningRecord,
    MarkRunsRunningRecord, PLAYBOOK_ASSIGNMENT_COLUMNS, PLAYBOOK_RUN_COLUMNS,
    PLAYBOOK_RUN_GROUP_COLUMNS, PLAYBOOK_SCHEDULE_COLUMNS, PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS,
    PLAYBOOK_TARGET_SET_COLUMNS, PlaybookAssignmentListFilter, PlaybookRepository,
    PlaybookRunListFilter, PlaybookScheduleListFilter, PlaybookServicePrincipalListFilter,
    PlaybookTargetSetListFilter, ReleaseScheduleOccurrenceRecord, RequeueStartedRunsRecord,
    RevokeServicePrincipalRecord, RunGroupWithRuns, SetActiveRunTimeoutsRecord,
    SqlxPlaybookRepository, StartLeasedRunRecord, StartLeasedRunsRecord, StartedRunRecord,
    TimedOutRunsRecord, TimeoutStuckRunsRecord, UpdateAssignmentRecord, UpdatePlaybookRecord,
    UpdateScheduleRecord, UpdateTargetSetRecord, page_result, playbook_blocker_metadata,
};
use crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupEventListFilter;
use crate::contexts::playbooks::application::{
    PlaybookCatalogItem, PlaybookTargetAssignment, PlaybookTargetAssignments,
};
use crate::contexts::playbooks::domain::{
    PlaybookArtifact, PlaybookAssignment, PlaybookCancelOutcome, PlaybookConnection,
    PlaybookExecutionEnvironment, PlaybookJobTemplate, PlaybookJobToken, PlaybookProject,
    PlaybookRun, PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunGroup, PlaybookRunStatus,
    PlaybookSchedule, PlaybookServicePrincipal, PlaybookStatus, PlaybookTargetSet,
    PlaybookTargetSetMode, PlaybookTargetSetPreview, PlaybookVersion,
};
use crate::contexts::secrets::application::{
    PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE, PLAYBOOK_PROJECT_SCM_CREDENTIAL_FIELD_KEY,
    PLAYBOOK_VERSION_RUNTIME_ENV_CONSUMER_TYPE,
};
use crate::contexts::secrets::domain::SECRET_KIND_NAMED;
use crate::contexts::shared::delete_guard::{
    delete_record_guarded, delete_record_guarded_in_transaction,
};
use crate::contexts::shared::list_query::{
    WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{PageResult, clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};
use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use std::collections::HashMap;
use uuid::Uuid;

#[async_trait]
impl PlaybookRepository for SqlxPlaybookRepository {
    async fn list_catalog(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookCatalogItem>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!(
            r#"
            SELECT {PLAYBOOK_COLUMNS}
            FROM playbook
            WHERE status <> 'archived'
              AND NOT (metadata @> '{{"internal": true}}'::jsonb)
            ORDER BY updated_at DESC, name ASC
            LIMIT $1 OFFSET $2
            "#
        );
        let rows = sqlx::query(&sql)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;

        let total: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)::bigint
            FROM playbook
            WHERE status <> 'archived'
              AND NOT (metadata @> '{"internal": true}'::jsonb)
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let playbook = playbook_from_row(&row)?;
            let current_version = current_version_for(&self.pool, &playbook).await?;
            items.push(PlaybookCatalogItem {
                playbook,
                current_version,
            });
        }
        page_result(items, total, limit, offset, "Playbook catalog")
    }

    async fn get_catalog_item(&self, id: Uuid) -> Result<Option<PlaybookCatalogItem>, AppError> {
        let sql = format!("SELECT {PLAYBOOK_COLUMNS} FROM playbook WHERE id = $1");
        let Some(row) = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        let playbook = playbook_from_row(&row)?;
        let current_version = current_version_for(&self.pool, &playbook).await?;
        Ok(Some(PlaybookCatalogItem {
            playbook,
            current_version,
        }))
    }

    async fn create_playbook(
        &self,
        record: CreatePlaybookRecord,
    ) -> Result<PlaybookCatalogItem, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let sql = format!(
            r#"
            INSERT INTO playbook (
                slug, name, description, status, runtime, risk, owner_user_id,
                category, vars_schema, connection_default, source_kind,
                created_by_user_id, updated_by_user_id, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13)
            RETURNING {PLAYBOOK_COLUMNS}
            "#
        );
        let playbook_row = sqlx::query(&sql)
            .bind(record.slug)
            .bind(record.name)
            .bind(record.description)
            .bind(record.status.as_str())
            .bind(record.runtime.as_str())
            .bind(record.risk.as_str())
            .bind(record.owner_user_id)
            .bind(record.category)
            .bind(record.vars_schema)
            .bind(record.connection_default.as_str())
            .bind(record.source_kind)
            .bind(record.user_id)
            .bind(record.metadata.clone())
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let playbook = playbook_from_row(&playbook_row)?;

        let version_sql = format!(
            r#"
            INSERT INTO playbook_version (
                playbook_id, version, language, runtime, entrypoint, content,
                content_sha256, input_schema, output_schema, token_policy,
                timeout_seconds, changelog, created_by_user_id, metadata
            )
            VALUES ($1, 1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            RETURNING {VERSION_COLUMNS}
            "#
        );
        let version_row = sqlx::query(&version_sql)
            .bind(playbook.id)
            .bind(record.language.as_str())
            .bind(record.runtime.as_str())
            .bind(record.entrypoint)
            .bind(record.content)
            .bind(record.content_sha256)
            .bind(record.input_schema)
            .bind(record.output_schema)
            .bind(record.token_policy)
            .bind(record.timeout_seconds)
            .bind(record.changelog)
            .bind(record.user_id)
            .bind(record.metadata)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let current_version = Some(version_from_row(&version_row)?);
        let version_id = current_version
            .as_ref()
            .map(|version| version.id)
            .ok_or_else(|| AppError::Database("Missing created playbook version".to_string()))?;
        let playbook_sql = format!(
            r#"
            UPDATE playbook
            SET current_version_id = $2,
                updated_at = NOW()
            WHERE id = $1
            RETURNING {PLAYBOOK_COLUMNS}
            "#
        );
        let playbook_row = sqlx::query(&playbook_sql)
            .bind(playbook.id)
            .bind(version_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let playbook = playbook_from_row(&playbook_row)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(PlaybookCatalogItem {
            playbook,
            current_version,
        })
    }

    async fn update_playbook(
        &self,
        record: UpdatePlaybookRecord,
    ) -> Result<Option<PlaybookCatalogItem>, AppError> {
        let sql = format!(
            r#"
            UPDATE playbook
            SET slug = COALESCE($2, slug),
                name = COALESCE($3, name),
                description = COALESCE($4, description),
                status = COALESCE($5, status),
                runtime = COALESCE($6, runtime),
                risk = COALESCE($7, risk),
                owner_user_id = CASE WHEN $8 THEN $9 ELSE owner_user_id END,
                category = COALESCE($10, category),
                metadata = COALESCE($11, metadata),
                updated_by_user_id = COALESCE($12, updated_by_user_id),
                default_target_set_id = CASE WHEN $13 THEN $14 ELSE default_target_set_id END,
                vars_schema = CASE WHEN $15 THEN $16 ELSE vars_schema END,
                connection_default = COALESCE($17, connection_default),
                updated_at = NOW()
            WHERE id = $1
            RETURNING {PLAYBOOK_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(record.id)
            .bind(record.slug)
            .bind(record.name)
            .bind(record.description)
            .bind(record.status.map(|status| status.as_str().to_string()))
            .bind(record.runtime.map(|runtime| runtime.as_str().to_string()))
            .bind(record.risk.map(|risk| risk.as_str().to_string()))
            .bind(record.owner_user_id.is_some())
            .bind(record.owner_user_id.flatten())
            .bind(record.category)
            .bind(record.metadata)
            .bind(record.user_id)
            .bind(record.default_target_set_id.is_some())
            .bind(record.default_target_set_id.flatten())
            .bind(record.vars_schema.is_some())
            .bind(record.vars_schema.flatten())
            .bind(
                record
                    .connection_default
                    .map(|connection| connection.as_str().to_string()),
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let playbook = playbook_from_row(&row)?;
        let current_version = current_version_for(&self.pool, &playbook).await?;
        Ok(Some(PlaybookCatalogItem {
            playbook,
            current_version,
        }))
    }

    async fn delete_playbook(&self, id: Uuid) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        // Opt this transaction into the version-immutability delete carve-out so
        // the trigger permits removing the playbook's immutable versions. The GUC
        // is transaction-local (`SET LOCAL`), so it never leaks past this delete.
        sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        // Sweep the author-time secret-usage projection for EVERY version
        // of this playbook. `secret_usage_binding.consumer_id` is polymorphic —
        // there is no FK onto `playbook_version` — so without this delete the
        // projected rows dangle after the versions go, permanently blocking
        // named-secret deletion (usage-check 409) and inflating usage counts.
        // While the playbook exists, bindings of superseded versions stay put on
        // purpose: usage is counted across ALL versions of a live playbook.
        sqlx::query(
            "DELETE FROM secret_usage_binding \
             WHERE consumer_type = $2 AND owner_scope_type = 'playbook' AND owner_scope_id = $1",
        )
        .bind(id)
        .bind(PLAYBOOK_VERSION_RUNTIME_ENV_CONSUMER_TYPE)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        // The parent delete cascades immutable versions while the transaction-
        // local GUC is active. Run history holds both the parent and its versions
        // with RESTRICT, so the guarded parent delete reports the direct run/run-
        // group blockers and the outer transaction rolls this cleanup back.
        let deleted = delete_record_guarded_in_transaction(
            &mut tx,
            "playbook",
            id,
            playbook_blocker_metadata,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(deleted > 0)
    }

    async fn list_versions(
        &self,
        playbook_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookVersion>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!(
            "SELECT {VERSION_COLUMNS} FROM playbook_version WHERE playbook_id = $1 ORDER BY version DESC LIMIT $2 OFFSET $3"
        );
        let rows = sqlx::query(&sql)
            .bind(playbook_id)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(version_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM playbook_version WHERE playbook_id = $1",
        )
        .bind(playbook_id)
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook version")
    }

    async fn create_version(
        &self,
        record: CreatePlaybookVersionRecord,
    ) -> Result<Option<PlaybookVersion>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let lock_sql = format!("SELECT {PLAYBOOK_COLUMNS} FROM playbook WHERE id = $1 FOR UPDATE");
        let Some(_) = sqlx::query(&lock_sql)
            .bind(record.playbook_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };

        let version_sql = format!(
            r#"
            INSERT INTO playbook_version (
                playbook_id, version, language, runtime, entrypoint, content,
                content_sha256, input_schema, output_schema, token_policy,
                timeout_seconds, changelog, created_by_user_id, metadata
            )
            SELECT
                $1,
                COALESCE(MAX(version), 0) + 1,
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
                $13
            FROM playbook_version
            WHERE playbook_id = $1
            RETURNING {VERSION_COLUMNS}
            "#
        );
        let row = sqlx::query(&version_sql)
            .bind(record.playbook_id)
            .bind(record.language.as_str())
            .bind(record.runtime.as_str())
            .bind(record.entrypoint)
            .bind(record.content)
            .bind(record.content_sha256)
            .bind(record.input_schema)
            .bind(record.output_schema)
            .bind(record.token_policy)
            .bind(record.timeout_seconds)
            .bind(record.changelog)
            .bind(record.user_id)
            .bind(record.metadata)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let version = version_from_row(&row)?;

        sqlx::query(
            r#"
            UPDATE playbook
            SET current_version_id = $2,
                updated_by_user_id = COALESCE($3, updated_by_user_id),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(record.playbook_id)
        .bind(version.id)
        .bind(record.user_id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(version))
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
        let rows = sqlx::query(&sql)
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
        let row = sqlx::query(&sql)
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
        let row = sqlx::query(&sql)
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
    ) -> Result<PageResult<PlaybookProject>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!("SELECT {PROJECT_SELECT} ORDER BY p.name ASC, p.id LIMIT $1 OFFSET $2");
        let rows = sqlx::query(&sql)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(project_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM playbook_project")
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook project")
    }

    async fn get_project(&self, id: Uuid) -> Result<Option<PlaybookProject>, AppError> {
        fetch_project_by_id(&self.pool, id).await
    }

    async fn create_project(
        &self,
        record: CreateProjectRecord,
    ) -> Result<PlaybookProject, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let row = sqlx::query(
            r#"
            INSERT INTO playbook_project (
                name, description, scm_type, scm_url, scm_branch, scm_ref,
                playbook_base_path, default_execution_environment_id,
                sync_policy, metadata, created_by_user_id, updated_by_user_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)
            RETURNING id
            "#,
        )
        .bind(record.name)
        .bind(record.description)
        .bind(record.scm_type)
        .bind(record.scm_url)
        .bind(record.scm_branch)
        .bind(record.scm_ref)
        .bind(record.playbook_base_path)
        .bind(record.default_execution_environment_id)
        .bind(record.sync_policy)
        .bind(record.metadata)
        .bind(record.user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
        reconcile_project_credential_binding_tx(&mut tx, id, record.credential_id).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        fetch_project_by_id(&self.pool, id).await?.ok_or_else(|| {
            AppError::Internal("Created playbook project could not be reloaded".to_string())
        })
    }

    async fn list_job_templates(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookJobTemplate>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));
        let sql = format!(
            "SELECT {JOB_TEMPLATE_COLUMNS} FROM playbook_job_template ORDER BY name ASC, id LIMIT $1 OFFSET $2"
        );
        let rows = sqlx::query(&sql)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(job_template_from_row)
            .collect::<Result<_, _>>()?;
        let total = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM playbook_job_template")
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        page_result(items, total, limit, offset, "Playbook job template")
    }

    async fn get_job_template(&self, id: Uuid) -> Result<Option<PlaybookJobTemplate>, AppError> {
        let sql = format!("SELECT {JOB_TEMPLATE_COLUMNS} FROM playbook_job_template WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(job_template_from_row).transpose()
    }

    async fn create_job_template(
        &self,
        record: CreateJobTemplateRecord,
    ) -> Result<PlaybookJobTemplate, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_job_template (
                name, description, playbook_id, project_id, playbook_path,
                inventory_target_set_id, execution_environment_id, credential_refs,
                extra_vars, launch_options, ask_variables_on_launch, ask_limit_on_launch,
                ask_tags_on_launch, ask_skip_tags_on_launch, ask_credential_on_launch,
                ask_execution_environment_on_launch, enabled, metadata,
                created_by_user_id, updated_by_user_id
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $19
            )
            RETURNING {JOB_TEMPLATE_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(record.name)
            .bind(record.description)
            .bind(record.playbook_id)
            .bind(record.project_id)
            .bind(record.playbook_path)
            .bind(record.inventory_target_set_id)
            .bind(record.execution_environment_id)
            .bind(record.credential_refs)
            .bind(record.extra_vars)
            .bind(record.launch_options)
            .bind(record.ask_variables_on_launch)
            .bind(record.ask_limit_on_launch)
            .bind(record.ask_tags_on_launch)
            .bind(record.ask_skip_tags_on_launch)
            .bind(record.ask_credential_on_launch)
            .bind(record.ask_execution_environment_on_launch)
            .bind(record.enabled)
            .bind(record.metadata)
            .bind(record.user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        job_template_from_row(&row)
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
        let row = sqlx::query(&sql)
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
            SELECT
                (SELECT COUNT(*) FROM playbook_project
                   WHERE default_execution_environment_id = $1)
              + (SELECT COUNT(*) FROM playbook_job_template
                   WHERE execution_environment_id = $1)
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
    ) -> Result<Option<PlaybookProject>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let row = sqlx::query(
            r#"
            UPDATE playbook_project
            SET name = $2, description = $3, scm_type = $4, scm_url = $5,
                scm_branch = $6, scm_ref = $7, playbook_base_path = $8,
                default_execution_environment_id = $9,
                sync_policy = $10, metadata = $11,
                updated_by_user_id = COALESCE($12, updated_by_user_id),
                updated_at = now()
            WHERE id = $1
            RETURNING id
            "#,
        )
        .bind(id)
        .bind(record.name)
        .bind(record.description)
        .bind(record.scm_type)
        .bind(record.scm_url)
        .bind(record.scm_branch)
        .bind(record.scm_ref)
        .bind(record.playbook_base_path)
        .bind(record.default_execution_environment_id)
        .bind(record.sync_policy)
        .bind(record.metadata)
        .bind(record.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let Some(row) = row else {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let persisted_id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
        reconcile_project_credential_binding_tx(&mut tx, persisted_id, record.credential_id)
            .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        fetch_project_by_id(&self.pool, persisted_id).await
    }

    async fn set_project_files(
        &self,
        id: Uuid,
        project_files: Value,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookProject>, AppError> {
        // jsonb_set only touches the `project_files` key so any other metadata
        // the operator stored on the project is preserved verbatim.
        let row = sqlx::query(
            r#"
            UPDATE playbook_project
            SET metadata = jsonb_set(COALESCE(metadata, '{}'::jsonb), '{project_files}', $2, true),
                updated_by_user_id = COALESCE($3, updated_by_user_id),
                updated_at = now()
            WHERE id = $1
            RETURNING id
            "#,
        )
        .bind(id)
        .bind(project_files)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let persisted_id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
        fetch_project_by_id(&self.pool, persisted_id).await
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
    ) -> Result<Option<PlaybookJobTemplate>, AppError> {
        let sql = format!(
            r#"
            UPDATE playbook_job_template
            SET name = $2, description = $3, playbook_id = $4, project_id = $5,
                playbook_path = $6, inventory_target_set_id = $7,
                execution_environment_id = $8, credential_refs = $9, extra_vars = $10,
                launch_options = $11, ask_variables_on_launch = $12,
                ask_limit_on_launch = $13, ask_tags_on_launch = $14,
                ask_skip_tags_on_launch = $15, ask_credential_on_launch = $16,
                ask_execution_environment_on_launch = $17, enabled = $18, metadata = $19,
                updated_by_user_id = COALESCE($20, updated_by_user_id), updated_at = now()
            WHERE id = $1
            RETURNING {JOB_TEMPLATE_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(id)
            .bind(record.name)
            .bind(record.description)
            .bind(record.playbook_id)
            .bind(record.project_id)
            .bind(record.playbook_path)
            .bind(record.inventory_target_set_id)
            .bind(record.execution_environment_id)
            .bind(record.credential_refs)
            .bind(record.extra_vars)
            .bind(record.launch_options)
            .bind(record.ask_variables_on_launch)
            .bind(record.ask_limit_on_launch)
            .bind(record.ask_tags_on_launch)
            .bind(record.ask_skip_tags_on_launch)
            .bind(record.ask_credential_on_launch)
            .bind(record.ask_execution_environment_on_launch)
            .bind(record.enabled)
            .bind(record.metadata)
            .bind(record.user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(job_template_from_row).transpose()
    }

    async fn delete_job_template(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM playbook_job_template WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
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
        let row = sqlx::query(&sql)
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
        let sql = format!(
            r#"
            UPDATE playbook_target_set
            SET name = $1, mode = $2, entity_kind = $3, selector = $4,
                updated_by_user_id = $5, updated_at = now()
            WHERE id = $6
            RETURNING {TARGET_SET_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(record.name)
            .bind(record.mode.as_str())
            .bind(record.entity_kind)
            .bind(record.selector)
            .bind(record.user_id)
            .bind(record.id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(target_set_from_row).transpose()
    }

    async fn delete_target_set(&self, id: Uuid) -> Result<bool, AppError> {
        let deleted = delete_record_guarded(
            &self.pool,
            "playbook_target_set",
            id,
            playbook_blocker_metadata,
        )
        .await?;
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
    ) -> Result<(Vec<PlaybookTargetSet>, i64), AppError> {
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
        let items = rows
            .iter()
            .map(target_set_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn get_target_set(&self, id: Uuid) -> Result<Option<PlaybookTargetSet>, AppError> {
        let sql = format!("SELECT {TARGET_SET_COLUMNS} FROM playbook_target_set WHERE id = $1");
        let Some(row) = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        target_set_from_row(&row).map(Some)
    }

    async fn preview_target_set(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetSetPreview>, AppError> {
        let sql = format!("SELECT {TARGET_SET_COLUMNS} FROM playbook_target_set WHERE id = $1");
        let Some(row) = sqlx::query(&sql)
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

    async fn create_assignment(
        &self,
        record: CreateAssignmentRecord,
    ) -> Result<PlaybookAssignment, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_assignment (
                playbook_id, target_set_id, default_version_id, name, scope_kind,
                scope_id, priority, enabled, visible, executable, quick_action_enabled,
                default_input, default_metadata, default_timeout_seconds, default_target_limit,
                created_by_user_id, updated_by_user_id, metadata
            )
            SELECT
                playbook.id,
                target_set.id,
                default_version.id,
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
                $16,
                $16,
                $17
            FROM playbook
            JOIN playbook_target_set AS target_set
              ON target_set.id = $2
            LEFT JOIN playbook_version AS default_version
              ON default_version.id = $3
             AND default_version.playbook_id = playbook.id
            WHERE playbook.id = $1
              AND ($3::uuid IS NULL OR default_version.id IS NOT NULL)
              AND (NOT $11 OR playbook.risk = 'low')
            RETURNING {ASSIGNMENT_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(&sql)
            .bind(record.playbook_id)
            .bind(record.target_set_id)
            .bind(record.default_version_id)
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
                "Playbook, target set, or default version not found".to_string(),
            ));
        };
        assignment_from_row(&row)
    }

    async fn get_assignment(&self, id: Uuid) -> Result<Option<PlaybookAssignment>, AppError> {
        let sql = format!("SELECT {ASSIGNMENT_COLUMNS} FROM playbook_assignment WHERE id = $1");
        let Some(row) = sqlx::query(&sql)
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
        let replace_default_version_id = record.default_version_id.is_some();
        let default_version_id = record.default_version_id.flatten();
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
                    COALESCE($2, playbook_id) AS playbook_id,
                    COALESCE($3, target_set_id) AS target_set_id,
                    CASE WHEN $4 THEN $5 ELSE default_version_id END AS default_version_id,
                    CASE WHEN $6 THEN $7 ELSE name END AS name,
                    $8 AS scope_kind,
                    $9 AS scope_id,
                    COALESCE($10, priority) AS priority,
                    COALESCE($11, enabled) AS enabled,
                    COALESCE($12, visible) AS visible,
                    COALESCE($13, executable) AS executable,
                    COALESCE($14, quick_action_enabled) AS quick_action_enabled,
                    COALESCE($15, default_input) AS default_input,
                    COALESCE($16, default_metadata) AS default_metadata,
                    CASE WHEN $17 THEN $18 ELSE default_timeout_seconds END AS default_timeout_seconds,
                    CASE WHEN $19 THEN $20 ELSE default_target_limit END AS default_target_limit,
                    COALESCE($21, metadata) AS metadata
                FROM existing
            ),
            validated AS (
                SELECT candidate.*
                FROM candidate
                JOIN playbook
                  ON playbook.id = candidate.playbook_id
                JOIN playbook_target_set AS target_set
                  ON target_set.id = candidate.target_set_id
                LEFT JOIN playbook_version AS default_version
                  ON default_version.id = candidate.default_version_id
                 AND default_version.playbook_id = candidate.playbook_id
                WHERE (candidate.default_version_id IS NULL OR default_version.id IS NOT NULL)
                  AND (NOT candidate.quick_action_enabled OR playbook.risk = 'low')
            ),
            updated AS (
                UPDATE playbook_assignment
                SET playbook_id = validated.playbook_id,
                    target_set_id = validated.target_set_id,
                    default_version_id = validated.default_version_id,
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
                    updated_by_user_id = COALESCE($22, updated_by_user_id),
                    updated_at = NOW()
                FROM validated
                WHERE playbook_assignment.id = $1
                RETURNING playbook_assignment.*
            )
            SELECT {ASSIGNMENT_COLUMNS}
            FROM updated
            "#
        );
        let row = sqlx::query(&sql)
            .bind(record.id)
            .bind(record.playbook_id)
            .bind(record.target_set_id)
            .bind(replace_default_version_id)
            .bind(default_version_id)
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
            "playbook_id",
            &filter.playbook_id,
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
    ) -> Result<Option<PlaybookTargetAssignments>, AppError> {
        let target_sql = format!(
            r#"
            {TARGET_PREVIEW_BASE_SQL}
            WHERE connection_targets.target_id = $1
            ORDER BY connection_targets.source_kind ASC, connection_targets.name ASC
            LIMIT 1
            "#
        );
        let Some(target_row) = sqlx::query(&target_sql)
            .bind(target_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        let target = target_preview_item_from_row(&target_row)?;
        if target.address.is_none() || !target.address_bound {
            return Ok(Some(PlaybookTargetAssignments {
                target,
                items: Vec::new(),
            }));
        }

        let sql = format!(
            r#"
            SELECT {ASSIGNMENT_COLUMNS_A}
            FROM playbook_assignment a
            JOIN playbook p ON p.id = a.playbook_id
            WHERE a.enabled = TRUE
              AND a.visible = TRUE
              AND a.executable = TRUE
              AND p.status = 'active'
              AND EXISTS (
                  SELECT 1
                  FROM playbook_version v
                  WHERE v.playbook_id = p.id
                    AND (a.default_version_id IS NULL OR v.id = a.default_version_id)
              )
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
        let rows = sqlx::query(&sql)
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
            if let Some(playbook) = self.get_catalog_item(assignment.playbook_id).await? {
                items.push(PlaybookTargetAssignment {
                    assignment,
                    playbook,
                });
            }
        }
        Ok(Some(PlaybookTargetAssignments { target, items }))
    }

    async fn create_schedule(
        &self,
        record: CreateScheduleRecord,
    ) -> Result<PlaybookSchedule, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_schedule (
                playbook_id, target_set_id, service_principal_id, name,
                cron_expression, timezone, misfire_policy, concurrency_policy,
                enabled, next_run_at, created_by_user_id, updated_by_user_id, metadata
            )
            SELECT
                playbook.id,
                target_set.id,
                principal.id,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $11,
                $12
            FROM playbook
            JOIN playbook_target_set AS target_set
              ON target_set.id = $2
            JOIN playbook_service_principal AS principal
              ON principal.id = $3
             AND principal.status = 'active'
             AND principal.revoked_at IS NULL
            WHERE playbook.id = $1
            RETURNING {SCHEDULE_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(&sql)
            .bind(record.playbook_id)
            .bind(record.target_set_id)
            .bind(record.service_principal_id)
            .bind(record.name)
            .bind(record.cron_expression)
            .bind(record.timezone)
            .bind(record.misfire_policy.as_str())
            .bind(record.concurrency_policy.as_str())
            .bind(record.enabled)
            .bind(record.next_run_at)
            .bind(record.user_id)
            .bind(record.metadata)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound(
                "Playbook, target set, or active service principal not found".to_string(),
            ));
        };
        schedule_from_row(&row)
    }

    async fn get_service_principal(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError> {
        let sql = format!(
            "SELECT {SERVICE_PRINCIPAL_COLUMNS} FROM playbook_service_principal WHERE id = $1"
        );
        sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(service_principal_from_row)
            .transpose()
    }

    async fn list_service_principals(
        &self,
        filter: &PlaybookServicePrincipalListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookServicePrincipal>, i64), AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            "SELECT {SERVICE_PRINCIPAL_COLUMNS}, COUNT(*) OVER() AS total_count \
             FROM playbook_service_principal"
        ));
        // Multi-value `status` IN filter (plain text column, matches any).
        if !filter.status.is_empty() {
            builder.push(" WHERE status IN (");
            let mut separated = builder.separated(", ");
            for status in &filter.status {
                separated.push_bind(status.as_str());
            }
            builder.push(")");
        }
        push_order_by(
            &mut builder,
            PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS,
            filter.sort.as_ref(),
            "status ASC, updated_at DESC, name ASC",
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
                AppError::Database(format!(
                    "Failed to read service principal total_count: {error}"
                ))
            })?,
            None => 0,
        };
        let items = rows
            .iter()
            .map(service_principal_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create_service_principal(
        &self,
        record: CreateServicePrincipalRecord,
    ) -> Result<PlaybookServicePrincipal, AppError> {
        let sql = format!(
            r#"
            INSERT INTO playbook_service_principal (
                name, slug, description, created_by_user_id,
                updated_by_user_id, metadata
            )
            VALUES ($1, $2, $3, $4, $4, $5)
            RETURNING {SERVICE_PRINCIPAL_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(record.name)
            .bind(record.slug)
            .bind(record.description)
            .bind(record.user_id)
            .bind(record.metadata)
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        service_principal_from_row(&row)
    }

    async fn revoke_service_principal(
        &self,
        record: RevokeServicePrincipalRecord,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError> {
        // Revoke is terminal and idempotent on the row itself: a principal that is
        // already revoked stays revoked (the `revoked_*` audit columns are not
        // overwritten on a repeat call). The `chk_playbook_service_principal_revoked_pair`
        // constraint is satisfied because `status` and `revoked_at` are set together.
        let sql = format!(
            r#"
            UPDATE playbook_service_principal
            SET status = 'revoked',
                revoked_at = COALESCE(revoked_at, $2),
                revoked_by_user_id = COALESCE(revoked_by_user_id, $3),
                revocation_reason = COALESCE(revocation_reason, $4),
                updated_by_user_id = $3,
                updated_at = $2
            WHERE id = $1
            RETURNING {SERVICE_PRINCIPAL_COLUMNS}
            "#
        );
        sqlx::query(&sql)
            .bind(record.id)
            .bind(record.now)
            .bind(record.user_id)
            .bind(record.revocation_reason)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(service_principal_from_row)
            .transpose()
    }

    async fn get_schedule(&self, id: Uuid) -> Result<Option<PlaybookSchedule>, AppError> {
        let sql = format!("SELECT {SCHEDULE_COLUMNS} FROM playbook_schedule WHERE id = $1");
        sqlx::query(&sql)
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
            "playbook_id",
            &filter.playbook_id,
        );
        push_uuid_in(
            &mut builder,
            &mut where_state,
            "target_set_id",
            &filter.target_set_id,
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
              AND EXISTS (
                  SELECT 1
                  FROM playbook_service_principal AS principal
                  WHERE principal.id = schedule.service_principal_id
                    AND principal.status = 'active'
                    AND principal.revoked_at IS NULL
              )
            ORDER BY schedule.next_run_at ASC, schedule.id ASC
            LIMIT $1
            "#
        );
        let rows = sqlx::query(&sql)
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
            let schedule_row = sqlx::query(&schedule_sql)
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
                  AND EXISTS (
                      SELECT 1
                      FROM playbook_service_principal AS principal
                      WHERE principal.id = schedule.service_principal_id
                        AND principal.status = 'active'
                        AND principal.revoked_at IS NULL
                  )
                FOR UPDATE OF schedule SKIP LOCKED
                "#
            );
            let Some(locked_schedule_row) = sqlx::query(&lock_sql)
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
            let schedule_row = sqlx::query(&update_sql)
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
        // When a new target set is requested, validate the FK reference up front so
        // a missing target set surfaces as a clean 404 instead of a constraint
        // violation. The schedule->target_set FK is ON DELETE RESTRICT, mirroring
        // the create path's JOIN-based validation.
        if let Some(target_set_id) = record.target_set_id
            && self.get_target_set(target_set_id).await?.is_none()
        {
            return Err(AppError::NotFound("Target set not found".to_string()));
        }
        // `chk_playbook_schedule_enabled_due` requires a non-null cursor whenever
        // the row ends up enabled. The service rejects enabling without a usable
        // `next_run_at`, so here we only COALESCE the provided value over the
        // existing one.
        let sql = format!(
            r#"
            UPDATE playbook_schedule
            SET target_set_id = COALESCE($2, target_set_id),
                name = COALESCE($3, name),
                cron_expression = COALESCE($4, cron_expression),
                timezone = COALESCE($5, timezone),
                misfire_policy = COALESCE($6, misfire_policy),
                concurrency_policy = COALESCE($7, concurrency_policy),
                enabled = COALESCE($8, enabled),
                next_run_at = COALESCE($9, next_run_at),
                metadata = COALESCE($10, metadata),
                updated_by_user_id = COALESCE($11, updated_by_user_id),
                updated_at = NOW()
            WHERE id = $1
            RETURNING {SCHEDULE_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(record.id)
            .bind(record.target_set_id)
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
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(schedule_from_row).transpose()
    }

    async fn delete_schedule(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM playbook_schedule WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(result.rows_affected() > 0)
    }

    async fn create_run_group(
        &self,
        record: CreateRunGroupRecord,
    ) -> Result<CreatedRunGroupRecord, AppError> {
        let Some(playbook) = self.get_catalog_item(record.playbook_id).await? else {
            return Err(AppError::NotFound("Playbook not found".to_string()));
        };
        if playbook.playbook.status != PlaybookStatus::Active {
            return Err(AppError::Validation(
                "Only active playbooks can be executed".to_string(),
            ));
        }
        let Some(version) = playbook.current_version else {
            return Err(AppError::Validation(
                "Playbook has no executable version".to_string(),
            ));
        };
        let metadata =
            merge_playbook_launch_defaults(&playbook.playbook.metadata, &record.metadata);
        let connection =
            run_connection_from_metadata(playbook.playbook.connection_default, &metadata)?;
        let Some(mut preview) = self
            .preview_target_set(record.target_set_id, record.target_limit)
            .await?
        else {
            return Err(AppError::NotFound("Target set not found".to_string()));
        };
        if matches!(connection, PlaybookConnection::Ssh) {
            preview = filter_preview_by_connection_protocol(&self.pool, preview, "ssh").await?;
        }
        if preview.items.is_empty() {
            let message = if matches!(connection, PlaybookConnection::Ssh) {
                "Target set resolves to zero SSH-capable targets"
            } else {
                "Target set resolves to zero targets"
            };
            return Err(AppError::Validation(message.to_string()));
        }
        if preview.truncated {
            return Err(AppError::Validation(format!(
                "Target set resolves to more than {} targets; narrow the selector or raise the run limit",
                preview.limit
            )));
        }
        let total_runs = i32::try_from(preview.items.len()).map_err(|_| {
            AppError::Validation("Target set is too large for one run group".to_string())
        })?;
        let credential_decision = run_credential_decision_from_metadata(&metadata)?;
        let launch_options = launch_options_metadata(&metadata);
        let run_timeout_seconds = run_timeout_seconds_for_target_count(
            &metadata,
            version.timeout_seconds,
            preview.items.len(),
        )?;
        let timeout_policy = run_timeout_policy_metadata(
            &metadata,
            version.timeout_seconds,
            preview.items.len(),
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
        let group_metadata = Value::Object(group_metadata);

        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        apply_scheduled_run_concurrency_policy(&mut tx, &record).await?;
        let group_sql = format!(
            r#"
            INSERT INTO playbook_run_group (
                playbook_id, playbook_version_id, target_set_id, schedule_id, scheduled_for,
                requested_by_user_id, service_principal_id, trigger_kind, status,
                total_runs, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', $9, $10)
            RETURNING {RUN_GROUP_COLUMNS}
            "#
        );
        let group_row = sqlx::query(&group_sql)
            .bind(playbook.playbook.id)
            .bind(version.id)
            .bind(preview.target_set.id)
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

        struct RunInsertInput {
            ord: i64,
            id: Uuid,
            target_kind: String,
            target_id: Uuid,
            source_kind: String,
            source_id: Uuid,
            target_name: String,
            target_address: Option<String>,
            target_address_bound: bool,
            metadata: Value,
        }

        let mut run_inputs = Vec::with_capacity(preview.items.len());
        let credential_metadata = run_credential_metadata(&record, &credential_decision);
        for (index, item) in preview.items.iter().enumerate() {
            let mut run_metadata = serde_json::Map::new();
            run_metadata.insert(
                "target".to_string(),
                json!({
                    "source_kind": item.source_kind,
                    "source_id": item.source_id,
                    "address": item.address,
                    "address_bound": item.address_bound,
                    "site_id": item.site_id,
                    "site_name": item.site_name,
                    "region_id": item.region_id,
                    "region_name": item.region_name,
                    "tenant_id": item.tenant_id,
                    "tenant_name": item.tenant_name,
                    "role_id": item.role_id,
                    "role_name": item.role_name,
                    "platform_id": item.platform_id,
                    "platform_name": item.platform_name,
                    "device_type_id": item.device_type_id,
                    "device_type_name": item.device_type_name,
                    "cluster_id": item.cluster_id,
                    "cluster_name": item.cluster_name,
                    "status": item.status,
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
                        "Target set is too large to preserve run order".to_string(),
                    )
                })?,
                id: Uuid::now_v7(),
                target_kind: item.target_kind.clone(),
                target_id: item.target_id,
                source_kind: item.source_kind.clone(),
                source_id: item.source_id,
                target_name: item.name.clone(),
                target_address: item.address.clone(),
                target_address_bound: item.address_bound,
                metadata: Value::Object(run_metadata),
            });
        }
        let mut run_insert = QueryBuilder::<Postgres>::new(
            r#"
            WITH run_input (
                ord, id, target_kind, target_id, source_kind, source_id,
                target_name, target_address, target_address_bound, metadata
            ) AS (
            "#,
        );
        run_insert.push_values(&run_inputs, |mut row, input| {
            row.push_bind(input.ord)
                .push_bind(input.id)
                .push_bind(input.target_kind.clone())
                .push_bind(input.target_id)
                .push_bind(input.source_kind.clone())
                .push_bind(input.source_id)
                .push_bind(input.target_name.clone())
                .push_bind(input.target_address.clone())
                .push_bind(input.target_address_bound)
                .push_bind(input.metadata.clone());
        });
        run_insert.push(
            r#"
            ),
            inserted AS (
                INSERT INTO playbook_run (
                    id, run_group_id, playbook_id, playbook_version_id, target_kind,
                    target_id, source_kind, source_id, target_name, target_address,
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
            .push(", ")
            .push_bind(playbook.playbook.id)
            .push(", ")
            .push_bind(version.id)
            .push(
                r#",
                    run_input.target_kind,
                    run_input.target_id,
                    run_input.source_kind,
                    run_input.source_id,
                    run_input.target_name,
                    run_input.target_address,
                    run_input.target_address_bound,
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
        if runs.len() != preview.items.len() {
            return Err(AppError::Database(
                "Not every target received a playbook run".to_string(),
            ));
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(CreatedRunGroupRecord {
            run_group,
            runs,
            target_set: preview.target_set,
        })
    }

    async fn get_run(&self, run_id: Uuid) -> Result<Option<PlaybookRun>, AppError> {
        let sql = format!("SELECT {RUN_COLUMNS} FROM playbook_run WHERE id = $1");
        sqlx::query(&sql)
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
        let push_where = |builder: &mut QueryBuilder<'_, Postgres>| {
            let mut where_state = WhereState::new();
            push_uuid_in(
                builder,
                &mut where_state,
                "playbook_id",
                &filter.playbook_id,
            );
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
            "playbook_id",
            &filter.playbook_id,
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
        let Some(group_row) = sqlx::query(&group_sql)
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
        let run_rows = sqlx::query(&runs_sql)
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
        let Some(run_row) = sqlx::query(&run_sql)
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
        let (detail, newly_canceled_runs) =
            request_run_group_cancel_tx(&mut tx, existing.run_group_id, i64::MAX, now, deadline_at)
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
        let requested =
            request_run_group_cancel_tx(&mut tx, run_group_id, limit, now, deadline_at).await?;
        let Some((detail, newly_canceled_runs)) = requested else {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
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
        let rows = sqlx::query(&sql)
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
        let Some(group_row) = sqlx::query(&group_sql)
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
        let run_rows = sqlx::query(&update_sql)
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
        append_cancel_finalized_events_tx(&mut tx, &canceled_runs, record.outcome, message).await?;
        let run_group = rollup_run_groups_tx(&mut tx, &[record.run_group_id], record.now)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Database("Playbook run group not found".to_string()))?;
        // The deadline path terminalizes whatever the runner never confirmed,
        // so the group's callback token must be revoked here too.
        revoke_terminal_callback_tokens_tx(&mut tx, &[record.run_group_id], record.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
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
                $7
            FROM playbook_run AS run
            WHERE run.id = $2
            RETURNING {JOB_TOKEN_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(&sql)
            .bind(record.id)
            .bind(record.run_id)
            .bind(record.token_hash)
            .bind(record.token_prefix)
            .bind(record.scopes)
            .bind(record.expires_at)
            .bind(record.metadata)
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
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<PlaybookJobToken, AppError> {
        let sql = format!(
            r#"
            UPDATE playbook_job_token
            SET last_used_at = $3
            WHERE id = $1
              AND token_hash = $2
              AND status = 'active'
              AND expires_at > $3
              AND EXISTS (
                  SELECT 1
                  FROM playbook_run AS run
                  WHERE run.id = playbook_job_token.run_id
                    AND run.target_kind = playbook_job_token.target_kind
                    AND run.target_id = playbook_job_token.target_id
                    AND run.source_kind = playbook_job_token.source_kind
                    AND run.source_id = playbook_job_token.source_id
                    AND run.status IN ('leased', 'dispatching', 'running')
                    AND run.finished_at IS NULL
              )
            RETURNING {JOB_TOKEN_COLUMNS}
            "#
        );
        let Some(row) = sqlx::query(&sql)
            .bind(token_id)
            .bind(token_hash)
            .bind(now)
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
    ) -> Result<PlaybookArtifact, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_group_id =
            authorize_run_callback_tx(&mut tx, &auth, record.run_id, "artifact:create").await?;
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
        let row = sqlx::query(&sql)
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
        let rows = sqlx::query(&sql)
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
        let run_group_id =
            authorize_run_callback_tx(&mut tx, &auth, record.run_id, "event:append").await?;
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

        let events = append_run_group_events_tx(&mut tx, records).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
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

        let events = append_run_group_events_tx(
            &mut tx,
            records.into_iter().map(|(record, _)| record).collect(),
        )
        .await?;
        revoke_terminal_callback_tokens_tx(&mut tx, &[run_group_id], auth.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
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
        let rows = sqlx::query(&sql)
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
        let rows = sqlx::query(&sql)
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

    async fn lease_queued_runs(
        &self,
        record: LeaseQueuedRunsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('playbook_run_lease'))")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let sql = format!(
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
            ), eligible AS (
                SELECT
                    run.id,
                    run.run_group_id,
                    run.queued_at,
                    COALESCE(active_groups.active_count, 0)::bigint AS active_count,
                    ROW_NUMBER() OVER (
                        PARTITION BY run.run_group_id
                        ORDER BY run.queued_at ASC, run.id ASC
                    )::bigint AS queued_rank
                FROM playbook_run AS run
                LEFT JOIN active_groups ON active_groups.run_group_id = run.run_group_id
                WHERE run.cancel_requested_at IS NULL
                  AND (
                    run.status = 'queued'
                    OR (run.status = 'leased' AND run.lease_expires_at <= $3)
                  )
            ), inactive_group_order AS (
                SELECT
                    run_group_id,
                    ROW_NUMBER() OVER (ORDER BY MIN(queued_at) ASC, run_group_id ASC)::bigint AS group_rank
                FROM eligible
                WHERE active_count = 0
                GROUP BY run_group_id
            ), candidate AS (
                SELECT run.id
                FROM playbook_run AS run
                JOIN eligible ON eligible.id = run.id
                CROSS JOIN active_group_total
                LEFT JOIN inactive_group_order ON inactive_group_order.run_group_id = eligible.run_group_id
                WHERE eligible.queued_rank <= GREATEST($6 - eligible.active_count, 0)
                  AND (
                    eligible.active_count > 0
                    OR inactive_group_order.group_rank <= GREATEST($5 - active_group_total.total, 0)
                  )
                ORDER BY eligible.queued_at ASC, run.id ASC
                LIMIT $1
                FOR UPDATE OF run SKIP LOCKED
            )
            UPDATE playbook_run AS run
            SET status = 'leased',
                lease_owner = $2,
                lease_expires_at = $4,
                leased_at = $3,
                error_message = NULL
            WHERE run.id IN (SELECT id FROM candidate)
            RETURNING {RUN_COLUMNS}
            "#
        );
        let rows = sqlx::query(&sql)
            .bind(record.limit)
            .bind(&record.lease_owner)
            .bind(record.now)
            .bind(record.lease_expires_at)
            .bind(record.max_concurrent_groups)
            .bind(record.max_concurrent_runs_per_group)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if !runs.is_empty() {
            let run_group_ids = runs.iter().map(|run| run.run_group_id).collect::<Vec<_>>();
            sqlx::query(
                r#"
                UPDATE playbook_run_group
                SET status = 'leased',
                    lease_owner = $3,
                    lease_expires_at = $4,
                    leased_at = $2,
                    started_at = COALESCE(started_at, $2),
                    updated_at = $2
                WHERE id = ANY($1)
                  AND status IN ('queued', 'leased')
                "#,
            )
            .bind(run_group_ids)
            .bind(record.now)
            .bind(&record.lease_owner)
            .bind(record.lease_expires_at)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(runs)
    }

    async fn start_leased_run(
        &self,
        record: StartLeasedRunRecord,
    ) -> Result<StartedRunRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'dispatching',
                started_at = COALESCE(started_at, $3),
                lease_expires_at = NULL,
                error_message = NULL,
                attempt = $4
            WHERE id = $1
              AND lease_owner = $2
              AND status = 'leased'
              AND lease_expires_at > $3
              AND cancel_requested_at IS NULL
            RETURNING {RUN_COLUMNS}
            "#
        );
        let Some(run_row) = sqlx::query(&run_sql)
            .bind(record.run_id)
            .bind(&record.lease_owner)
            .bind(record.now)
            .bind(record.dispatch_attempt)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::Conflict(
                "Playbook run is not leased by this worker, was canceled, or lease expired"
                    .to_string(),
            ));
        };
        let run = run_from_row(&run_row)?;

        let group_update = sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET status = 'dispatching',
                lease_owner = $3,
                lease_expires_at = $4,
                leased_at = COALESCE(leased_at, $2),
                started_at = COALESCE(started_at, $2),
                dispatch_id = $5,
                dispatch_attempt = $6,
                updated_at = $2
            WHERE id = $1
              AND status IN ('queued', 'leased')
              AND dispatch_attempt < $6
            "#,
        )
        .bind(run.run_group_id)
        .bind(record.now)
        .bind(&record.lease_owner)
        .bind(
            record
                .now
                .checked_add_signed(chrono::Duration::seconds(i64::from(
                    run.timeout_seconds.max(1),
                )))
                .unwrap_or(record.now),
        )
        .bind(record.dispatch_id)
        .bind(record.dispatch_attempt)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if group_update.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Playbook run group dispatch attempt changed concurrently".to_string(),
            ));
        }

        append_run_event_tx(
            &mut tx,
            CreateRunEventRecord {
                run_id: run.id,
                host: None,
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatching".to_string(),
                message: Some("Run dispatch prepared by worker lease owner".to_string()),
                payload: serde_json::json!({
                    "lease_owner": record.lease_owner,
                    "dispatch_id": record.dispatch_id,
                    "dispatch_attempt": record.dispatch_attempt,
                }),
            },
        )
        .await?;

        let version = version_by_id(&mut tx, run.playbook_version_id)
            .await?
            .ok_or_else(|| AppError::Database("Playbook version not found for run".to_string()))?;

        let token_sql = format!(
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
                $7
            FROM playbook_run AS run
            WHERE run.id = $2
            RETURNING {JOB_TOKEN_COLUMNS}
            "#
        );
        let Some(token_row) = sqlx::query(&token_sql)
            .bind(record.job_token.id)
            .bind(record.job_token.run_id)
            .bind(record.job_token.token_hash)
            .bind(record.job_token.token_prefix)
            .bind(record.job_token.scopes)
            .bind(record.job_token.expires_at)
            .bind(record.job_token.metadata)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound("Playbook run not found".to_string()));
        };
        let job_token = job_token_from_row(&token_row)?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(StartedRunRecord {
            run,
            version,
            job_token,
        })
    }

    async fn start_leased_runs(
        &self,
        record: StartLeasedRunsRecord,
    ) -> Result<Vec<StartedRunRecord>, AppError> {
        if record.run_ids.is_empty() {
            return Ok(Vec::new());
        }
        if record.run_ids.len() != record.job_tokens.len() {
            return Err(AppError::Validation(
                "start batch must provide one job token per run".to_string(),
            ));
        }

        let expected_count = i64::try_from(record.run_ids.len()).map_err(|_| {
            AppError::Validation("start batch is too large to validate".to_string())
        })?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;

        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'dispatching',
                started_at = COALESCE(started_at, $3),
                lease_expires_at = NULL,
                error_message = NULL,
                attempt = $4
            WHERE id = ANY($1)
              AND lease_owner = $2
              AND status = 'leased'
              AND lease_expires_at > $3
              AND cancel_requested_at IS NULL
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(&run_sql)
            .bind(&record.run_ids)
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
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;

        let first_run = runs
            .first()
            .ok_or_else(|| AppError::Database("Started run batch is empty".to_string()))?;
        let run_group_id = first_run.run_group_id;
        let version_id = first_run.playbook_version_id;
        if runs.iter().any(|run| run.run_group_id != run_group_id) {
            return Err(AppError::Validation(
                "start batch must target exactly one run group".to_string(),
            ));
        }
        if runs.iter().any(|run| run.playbook_version_id != version_id) {
            return Err(AppError::Validation(
                "start batch must use exactly one playbook version".to_string(),
            ));
        }

        let max_timeout_seconds = runs
            .iter()
            .map(|run| run.timeout_seconds.max(1))
            .max()
            .unwrap_or(1);
        let lease_expires_at = record
            .now
            .checked_add_signed(chrono::Duration::seconds(i64::from(max_timeout_seconds)))
            .unwrap_or(record.now);
        let group_update = sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET status = 'dispatching',
                lease_owner = $3,
                lease_expires_at = $4,
                leased_at = COALESCE(leased_at, $2),
                started_at = COALESCE(started_at, $2),
                dispatch_id = $5,
                dispatch_attempt = $6,
                updated_at = $2
            WHERE id = $1
              AND status IN ('queued', 'leased')
              AND dispatch_attempt < $6
            "#,
        )
        .bind(run_group_id)
        .bind(record.now)
        .bind(&record.lease_owner)
        .bind(lease_expires_at)
        .bind(record.dispatch_id)
        .bind(record.dispatch_attempt)
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
                run_group_id,
                run_id: Some(run.id),
                host: None,
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
        append_run_group_events_tx(&mut tx, events).await?;

        let version = version_by_id(&mut tx, version_id)
            .await?
            .ok_or_else(|| AppError::Database("Playbook version not found for run".to_string()))?;

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
        .bind(run_group_id)
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

        let mut token_insert = QueryBuilder::<Postgres>::new(
            r#"
            WITH token_input (
                id, run_id, token_hash, token_prefix, scopes, expires_at, metadata
            ) AS (
            "#,
        );
        token_insert.push_values(&record.job_tokens, |mut row, token| {
            row.push_bind(token.id)
                .push_bind(token.run_id)
                .push_bind(token.token_hash.clone())
                .push_bind(token.token_prefix.clone())
                .push_bind(token.scopes.clone())
                .push_bind(token.expires_at)
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
                token_input.metadata
            FROM token_input
            JOIN playbook_run AS run ON run.id = token_input.run_id
            RETURNING
            "#,
        );
        token_insert.push(JOB_TOKEN_COLUMNS);
        let token_rows = token_insert
            .build()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        if i64::try_from(token_rows.len()).unwrap_or(i64::MAX) != expected_count {
            return Err(AppError::Database(
                "Not every started run received a job token".to_string(),
            ));
        }
        let mut tokens_by_run_id = HashMap::with_capacity(token_rows.len());
        for row in token_rows {
            let token = job_token_from_row(&row)?;
            tokens_by_run_id.insert(token.run_id, token);
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
            started.push(StartedRunRecord {
                run,
                version: version.clone(),
                job_token,
            });
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(started)
    }

    async fn mark_run_running(
        &self,
        record: MarkRunRunningRecord,
    ) -> Result<PlaybookRun, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'running'
            WHERE id = $1
              AND lease_owner = $2
              AND status = 'dispatching'
            RETURNING {RUN_COLUMNS}
            "#
        );
        let Some(run_row) = sqlx::query(&run_sql)
            .bind(record.run_id)
            .bind(&record.lease_owner)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::Conflict(
                "Playbook run is not dispatching for this worker or was already completed"
                    .to_string(),
            ));
        };
        let run = run_from_row(&run_row)?;

        sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET status = 'running', updated_at = $2
            WHERE id = $1 AND status = 'dispatching'
            "#,
        )
        .bind(run.run_group_id)
        .bind(record.now)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        append_run_event_tx(
            &mut tx,
            CreateRunEventRecord {
                run_id: run.id,
                host: None,
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatched".to_string(),
                message: Some("Ansible job dispatched; waiting for runner events".to_string()),
                payload: serde_json::json!({ "lease_owner": record.lease_owner }),
            },
        )
        .await?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(run)
    }

    async fn mark_runs_running(
        &self,
        record: MarkRunsRunningRecord,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        if record.run_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'running'
            WHERE id = ANY($1)
              AND lease_owner = $2
              AND status = 'dispatching'
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(&run_sql)
            .bind(&record.run_ids)
            .bind(&record.lease_owner)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if runs.is_empty() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(Vec::new());
        }

        let run_group_id = runs[0].run_group_id;
        if runs.iter().any(|run| run.run_group_id != run_group_id) {
            return Err(AppError::Validation(
                "running batch must target exactly one run group".to_string(),
            ));
        }
        sqlx::query(
            r#"
            UPDATE playbook_run_group
            SET status = 'running', updated_at = $2
            WHERE id = $1 AND status = 'dispatching'
            "#,
        )
        .bind(run_group_id)
        .bind(record.now)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        let events = runs
            .iter()
            .map(|run| CreateRunGroupEventRecord {
                run_group_id,
                run_id: Some(run.id),
                host: None,
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatched".to_string(),
                message: Some("Ansible job dispatched; waiting for runner events".to_string()),
                payload: serde_json::json!({ "lease_owner": record.lease_owner }),
            })
            .collect::<Vec<_>>();
        append_run_group_events_tx(&mut tx, events).await?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(runs)
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
        let run_rows = sqlx::query(&run_sql)
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

    async fn requeue_started_runs(
        &self,
        record: RequeueStartedRunsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError> {
        if record.run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        // `started_at` is reset so the timeout reaper measures the next proven-
        // safe dispatch attempt, not this rejected one.
        let run_sql = format!(
            r#"
            UPDATE playbook_run
            SET status = 'leased',
                lease_expires_at = $3,
                leased_at = $4,
                started_at = NULL,
                error_message = $5
            WHERE id = ANY($1)
              AND lease_owner = $2
              AND status = 'dispatching'
              AND cancel_requested_at IS NULL
            RETURNING {RUN_COLUMNS}
            "#
        );
        let run_rows = sqlx::query(&run_sql)
            .bind(&record.run_ids)
            .bind(&record.lease_owner)
            .bind(record.retry_at)
            .bind(record.now)
            .bind(&record.reason)
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let runs = run_rows
            .iter()
            .map(run_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if runs.is_empty() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(runs);
        }

        let run_group_id = runs[0].run_group_id;
        sqlx::query(
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
        .bind(&record.lease_owner)
        .bind(record.retry_at)
        .bind(record.now)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        // Receipt lookup returned 404, so deleting this attempt's tokens cannot
        // strand a ghost execution. Ambiguous failures never enter this path.
        let requeued_ids = runs.iter().map(|run| run.id).collect::<Vec<_>>();
        sqlx::query("DELETE FROM playbook_job_token WHERE run_id = ANY($1)")
            .bind(&requeued_ids)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        sqlx::query("DELETE FROM playbook_run_group_callback_token WHERE run_group_id = $1")
            .bind(run_group_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;

        let events = runs
            .iter()
            .map(|run| CreateRunGroupEventRecord {
                run_group_id: run.run_group_id,
                run_id: Some(run.id),
                host: None,
                event_kind: PlaybookRunEventKind::Status,
                event_type: "run_dispatch_requeued".to_string(),
                message: Some(record.reason.clone()),
                payload: serde_json::json!({
                    "lease_owner": record.lease_owner,
                    "attempt": run.attempt,
                    "retry_at": record.retry_at,
                }),
            })
            .collect::<Vec<_>>();
        append_run_group_events_tx(&mut tx, events).await?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(runs)
    }

    async fn complete_run(
        &self,
        record: CompleteRunRecord,
    ) -> Result<CompletedRunRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let completed = complete_run_tx(&mut tx, &record).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(completed)
    }

    async fn complete_run_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunRecord,
    ) -> Result<CompletedRunRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let run_group_id =
            authorize_run_callback_tx(&mut tx, &auth, record.run_id, "status:update").await?;
        let usage = CallbackUsage {
            request_count: 1,
            ..CallbackUsage::default()
        };
        charge_run_callback_quota_tx(&mut tx, record.run_id, run_group_id, auth.now, usage).await?;
        charge_group_callback_quota_tx(&mut tx, run_group_id, auth.now, usage).await?;
        let completed = complete_run_tx(&mut tx, &record).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(completed)
    }

    async fn complete_run_group_batch(
        &self,
        record: CompleteRunGroupBatchRecord,
    ) -> Result<CompletedRunGroupBatchRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let completed = complete_run_group_batch_tx(&mut tx, &record).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(completed)
    }

    async fn complete_run_group_batch_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunGroupBatchRecord,
    ) -> Result<CompletedRunGroupBatchRecord, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        authorize_group_callback_tx(&mut tx, &auth, record.run_group_id).await?;
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
        let completed = complete_run_group_batch_tx(&mut tx, &record).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        // TOFU harvest: the runner reports keys it observed for the first time.
        // Persist only after the completion committed, so a rolled-back
        // completion can never leave a pin without its run. `upsert_lines` is
        // the single writer of the pin store; it keeps the first-seen pin and
        // never overwrites.
        if !record.new_host_keys.is_empty() {
            crate::contexts::shared::host_key_pins::upsert_lines(&self.pool, &record.new_host_keys)
                .await?;
        }
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
        let run_rows = sqlx::query(&run_sql)
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
        let run_groups = rollup_run_groups_tx(&mut tx, &group_ids, record.now).await?;
        revoke_terminal_callback_tokens_tx(&mut tx, &group_ids, record.now).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(TimedOutRunsRecord { runs, run_groups })
    }
}

/// Fetch a project by id through the canonical binding-aware read model.
async fn fetch_project_by_id(pool: &PgPool, id: Uuid) -> Result<Option<PlaybookProject>, AppError> {
    let sql = format!("SELECT {PROJECT_SELECT} WHERE p.id = $1");
    let row = sqlx::query(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    row.as_ref().map(project_from_row).transpose()
}

/// Verify and key-share-lock the named secret while a project transaction
/// creates or rebinds its canonical usage row. The lock closes the race with
/// named-secret deletion: deletion takes `FOR UPDATE` before counting usages.
async fn ensure_named_project_credential_tx(
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
async fn reconcile_project_credential_binding_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    project_id: Uuid,
    credential_secret_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(secret_id) = credential_secret_id {
        ensure_named_project_credential_tx(tx, secret_id).await?;
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
