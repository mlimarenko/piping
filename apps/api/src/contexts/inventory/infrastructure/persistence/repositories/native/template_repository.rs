//! Template repository — canonical SQL surface for inventory templates,
//! template runs, and step results.
//!
//! Every query that touches `inventory_template*` tables lives here. Handlers
//! and services must never inline SQL for these tables.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use uuid::Uuid;

use crate::contexts::shared::list_query::{ListColumn, SortSpec, list_sort_column};
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;

pub const INVENTORY_TEMPLATE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.templates.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.templates.columns.slug"),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.templates.columns.status",
    ),
    ListColumn::sortable(
        "shape",
        "(SELECT COUNT(*) FROM inventory_template_variable v WHERE v.template_id = inventory_template.id) + (SELECT COUNT(*) FROM inventory_template_step s WHERE s.template_id = inventory_template.id)",
        "inventory.templates.columns.shape",
    ),
    ListColumn::sortable(
        "createdAt",
        "created_at",
        "inventory.templates.columns.created_at",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "inventory.templates.columns.created_at",
    ),
    ListColumn::sortable(
        "updatedAt",
        "updated_at",
        "inventory.templates.columns.updated_at",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "inventory.templates.columns.updated_at",
    ),
];

pub const INVENTORY_TEMPLATE_RUN_COLUMNS: &[ListColumn] = &[
    ListColumn::enum_filter(
        "status",
        "r.status",
        "r.status",
        "inventory.templates.recent_runs.status",
    ),
    ListColumn::fk_filter(
        "template",
        "LOWER(t.name)",
        "template_id",
        "r.template_id",
        "inventory.templates",
        "inventory.templates.recent_runs.template",
    ),
    ListColumn::sortable(
        "startedAt",
        "COALESCE(r.started_at, r.created_at)",
        "inventory.templates.recent_runs.started_at",
    ),
    ListColumn::sortable(
        "duration",
        "COALESCE(EXTRACT(EPOCH FROM (r.finished_at - r.started_at)) * 1000, -1)",
        "inventory.templates.recent_runs.duration",
    ),
    ListColumn::sortable(
        "steps",
        "COALESCE((r.summary_json->>'total_steps')::int, 0)",
        "inventory.templates.recent_runs.steps",
    ),
];

// ---------------------------------------------------------------------------
// Row DTOs exposed to the service layer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TemplateIdRow {
    pub id: Uuid,
}

#[derive(Debug, Clone)]
pub struct TemplateHeaderRow {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub is_system: bool,
    pub owner_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Variable payload tagged with its owning `template_id`, used by the batch
/// loader so the service can group rows back to their templates in memory.
#[derive(Debug, Clone)]
pub struct TemplateVariableRow {
    pub template_id: Uuid,
    pub variable: Value,
}

/// Step payload tagged with its owning `template_id` for the batch loader.
#[derive(Debug, Clone)]
pub struct TemplateStepRow {
    pub template_id: Uuid,
    pub step: Value,
}

#[derive(Debug, Clone)]
pub struct TemplateRunRow {
    pub id: Uuid,
    pub template_id: Uuid,
    pub requested_by_user_id: Option<Uuid>,
    pub status: String,
    pub input_variables: Value,
    pub template_snapshot: Value,
    pub summary: Value,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TemplateRunStepRow {
    pub id: Uuid,
    pub run_id: Uuid,
    pub step_key: String,
    pub status: String,
    pub created_resource_ids: Value,
    pub resolved_payload: Value,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub created_at: DateTime<Utc>,
}

// Parsed-and-validated row shapes used by the service when it passes data to
// `create_template`/`update_template` via repo. Input-side validation is done
// in the service; the repo simply persists.
#[derive(Debug, Clone)]
pub struct TemplateInsertRow {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: &'static str,
    pub owner_user_id: Option<Uuid>,
    pub is_system: bool,
    pub updated_by_user_id: Option<Uuid>,
    pub variables: Vec<PreparedTemplateVariable>,
    pub steps: Vec<PreparedTemplateStep>,
}

#[derive(Debug, Clone)]
pub struct TemplateUpdateRow {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    /// `None` means "leave as it is", same rule as the fields below. An absent
    /// status used to resolve to `draft`, so a rename demoted an active
    /// template - the defect the rest of this struct was already fixed for.
    pub status: Option<&'static str>,
    /// `None` means "leave as it is". A partial update must not be able to
    /// clear the system flag, and the template editor never sends this field:
    /// treating absent as `false` silently un-marked every built-in template
    /// the moment anyone saved it.
    pub is_system: Option<bool>,
    pub updated_by_user_id: Option<Uuid>,
    /// `None` means "leave as they are", `Some(vec![])` means "make it empty".
    /// The two are different requests and used to be the same one: the
    /// replacements below start with a DELETE, so an update that simply did
    /// not mention variables or steps erased all of them.
    pub variables: Option<Vec<PreparedTemplateVariable>>,
    pub steps: Option<Vec<PreparedTemplateStep>>,
}

#[derive(Debug, Clone)]
pub struct PreparedTemplateVariable {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub value_type: String,
    pub required: bool,
    pub default_value: Option<Value>,
    pub validation_rules: Value,
    pub sort_order: i32,
}

#[derive(Debug, Clone)]
pub struct PreparedTemplateStep {
    pub step_key: String,
    pub title: String,
    pub description: Option<String>,
    pub resource_type: String,
    pub operation: String,
    pub depends_on_step_keys: Vec<String>,
    pub payload_template: Value,
    pub repeat: Value,
    pub on_conflict_policy: String,
    pub sort_order: i32,
}

#[derive(Debug, Clone)]
pub struct ResolvedStepRepeat {
    pub count: u32,
    pub start: i64,
    pub index_token: String,
}

// Snapshot of a template used by the service when creating a run.
#[derive(Debug, Clone)]
pub struct TemplateSnapshot {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub is_system: bool,
    pub owner_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub variables: Vec<Value>,
    pub steps: Vec<Value>,
}

// Per-step outcome produced by the run executor; repo uses it to insert rows
// into `inventory_template_run_step_result`.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub step_key: String,
    pub status: String,
    pub created_resource_ids: Value,
    pub resolved_payload: Value,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
}

// ---------------------------------------------------------------------------
// Step resource executor trait — injected by the service to keep resource-row
// INSERTs (tenant, site, device, VM, interfaces, IP) out of the repo while the
// repo still owns the outer transaction.
// ---------------------------------------------------------------------------

/// Boxed `Send` future returned by the object-safe repository trait methods.
type RepoFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait StepResourceExecutor: Send + Sync {
    fn execute<'a, 'tx>(
        &'a self,
        tx: &'a mut Transaction<'tx, Postgres>,
        step: &'a Value,
        resolved_payload: &'a Value,
    ) -> RepoFuture<'a, Result<Vec<Uuid>, AppError>>;
}

// Outcome producer trait supplied by the service: it inspects dependency
// state + conflict policy + execution error and returns the record the repo
// should persist. This keeps policy decisions in the service.
pub trait StepOutcomeProducer: Send + Sync {
    fn on_invalid_dependencies(&self, step_key: &str, err: &AppError) -> StepOutcome;
    fn on_dependency_not_met(&self, step_key: &str, blocking: &str) -> StepOutcome;
    fn on_success(
        &self,
        step_key: &str,
        resolved_payload: &Value,
        created_ids: &[Uuid],
    ) -> StepOutcome;
    fn on_failure(
        &self,
        step_key: &str,
        resolved_payload: &Value,
        err: &AppError,
        is_conflict_skip: bool,
    ) -> StepOutcome;
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// One template run request: the frozen snapshot, the resolved variables and
/// steps, and the initial token map handed to the step executor.
pub struct TemplateRunInput {
    pub template_id: Uuid,
    pub template_snapshot: Value,
    pub requested_by_user_id: Uuid,
    pub input_variables: Value,
    pub steps: Vec<Value>,
    pub initial_tokens: HashMap<String, Value>,
}

/// Inputs for a page of template runs: status/template filters, search and sort.
#[derive(Debug, Clone, Default)]
pub struct TemplateRunListFilter {
    pub status_filter: Vec<String>,
    pub template_filter: Vec<Uuid>,
    pub search_filter: Option<String>,
    pub sort: Option<SortSpec>,
}

pub trait TemplateRepository: Send + Sync {
    // Listing / lookup
    fn list_templates_page<'a>(
        &'a self,
        status_filter: Vec<String>,
        search_filter: Option<String>,
        sort: Option<SortSpec>,
        limit: u32,
        offset: u32,
    ) -> RepoFuture<'a, Result<PageResult<TemplateIdRow>, AppError>>;
    fn list_template_run_ids_for_template<'a>(
        &'a self,
        template_id: Uuid,
        search_filter: Option<String>,
        limit: u32,
        offset: u32,
    ) -> RepoFuture<'a, Result<Vec<Uuid>, AppError>>;

    fn list_template_runs_page<'a>(
        &'a self,
        filter: TemplateRunListFilter,
        limit: u32,
        offset: u32,
    ) -> RepoFuture<'a, Result<PageResult<TemplateRunRow>, AppError>>;

    fn get_template_header<'a>(
        &'a self,
        template_id: Uuid,
    ) -> RepoFuture<'a, Result<Option<TemplateHeaderRow>, AppError>>;

    fn template_exists<'a>(&'a self, template_id: Uuid) -> RepoFuture<'a, Result<bool, AppError>>;

    fn load_template_variables<'a>(
        &'a self,
        template_id: Uuid,
    ) -> RepoFuture<'a, Result<Vec<Value>, AppError>>;

    fn load_template_steps<'a>(
        &'a self,
        template_id: Uuid,
    ) -> RepoFuture<'a, Result<Vec<Value>, AppError>>;

    /// Batch loader for template headers — avoids N+1 in `list_templates`.
    /// Returns headers for every id in `ids`; ids without a matching row are
    /// simply absent. An empty `ids` slice short-circuits with no query.
    fn load_template_headers<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> RepoFuture<'a, Result<Vec<TemplateHeaderRow>, AppError>>;

    /// Batch loader for template variables across many templates. Each row
    /// carries its `template_id` for in-memory grouping. Ordering matches the
    /// single-template loader (`sort_order ASC, created_at ASC`) within each
    /// template. Empty `ids` short-circuits with no query.
    fn load_template_variables_for<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> RepoFuture<'a, Result<Vec<TemplateVariableRow>, AppError>>;

    /// Batch loader for template steps across many templates. Each row carries
    /// its `template_id` for in-memory grouping. Ordering matches the
    /// single-template loader (`sort_order ASC, created_at ASC`) within each
    /// template. Empty `ids` short-circuits with no query.
    fn load_template_steps_for<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> RepoFuture<'a, Result<Vec<TemplateStepRow>, AppError>>;

    fn get_template_run<'a>(
        &'a self,
        run_id: Uuid,
    ) -> RepoFuture<'a, Result<Option<TemplateRunRow>, AppError>>;

    fn list_run_step_results<'a>(
        &'a self,
        run_id: Uuid,
    ) -> RepoFuture<'a, Result<Vec<TemplateRunStepRow>, AppError>>;

    // Mutations
    fn validate_selections<'a>(
        &'a self,
        selections: &'a [crate::contexts::inventory::domain::native::template_reference::TemplateSelection],
    ) -> RepoFuture<'a, Result<(), AppError>>;

    fn create_template<'a>(
        &'a self,
        row: TemplateInsertRow,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

    fn update_template<'a>(
        &'a self,
        row: TemplateUpdateRow,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

    fn delete_template<'a>(
        &'a self,
        template_id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u64, AppError>> + Send + 'a>>;

    /// Runs one template: opens a transaction, inserts the run row, executes
    /// each step (recording outcomes), finalizes the run. Returns run summary
    /// plus number of success/failure/skipped steps.
    ///
    /// Step execution itself is split — the transactional bookkeeping lives
    /// here, while the actual resource INSERTs are delegated to the injected
    /// [`StepResourceExecutor`]. This keeps the SQL split across canonical
    /// layers without forcing cross-table SQL into this file.
    fn execute_template_run<'a>(
        &'a self,
        input: TemplateRunInput,
        resource_executor: &'a (dyn StepResourceExecutor + 'a),
        outcome_producer: &'a (dyn StepOutcomeProducer + 'a),
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Uuid, AppError>> + Send + 'a>>;
}

// ---------------------------------------------------------------------------
// Sqlx implementation
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqlxTemplateRepository {
    pool: PgPool,
}

impl SqlxTemplateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn insert_run_step_result(
        tx: &mut Transaction<'_, Postgres>,
        run_id: Uuid,
        outcome: &StepOutcome,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO inventory_template_run_step_result (
                id,
                run_id,
                step_key,
                status,
                created_resource_ids_json,
                resolved_payload_json,
                error_code,
                error_message,
                duration_ms
            )
            VALUES ($1, $2, $3, $4::inventory_template_step_result_status, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(run_id)
        .bind(&outcome.step_key)
        .bind(&outcome.status)
        .bind(&outcome.created_resource_ids)
        .bind(&outcome.resolved_payload)
        .bind(outcome.error_code.as_deref())
        .bind(outcome.error_message.as_deref())
        .bind(outcome.duration_ms)
        .execute(&mut **tx)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to insert run step result: {error}"))
        })?;
        Ok(())
    }

    async fn replace_variables(
        tx: &mut Transaction<'_, Postgres>,
        template_id: Uuid,
        variables: &[PreparedTemplateVariable],
    ) -> Result<(), AppError> {
        sqlx::query("DELETE FROM inventory_template_variable WHERE template_id = $1")
            .bind(template_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to cleanup template variables: {error}"))
            })?;

        for variable in variables {
            sqlx::query(
                r#"
                INSERT INTO inventory_template_variable (
                    id, template_id, key, label, description, value_type, required,
                    default_value_json, validation_rules_json, sort_order
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(template_id)
            .bind(&variable.key)
            .bind(&variable.label)
            .bind(variable.description.as_deref())
            .bind(&variable.value_type)
            .bind(variable.required)
            .bind(&variable.default_value)
            .bind(&variable.validation_rules)
            .bind(variable.sort_order)
            .execute(&mut **tx)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to insert template variable: {error}"))
            })?;
        }
        Ok(())
    }

    async fn replace_steps(
        tx: &mut Transaction<'_, Postgres>,
        template_id: Uuid,
        steps: &[PreparedTemplateStep],
    ) -> Result<(), AppError> {
        sqlx::query("DELETE FROM inventory_template_step WHERE template_id = $1")
            .bind(template_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to cleanup template steps: {error}"))
            })?;

        for step in steps {
            let deps = Value::Array(
                step.depends_on_step_keys
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            );
            sqlx::query(
                r#"
                INSERT INTO inventory_template_step (
                    id, template_id, step_key, title, description, resource_type,
                    operation, depends_on_step_keys, payload_template_json,
                    repeat_json, on_conflict_policy, sort_order
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(template_id)
            .bind(&step.step_key)
            .bind(&step.title)
            .bind(step.description.as_deref())
            .bind(&step.resource_type)
            .bind(&step.operation)
            .bind(deps)
            .bind(&step.payload_template)
            .bind(&step.repeat)
            .bind(&step.on_conflict_policy)
            .bind(step.sort_order)
            .execute(&mut **tx)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to insert template step: {error}"))
            })?;
        }
        Ok(())
    }
}

impl TemplateRepository for SqlxTemplateRepository {
    fn validate_selections<'a>(
        &'a self,
        selections: &'a [crate::contexts::inventory::domain::native::template_reference::TemplateSelection],
    ) -> RepoFuture<'a, Result<(), AppError>> {
        Box::pin(
            super::template_step_resource_repository::validate_template_selections(
                &self.pool, selections,
            ),
        )
    }
    fn list_templates_page<'a>(
        &'a self,
        status_filter: Vec<String>,
        search_filter: Option<String>,
        sort: Option<SortSpec>,
        limit: u32,
        offset: u32,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<PageResult<TemplateIdRow>, AppError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let order_by = match sort.as_ref() {
                Some(spec) => {
                    let column = list_sort_column(INVENTORY_TEMPLATE_COLUMNS, &spec.key)
                        .unwrap_or("updated_at");
                    format!("{column}{}", spec.sql_direction())
                }
                None => "updated_at DESC".to_string(),
            };
            let rows = sqlx::query(&format!(
                r#"
                SELECT id
                FROM inventory_template
                WHERE (cardinality($1::text[]) = 0 OR status::text = ANY($1::text[]))
                  AND (
                    $2::text IS NULL
                    OR name ILIKE '%' || $2 || '%'
                    OR COALESCE(slug, '') ILIKE '%' || $2 || '%'
                  )
                ORDER BY {order_by}, id
                LIMIT $3 OFFSET $4
                "#,
            ))
            .bind(&status_filter)
            .bind(&search_filter)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to list templates: {error}")))?;

            let items: Vec<TemplateIdRow> = rows
                .into_iter()
                .map(|row| {
                    row.try_get::<Uuid, _>("id")
                        .map(|id| TemplateIdRow { id })
                        .map_err(|error| {
                            AppError::Database(format!("Failed to parse template id: {error}"))
                        })
                })
                .collect::<Result<_, _>>()?;

            let total: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)::bigint
                FROM inventory_template
                WHERE (cardinality($1::text[]) = 0 OR status::text = ANY($1::text[]))
                  AND (
                    $2::text IS NULL
                    OR name ILIKE '%' || $2 || '%'
                    OR COALESCE(slug, '') ILIKE '%' || $2 || '%'
                  )
                "#,
            )
            .bind(&status_filter)
            .bind(&search_filter)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to count templates: {error}")))?;

            let total = u64::try_from(total)
                .map_err(|_| AppError::Internal("Template total count overflow".to_string()))?;

            Ok(PageResult {
                items,
                total,
                limit,
                offset,
            })
        })
    }

    fn list_template_run_ids_for_template<'a>(
        &'a self,
        template_id: Uuid,
        search_filter: Option<String>,
        limit: u32,
        offset: u32,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<Uuid>, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT id
                FROM inventory_template_run
                WHERE template_id = $1
                  AND (
                    $2::text IS NULL
                    OR id::text ILIKE '%' || $2 || '%'
                    OR status::text ILIKE '%' || $2 || '%'
                  )
                ORDER BY created_at DESC
                LIMIT $3 OFFSET $4
                "#,
            )
            .bind(template_id)
            .bind(&search_filter)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to list template runs: {error}"))
            })?;

            rows.into_iter()
                .map(|row| {
                    row.try_get::<Uuid, _>("id").map_err(|error| {
                        AppError::Database(format!("Failed to parse run id: {error}"))
                    })
                })
                .collect()
        })
    }

    fn list_template_runs_page<'a>(
        &'a self,
        filter: TemplateRunListFilter,
        limit: u32,
        offset: u32,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<PageResult<TemplateRunRow>, AppError>> + Send + 'a>,
    > {
        let TemplateRunListFilter {
            status_filter,
            template_filter,
            search_filter,
            sort,
        } = filter;
        Box::pin(async move {
            let order_by = match sort.as_ref() {
                Some(spec) => {
                    let column = list_sort_column(INVENTORY_TEMPLATE_RUN_COLUMNS, &spec.key)
                        .unwrap_or("r.created_at");
                    format!("{column}{}", spec.sql_direction())
                }
                None => "r.created_at DESC".to_string(),
            };
            let rows = sqlx::query(&format!(
                r#"
                SELECT r.id, r.template_id, r.requested_by_user_id, r.status::text AS status,
                       r.input_variables_json, r.template_snapshot_json, r.summary_json,
                       r.started_at, r.finished_at, r.created_at
                FROM inventory_template_run r
                JOIN inventory_template t ON t.id = r.template_id
                WHERE (cardinality($1::text[]) = 0 OR r.status::text = ANY($1::text[]))
                  AND (cardinality($2::uuid[]) = 0 OR r.template_id = ANY($2::uuid[]))
                  AND (
                    $3::text IS NULL
                    OR r.id::text ILIKE '%' || $3 || '%'
                    OR r.status::text ILIKE '%' || $3 || '%'
                    OR COALESCE(t.name, '') ILIKE '%' || $3 || '%'
                    OR COALESCE(t.slug, '') ILIKE '%' || $3 || '%'
                  )
                ORDER BY {order_by}, r.id
                LIMIT $4 OFFSET $5
                "#,
            ))
            .bind(&status_filter)
            .bind(&template_filter)
            .bind(&search_filter)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to list template runs: {error}"))
            })?;

            let items = rows
                .iter()
                .map(map_template_run_row)
                .collect::<Result<Vec<_>, _>>()?;

            let total: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)::bigint
                FROM inventory_template_run r
                JOIN inventory_template t ON t.id = r.template_id
                WHERE (cardinality($1::text[]) = 0 OR r.status::text = ANY($1::text[]))
                  AND (cardinality($2::uuid[]) = 0 OR r.template_id = ANY($2::uuid[]))
                  AND (
                    $3::text IS NULL
                    OR r.id::text ILIKE '%' || $3 || '%'
                    OR r.status::text ILIKE '%' || $3 || '%'
                    OR COALESCE(t.name, '') ILIKE '%' || $3 || '%'
                    OR COALESCE(t.slug, '') ILIKE '%' || $3 || '%'
                  )
                "#,
            )
            .bind(&status_filter)
            .bind(&template_filter)
            .bind(&search_filter)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to count template runs: {error}"))
            })?;

            let total = u64::try_from(total)
                .map_err(|_| AppError::Internal("Template run total count overflow".to_string()))?;

            Ok(PageResult {
                items,
                total,
                limit,
                offset,
            })
        })
    }

    fn get_template_header<'a>(
        &'a self,
        template_id: Uuid,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Option<TemplateHeaderRow>, AppError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT id, name, slug, description, status::text AS status, is_system,
                       owner_user_id, updated_by_user_id, created_at, updated_at
                FROM inventory_template
                WHERE id = $1
                "#,
            )
            .bind(template_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to load template: {error}")))?;

            let Some(row) = row else {
                return Ok(None);
            };
            Ok(Some(map_template_header_row(&row)?))
        })
    }

    fn template_exists<'a>(
        &'a self,
        template_id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<bool, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let row = sqlx::query("SELECT id FROM inventory_template WHERE id = $1")
                .bind(template_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| AppError::Database(format!("Failed to load template: {error}")))?;
            Ok(row.is_some())
        })
    }

    fn load_template_variables<'a>(
        &'a self,
        template_id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<Value>, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT key, label, description, value_type, required,
                       default_value_json, validation_rules_json, sort_order
                FROM inventory_template_variable
                WHERE template_id = $1
                ORDER BY sort_order ASC, created_at ASC
                "#,
            )
            .bind(template_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load template variables: {error}"))
            })?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                out.push(map_template_variable_row(&row)?);
            }
            Ok(out)
        })
    }

    fn load_template_steps<'a>(
        &'a self,
        template_id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<Value>, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT step_key, title, description, resource_type, operation,
                       depends_on_step_keys, payload_template_json,
                       repeat_json, on_conflict_policy, sort_order
                FROM inventory_template_step
                WHERE template_id = $1
                ORDER BY sort_order ASC, created_at ASC
                "#,
            )
            .bind(template_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load template steps: {error}"))
            })?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                out.push(map_template_step_row(&row)?);
            }
            Ok(out)
        })
    }

    fn load_template_headers<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<TemplateHeaderRow>, AppError>> + Send + 'a>>
    {
        Box::pin(async move {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let rows = sqlx::query(
                r#"
                SELECT id, name, slug, description, status::text AS status, is_system,
                       owner_user_id, updated_by_user_id, created_at, updated_at
                FROM inventory_template
                WHERE id = ANY($1)
                "#,
            )
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to load templates: {error}")))?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                out.push(map_template_header_row(&row)?);
            }
            Ok(out)
        })
    }

    fn load_template_variables_for<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Vec<TemplateVariableRow>, AppError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let rows = sqlx::query(
                r#"
                SELECT template_id, key, label, description, value_type, required,
                       default_value_json, validation_rules_json, sort_order
                FROM inventory_template_variable
                WHERE template_id = ANY($1)
                ORDER BY template_id, sort_order ASC, created_at ASC
                "#,
            )
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load template variables: {error}"))
            })?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let template_id: Uuid = row.try_get("template_id").map_err(|e| {
                    AppError::Database(format!("Failed to parse variable template_id: {e}"))
                })?;
                out.push(TemplateVariableRow {
                    template_id,
                    variable: map_template_variable_row(&row)?,
                });
            }
            Ok(out)
        })
    }

    fn load_template_steps_for<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<TemplateStepRow>, AppError>> + Send + 'a>>
    {
        Box::pin(async move {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let rows = sqlx::query(
                r#"
                SELECT template_id, step_key, title, description, resource_type, operation,
                       depends_on_step_keys, payload_template_json,
                       repeat_json, on_conflict_policy, sort_order
                FROM inventory_template_step
                WHERE template_id = ANY($1)
                ORDER BY template_id, sort_order ASC, created_at ASC
                "#,
            )
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load template steps: {error}"))
            })?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let template_id: Uuid = row.try_get("template_id").map_err(|e| {
                    AppError::Database(format!("Failed to parse step template_id: {e}"))
                })?;
                out.push(TemplateStepRow {
                    template_id,
                    step: map_template_step_row(&row)?,
                });
            }
            Ok(out)
        })
    }

    fn get_template_run<'a>(
        &'a self,
        run_id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Option<TemplateRunRow>, AppError>> + Send + 'a>>
    {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT id, template_id, requested_by_user_id, status::text AS status,
                       input_variables_json, template_snapshot_json, summary_json,
                       started_at, finished_at, created_at
                FROM inventory_template_run
                WHERE id = $1
                "#,
            )
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to load template run: {error}")))?;

            let Some(row) = row else {
                return Ok(None);
            };
            Ok(Some(map_template_run_row(&row)?))
        })
    }

    fn list_run_step_results<'a>(
        &'a self,
        run_id: Uuid,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Vec<TemplateRunStepRow>, AppError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT id, run_id, step_key, status::text AS status,
                       created_resource_ids_json, resolved_payload_json,
                       error_code, error_message, duration_ms, created_at
                FROM inventory_template_run_step_result
                WHERE run_id = $1
                ORDER BY created_at ASC
                "#,
            )
            .bind(run_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to list run steps: {error}")))?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let created_at: DateTime<Utc> = row.try_get("created_at").map_err(|e| {
                    AppError::Database(format!("Failed to parse run step created_at: {e}"))
                })?;
                out.push(TemplateRunStepRow {
                    id: row.try_get("id").unwrap_or_else(|_| Uuid::nil()),
                    run_id: row.try_get("run_id").unwrap_or_else(|_| Uuid::nil()),
                    step_key: row
                        .try_get("step_key")
                        .unwrap_or_else(|_| "step".to_string()),
                    status: row
                        .try_get("status")
                        .unwrap_or_else(|_| "failed".to_string()),
                    created_resource_ids: row
                        .try_get("created_resource_ids_json")
                        .unwrap_or_else(|_| Value::Array(Vec::new())),
                    resolved_payload: row
                        .try_get("resolved_payload_json")
                        .unwrap_or_else(|_| Value::Object(Map::new())),
                    error_code: row.try_get("error_code").ok(),
                    error_message: row.try_get("error_message").ok(),
                    duration_ms: row.try_get("duration_ms").ok(),
                    created_at,
                });
            }
            Ok(out)
        })
    }

    fn create_template<'a>(
        &'a self,
        row: TemplateInsertRow,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(|error| {
                AppError::Database(format!("Failed to start template transaction: {error}"))
            })?;

            sqlx::query(
                r#"
                INSERT INTO inventory_template (
                    id, name, slug, description, status, owner_user_id, is_system, updated_by_user_id
                )
                VALUES ($1, $2, $3, $4, $5::inventory_template_status, $6, $7, $8)
                "#,
            )
            .bind(row.id)
            .bind(&row.name)
            .bind(row.slug.as_deref())
            .bind(row.description.as_deref())
            .bind(row.status)
            .bind(row.owner_user_id)
            .bind(row.is_system)
            .bind(row.updated_by_user_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to create template: {error}"))
            })?;

            Self::replace_variables(&mut tx, row.id, &row.variables).await?;
            Self::replace_steps(&mut tx, row.id, &row.steps).await?;

            tx.commit().await.map_err(|error| {
                AppError::Database(format!("Failed to commit template creation: {error}"))
            })?;
            Ok(())
        })
    }

    fn update_template<'a>(
        &'a self,
        row: TemplateUpdateRow,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(|error| {
                AppError::Database(format!("Failed to start template transaction: {error}"))
            })?;

            sqlx::query(
                r#"
                UPDATE inventory_template
                SET name = $2,
                    slug = $3,
                    description = $4,
                    status = COALESCE($5::inventory_template_status, status),
                    is_system = COALESCE($6, is_system),
                    updated_by_user_id = $7,
                    updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(row.id)
            .bind(&row.name)
            .bind(row.slug.as_deref())
            .bind(row.description.as_deref())
            .bind(row.status)
            .bind(row.is_system)
            .bind(row.updated_by_user_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Database(format!("Failed to update template: {error}")))?;

            // Only when the caller actually sent them. `replace_*` deletes first, so
            // calling it with an empty vector on behalf of a caller who said nothing
            // is what destroyed the contents.
            if let Some(variables) = &row.variables {
                Self::replace_variables(&mut tx, row.id, variables).await?;
            }
            if let Some(steps) = &row.steps {
                Self::replace_steps(&mut tx, row.id, steps).await?;
            }

            tx.commit().await.map_err(|error| {
                AppError::Database(format!("Failed to commit template update: {error}"))
            })?;
            Ok(())
        })
    }

    fn delete_template<'a>(
        &'a self,
        template_id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u64, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let result = sqlx::query("DELETE FROM inventory_template WHERE id = $1")
                .bind(template_id)
                .execute(&self.pool)
                .await
                .map_err(|error| {
                    AppError::Database(format!("Failed to delete template: {error}"))
                })?;
            Ok(result.rows_affected())
        })
    }

    fn execute_template_run<'a>(
        &'a self,
        input: TemplateRunInput,
        resource_executor: &'a (dyn StepResourceExecutor + 'a),
        outcome_producer: &'a (dyn StepOutcomeProducer + 'a),
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Uuid, AppError>> + Send + 'a>> {
        let TemplateRunInput {
            template_id,
            template_snapshot,
            requested_by_user_id,
            input_variables,
            steps,
            initial_tokens,
        } = input;
        Box::pin(async move {
            let run_id = Uuid::now_v7();
            let mut tx = self.pool.begin().await.map_err(|error| {
                AppError::Database(format!("Failed to start run transaction: {error}"))
            })?;

            sqlx::query(
                r#"
                INSERT INTO inventory_template_run (
                    id,
                    template_id,
                    requested_by_user_id,
                    status,
                    input_variables_json,
                    template_snapshot_json,
                    summary_json,
                    started_at
                )
                VALUES ($1, $2, $3, 'running'::inventory_template_run_status, $4, $5, '{}'::jsonb, NOW())
                "#,
            )
            .bind(run_id)
            .bind(template_id)
            .bind(requested_by_user_id)
            .bind(&input_variables)
            .bind(&template_snapshot)
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Database(format!("Failed to create run: {error}")))?;

            let mut token_map = initial_tokens;
            let mut success_count: u32 = 0;
            let mut failure_count: u32 = 0;
            let mut skipped_count: u32 = 0;
            let mut total_result_count: u32 = 0;
            let mut step_statuses: HashMap<String, String> = HashMap::new();

            for step in &steps {
                let step_key = extract_step_key(step);

                // Parse dependencies
                let dependencies = match parse_step_dependencies(step) {
                    Ok(v) => v,
                    Err(err) => {
                        let outcome = outcome_producer.on_invalid_dependencies(&step_key, &err);
                        let mut outcome = outcome;
                        outcome.duration_ms = Some(0);
                        step_statuses.insert(step_key.clone(), outcome.status.clone());
                        total_result_count += 1;
                        match outcome.status.as_str() {
                            "success" => success_count += 1,
                            "skipped" => skipped_count += 1,
                            _ => failure_count += 1,
                        }
                        Self::insert_run_step_result(&mut tx, run_id, &outcome).await?;
                        continue;
                    }
                };

                // Check blocking dependency
                let blocking = dependencies.iter().find_map(|d| {
                    if matches!(step_statuses.get(d).map(String::as_str), Some("success")) {
                        None
                    } else {
                        Some(d.as_str())
                    }
                });
                if let Some(blocking) = blocking {
                    let mut outcome = outcome_producer.on_dependency_not_met(&step_key, blocking);
                    outcome.duration_ms = Some(0);
                    step_statuses.insert(step_key.clone(), outcome.status.clone());
                    total_result_count += 1;
                    match outcome.status.as_str() {
                        "success" => success_count += 1,
                        "skipped" => skipped_count += 1,
                        _ => failure_count += 1,
                    }
                    Self::insert_run_step_result(&mut tx, run_id, &outcome).await?;
                    continue;
                }

                let repeat = match resolve_step_repeat(step, &token_map) {
                    Ok(repeat) => repeat,
                    Err(err) => {
                        let mut outcome = outcome_producer.on_invalid_dependencies(&step_key, &err);
                        outcome.duration_ms = Some(0);
                        step_statuses.insert(step_key.clone(), outcome.status.clone());
                        total_result_count += 1;
                        failure_count += 1;
                        Self::insert_run_step_result(&mut tx, run_id, &outcome).await?;
                        continue;
                    }
                };
                if repeat.count == 0 {
                    step_statuses.insert(step_key.clone(), "success".to_string());
                    token_map.insert(format!("steps.{step_key}.ids"), Value::Array(Vec::new()));
                    continue;
                }

                let mut aggregate_ids: Vec<Uuid> = Vec::new();
                let mut repeated_statuses: Vec<String> = Vec::with_capacity(repeat.count as usize);

                for iteration in 0..repeat.count {
                    let started = Utc::now();
                    let position = repeat.start + i64::from(iteration);
                    let effective_step_key = if repeat.count == 1 {
                        step_key.clone()
                    } else {
                        format!("{step_key}[{position}]")
                    };
                    let mut iteration_tokens = token_map.clone();
                    iteration_tokens
                        .insert(repeat.index_token.clone(), Value::Number(position.into()));
                    iteration_tokens.insert("position".to_string(), Value::Number(position.into()));
                    insert_current_iteration_step_tokens(&mut iteration_tokens, position);

                    let resolved_payload = resolve_step_payload(step, &iteration_tokens)?;
                    match resource_executor
                        .execute(&mut tx, step, &resolved_payload)
                        .await
                    {
                        Ok(created_ids) => {
                            if let Some(first) = created_ids.first() {
                                token_map.insert(
                                    format!("steps.{step_key}.{position}.id"),
                                    Value::String(first.to_string()),
                                );
                                if aggregate_ids.is_empty() {
                                    token_map.insert(
                                        format!("steps.{step_key}.id"),
                                        Value::String(first.to_string()),
                                    );
                                }
                            }
                            token_map.insert(
                                format!("steps.{step_key}.{position}.ids"),
                                Value::Array(
                                    created_ids
                                        .iter()
                                        .map(|v| Value::String(v.to_string()))
                                        .collect(),
                                ),
                            );
                            aggregate_ids.extend(created_ids.iter().copied());
                            let mut outcome = outcome_producer.on_success(
                                &effective_step_key,
                                &resolved_payload,
                                &created_ids,
                            );
                            outcome.duration_ms = Some(duration_ms(started));
                            repeated_statuses.push(outcome.status.clone());
                            success_count += 1;
                            total_result_count += 1;
                            Self::insert_run_step_result(&mut tx, run_id, &outcome).await?;
                        }
                        Err(err) => {
                            let is_conflict_skip = is_conflict_skip_step(step, &err);
                            let mut outcome = outcome_producer.on_failure(
                                &effective_step_key,
                                &resolved_payload,
                                &err,
                                is_conflict_skip,
                            );
                            outcome.duration_ms = Some(duration_ms(started));
                            repeated_statuses.push(outcome.status.clone());
                            total_result_count += 1;
                            match outcome.status.as_str() {
                                "skipped" => skipped_count += 1,
                                _ => failure_count += 1,
                            }
                            Self::insert_run_step_result(&mut tx, run_id, &outcome).await?;
                        }
                    }
                }

                token_map.insert(
                    format!("steps.{step_key}.ids"),
                    Value::Array(
                        aggregate_ids
                            .iter()
                            .map(|v| Value::String(v.to_string()))
                            .collect(),
                    ),
                );
                let aggregate_status = if repeated_statuses.iter().any(|s| s == "failed") {
                    "failed"
                } else if repeated_statuses.iter().any(|s| s == "skipped") {
                    "skipped"
                } else {
                    "success"
                };
                step_statuses.insert(step_key.clone(), aggregate_status.to_string());
            }

            let status = run_status_for(failure_count, skipped_count);
            let summary = summarize_run(
                total_result_count as usize,
                success_count,
                skipped_count,
                failure_count,
            );

            sqlx::query(
                r#"
                UPDATE inventory_template_run
                SET status = $2::inventory_template_run_status,
                    summary_json = $3,
                    finished_at = NOW(),
                    updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(run_id)
            .bind(status)
            .bind(summary)
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Database(format!("Failed to finalize run: {error}")))?;

            tx.commit().await.map_err(|error| {
                AppError::Database(format!("Failed to commit run execution: {error}"))
            })?;
            Ok(run_id)
        })
    }
}

fn duration_ms(started_at: DateTime<Utc>) -> i32 {
    (Utc::now() - started_at).num_milliseconds() as i32
}

// ---------------------------------------------------------------------------
// Row mappers — shared by the single-template loaders and their batch
// (`*_for` / `_headers`) counterparts so both produce identical JSON shapes.
// ---------------------------------------------------------------------------

fn map_template_header_row(row: &PgRow) -> Result<TemplateHeaderRow, AppError> {
    Ok(TemplateHeaderRow {
        id: row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to parse template id: {e}")))?,
        name: row
            .try_get("name")
            .map_err(|e| AppError::Database(format!("Failed to parse template name: {e}")))?,
        slug: row.try_get("slug").ok(),
        description: row.try_get("description").ok(),
        status: row
            .try_get("status")
            .map_err(|e| AppError::Database(format!("Failed to parse template status: {e}")))?,
        is_system: row.try_get("is_system").unwrap_or(false),
        owner_user_id: row.try_get("owner_user_id").ok(),
        updated_by_user_id: row.try_get("updated_by_user_id").ok(),
        created_at: row
            .try_get("created_at")
            .map_err(|e| AppError::Database(format!("Failed to parse template created_at: {e}")))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|e| AppError::Database(format!("Failed to parse template updated_at: {e}")))?,
    })
}

fn map_template_run_row(row: &PgRow) -> Result<TemplateRunRow, AppError> {
    Ok(TemplateRunRow {
        id: row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to parse run id: {e}")))?,
        template_id: row
            .try_get("template_id")
            .map_err(|e| AppError::Database(format!("Failed to parse run template_id: {e}")))?,
        requested_by_user_id: row.try_get("requested_by_user_id").ok(),
        status: row
            .try_get("status")
            .map_err(|e| AppError::Database(format!("Failed to parse run status: {e}")))?,
        input_variables: row
            .try_get("input_variables_json")
            .unwrap_or_else(|_| Value::Object(Map::new())),
        template_snapshot: row
            .try_get("template_snapshot_json")
            .unwrap_or_else(|_| Value::Object(Map::new())),
        summary: row
            .try_get("summary_json")
            .unwrap_or_else(|_| Value::Object(Map::new())),
        started_at: row.try_get("started_at").ok(),
        finished_at: row.try_get("finished_at").ok(),
        created_at: row
            .try_get("created_at")
            .map_err(|e| AppError::Database(format!("Failed to parse run created_at: {e}")))?,
    })
}

fn map_template_variable_row(row: &PgRow) -> Result<Value, AppError> {
    let key: String = row
        .try_get("key")
        .map_err(|e| AppError::Database(format!("Failed to parse variable key: {e}")))?;
    let label: String = row
        .try_get("label")
        .map_err(|e| AppError::Database(format!("Failed to parse variable label: {e}")))?;
    let description: Option<String> = row.try_get("description").ok();
    let value_type: String = row
        .try_get("value_type")
        .map_err(|e| AppError::Database(format!("Failed to parse variable value_type: {e}")))?;
    let required: bool = row.try_get("required").unwrap_or(false);
    let default_value: Option<Value> = row.try_get("default_value_json").ok();
    let validation_rules: Value = row
        .try_get("validation_rules_json")
        .unwrap_or_else(|_| Value::Object(Map::new()));
    let sort_order: i32 = row.try_get("sort_order").unwrap_or(0);
    Ok(json!({
        "key": key,
        "label": label,
        "description": description,
        "value_type": value_type,
        "required": required,
        "default_value": default_value,
        "validation_rules": validation_rules,
        "sort_order": sort_order,
    }))
}

fn map_template_step_row(row: &PgRow) -> Result<Value, AppError> {
    let step_key: String = row
        .try_get("step_key")
        .map_err(|e| AppError::Database(format!("Failed to parse step key: {e}")))?;
    let title: String = row
        .try_get("title")
        .map_err(|e| AppError::Database(format!("Failed to parse step title: {e}")))?;
    let description: Option<String> = row.try_get("description").ok();
    let resource_type: String = row
        .try_get("resource_type")
        .map_err(|e| AppError::Database(format!("Failed to parse step resource_type: {e}")))?;
    let operation: String = row
        .try_get("operation")
        .map_err(|e| AppError::Database(format!("Failed to parse step operation: {e}")))?;
    let depends_on_step_keys: Value = row
        .try_get("depends_on_step_keys")
        .unwrap_or_else(|_| Value::Array(Vec::new()));
    let payload_template: Value = row
        .try_get("payload_template_json")
        .unwrap_or_else(|_| Value::Object(Map::new()));
    let repeat: Value = row
        .try_get("repeat_json")
        .unwrap_or_else(|_| Value::Object(Map::new()));
    let on_conflict_policy: String = row
        .try_get("on_conflict_policy")
        .unwrap_or_else(|_| "fail".to_string());
    let sort_order: i32 = row.try_get("sort_order").unwrap_or(0);
    Ok(json!({
        "step_key": step_key,
        "title": title,
        "description": description,
        "resource_type": resource_type,
        "operation": operation,
        "depends_on_step_keys": depends_on_step_keys,
        "payload_template": payload_template,
        "repeat": repeat,
        "on_conflict_policy": on_conflict_policy,
        "sort_order": sort_order,
    }))
}

// ---------------------------------------------------------------------------
// Stateless step helpers (pure JSON manipulation). Kept here because
// `execute_template_run` is the only runtime caller; the service re-exports
// them for preview flows and validation.
// ---------------------------------------------------------------------------

pub fn extract_step_key(step: &Value) -> String {
    step.get("step_key")
        .and_then(Value::as_str)
        .unwrap_or("step")
        .to_string()
}

pub fn parse_step_dependencies(step: &Value) -> Result<Vec<String>, AppError> {
    let Some(raw) = step.get("depends_on_step_keys") else {
        return Ok(Vec::new());
    };
    let Some(items) = raw.as_array() else {
        return Err(AppError::Validation(
            "depends_on_step_keys must be an array".to_string(),
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let dep = item
            .as_str()
            .ok_or_else(|| {
                AppError::Validation(
                    "depends_on_step_keys must contain only string values".to_string(),
                )
            })?
            .trim()
            .to_string();
        if dep.is_empty() {
            return Err(AppError::Validation(
                "depends_on_step_keys cannot contain empty values".to_string(),
            ));
        }
        out.push(dep);
    }
    Ok(out)
}

pub fn resolve_step_payload(
    step: &Value,
    tokens: &HashMap<String, Value>,
) -> Result<Value, AppError> {
    let payload = step
        .get("payload_template")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    crate::contexts::inventory::domain::native::template_reference::project_payload(
        step,
        resolve_template_value(&payload, tokens),
    )
}

pub fn resolve_step_preview(
    step: &Value,
    tokens: &HashMap<String, Value>,
) -> Result<Value, AppError> {
    let mut resolved = step.clone();
    if step.get("payload_template").is_some() {
        let payload = resolve_step_payload(step, tokens)?;
        if let Some(object) = resolved.as_object_mut() {
            object.insert("payload".to_string(), payload.clone());
            object.insert("resolved_payload".to_string(), payload);
        }
    }
    Ok(resolved)
}

pub fn resolve_step_repeat(
    step: &Value,
    tokens: &HashMap<String, Value>,
) -> Result<ResolvedStepRepeat, AppError> {
    let Some(raw) = step.get("repeat") else {
        return Ok(default_step_repeat());
    };
    if raw.is_null() {
        return Ok(default_step_repeat());
    }
    let Some(object) = raw.as_object() else {
        return Err(AppError::Validation(
            "repeat must be an object when provided".to_string(),
        ));
    };
    if object.is_empty() {
        return Ok(default_step_repeat());
    }

    let count = object
        .get("count")
        .map(|value| resolve_template_value(value, tokens))
        .map(|value| parse_repeat_i64(&value, "repeat.count"))
        .transpose()?
        .unwrap_or(1);
    if !(0..=500).contains(&count) {
        return Err(AppError::Validation(
            "repeat.count must be between 0 and 500".to_string(),
        ));
    }

    let start = object
        .get("start")
        .map(|value| resolve_template_value(value, tokens))
        .map(|value| parse_repeat_i64(&value, "repeat.start"))
        .transpose()?
        .unwrap_or(1);

    let index_token = object
        .get("index_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("position")
        .to_string();

    Ok(ResolvedStepRepeat {
        count: count as u32,
        start,
        index_token,
    })
}

pub fn resolve_step_previews(
    step: &Value,
    tokens: &mut HashMap<String, Value>,
) -> Result<Vec<Value>, AppError> {
    let step_key = extract_step_key(step);
    let repeat = resolve_step_repeat(step, tokens)?;
    if repeat.count == 0 {
        tokens.insert(format!("steps.{step_key}.ids"), Value::Array(Vec::new()));
        return Ok(Vec::new());
    }

    let mut previews = Vec::with_capacity(repeat.count as usize);
    let mut ids = Vec::with_capacity(repeat.count as usize);
    for iteration in 0..repeat.count {
        let position = repeat.start + i64::from(iteration);
        let effective_step_key = if repeat.count == 1 {
            step_key.clone()
        } else {
            format!("{step_key}[{position}]")
        };
        let mut iteration_tokens = tokens.clone();
        iteration_tokens.insert(repeat.index_token.clone(), Value::Number(position.into()));
        iteration_tokens.insert("position".to_string(), Value::Number(position.into()));
        insert_current_iteration_step_tokens(&mut iteration_tokens, position);

        let mut preview = resolve_step_preview(step, &iteration_tokens)?;
        if let Some(object) = preview.as_object_mut() {
            object.insert(
                "step_key".to_string(),
                Value::String(effective_step_key.clone()),
            );
        }
        let fake_id = Value::String(format!("preview:{step_key}:{position}"));
        tokens.insert(format!("steps.{step_key}.{position}.id"), fake_id.clone());
        tokens.insert(
            format!("steps.{step_key}.{position}.ids"),
            Value::Array(vec![fake_id.clone()]),
        );
        if ids.is_empty() {
            tokens.insert(format!("steps.{step_key}.id"), fake_id.clone());
        }
        ids.push(fake_id);
        previews.push(preview);
    }
    tokens.insert(format!("steps.{step_key}.ids"), Value::Array(ids));
    Ok(previews)
}

pub fn resolve_template_value(value: &Value, tokens: &HashMap<String, Value>) -> Value {
    match value {
        Value::Object(map) => {
            let mut resolved = Map::new();
            for (k, v) in map {
                resolved.insert(k.clone(), resolve_template_value(v, tokens));
            }
            Value::Object(resolved)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|i| resolve_template_value(i, tokens))
                .collect(),
        ),
        Value::String(text) => resolve_template_string(text, tokens),
        _ => value.clone(),
    }
}

fn resolve_template_string(text: &str, tokens: &HashMap<String, Value>) -> Value {
    if !text.contains("{{") {
        return Value::String(text.to_string());
    }
    let trimmed = text.trim();
    if trimmed.starts_with("{{")
        && trimmed.ends_with("}}")
        && !trimmed[2..trimmed.len() - 2].contains("{{")
    {
        let token = trimmed[2..trimmed.len() - 2].trim();
        if let Some(value) = resolve_template_helper(token, tokens) {
            return value;
        }
        if let Some(value) = tokens.get(token) {
            return value.clone();
        }
        return Value::String(text.to_string());
    }
    let mut output = text.to_string();
    for (token, value) in tokens {
        let placeholder = format!("{{{{{token}}}}}");
        let replacement = value
            .as_str()
            .map(|v| v.to_string())
            .unwrap_or_else(|| value.to_string());
        output = output.replace(&placeholder, &replacement);
    }
    Value::String(output)
}

fn default_step_repeat() -> ResolvedStepRepeat {
    ResolvedStepRepeat {
        count: 1,
        start: 1,
        index_token: "position".to_string(),
    }
}

fn parse_repeat_i64(value: &Value, field: &str) -> Result<i64, AppError> {
    match value {
        Value::Number(number) => number.as_i64().ok_or_else(|| {
            AppError::Validation(format!("{field} must be an integer-compatible number"))
        }),
        Value::String(text) => text.trim().parse::<i64>().map_err(|_| {
            AppError::Validation(format!("{field} must be an integer-compatible value"))
        }),
        _ => Err(AppError::Validation(format!(
            "{field} must be an integer-compatible value"
        ))),
    }
}

fn insert_current_iteration_step_tokens(tokens: &mut HashMap<String, Value>, position: i64) {
    let id_suffix = format!(".{position}.id");
    let ids_suffix = format!(".{position}.ids");
    let aliases: Vec<(String, Value)> = tokens
        .iter()
        .filter_map(|(key, value)| {
            if !key.starts_with("steps.") {
                return None;
            }
            if let Some(prefix) = key.strip_suffix(&id_suffix) {
                return Some((format!("{prefix}.current.id"), value.clone()));
            }
            key.strip_suffix(&ids_suffix)
                .map(|prefix| (format!("{prefix}.current.ids"), value.clone()))
        })
        .collect();
    for (key, value) in aliases {
        tokens.insert(key, value);
    }
}

fn resolve_template_helper(token: &str, tokens: &HashMap<String, Value>) -> Option<Value> {
    let parts: Vec<&str> = token.split_whitespace().collect();
    match parts.as_slice() {
        ["cidr_host", cidr_token, offset_token] => {
            resolve_cidr_host(cidr_token, offset_token, None, tokens).map(Value::String)
        }
        ["cidr_host", cidr_token, offset_token, index_token] => {
            resolve_cidr_host(cidr_token, offset_token, Some(*index_token), tokens)
                .map(Value::String)
        }
        _ => None,
    }
}

fn resolve_cidr_host(
    cidr_token: &str,
    offset_token: &str,
    index_token: Option<&str>,
    tokens: &HashMap<String, Value>,
) -> Option<String> {
    let cidr = lookup_token_text(cidr_token, tokens)?;
    let offset = lookup_token_i64(offset_token, tokens)?;
    let index = index_token
        .map(|token| lookup_token_i64(token, tokens))
        .unwrap_or(Some(1))?;
    let host_offset = offset.checked_add(index.checked_sub(1)?)?;
    if host_offset < 0 {
        return None;
    }
    cidr_host(&cidr, host_offset as u32)
}

fn lookup_token_text(token: &str, tokens: &HashMap<String, Value>) -> Option<String> {
    if let Some(value) = tokens.get(token) {
        return value.as_str().map(ToString::to_string);
    }
    Some(token.to_string()).filter(|value| value.contains('/'))
}

fn lookup_token_i64(token: &str, tokens: &HashMap<String, Value>) -> Option<i64> {
    if let Some(value) = tokens.get(token) {
        return match value {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.trim().parse::<i64>().ok(),
            _ => None,
        };
    }
    token.parse::<i64>().ok()
}

fn cidr_host(cidr: &str, host_offset: u32) -> Option<String> {
    let (address, prefix_raw) = cidr.trim().split_once('/')?;
    let base = address.parse::<Ipv4Addr>().ok()?;
    let prefix = prefix_raw.parse::<u32>().ok()?;
    if prefix > 32 {
        return None;
    }
    let base_int = u32::from(base);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX.checked_shl(32 - prefix).unwrap_or(0)
    };
    let network = base_int & mask;
    let host = network.checked_add(host_offset)?;
    if host & mask != network {
        return None;
    }
    Some(format!("{}/{}", Ipv4Addr::from(host), prefix))
}

pub fn run_status_for(failure_count: u32, skipped_count: u32) -> &'static str {
    if failure_count == 0 && skipped_count == 0 {
        "completed"
    } else {
        "completed_with_errors"
    }
}

pub fn summarize_run(total_steps: usize, success: u32, skipped: u32, failed: u32) -> Value {
    json!({
        "total_steps": total_steps,
        "succeeded_steps": success,
        "skipped_steps": skipped,
        "failed_steps": failed,
    })
}

fn is_conflict_skip_step(step: &Value, error: &AppError) -> bool {
    let policy = step
        .get("on_conflict_policy")
        .and_then(Value::as_str)
        .unwrap_or("fail");
    if policy != "skip" {
        return false;
    }
    match error {
        AppError::Database(message) => message.contains("duplicate key value violates unique"),
        AppError::DatabaseSource(sqlx_error) => sqlx_error
            .as_database_error()
            .and_then(|db| db.code())
            .map(|c| c == "23505")
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod template_reference_projection_tests {
    use super::*;

    #[test]
    fn template_reference_selection_projects_factory_name_to_uuid() {
        let id = Uuid::new_v4();
        let step = serde_json::json!({"resource_type":"device", "payload_template":{"device_type":"{{chosen}}"}});
        let tokens = HashMap::from([(
            "chosen".to_string(),
            serde_json::json!({"resource":"device_type","id":id}),
        )]);
        let payload = resolve_step_payload(&step, &tokens).expect("selected reference");
        assert_eq!(payload.get("device_type_id"), Some(&serde_json::json!(id)));
        assert!(payload.get("device_type").is_none());
    }
}
