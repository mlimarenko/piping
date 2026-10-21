use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::errors::AppError;

const FULL_BACKUP_VERSION_V1: &str = "piping-full-backup-v1";
const EXCLUDED_TABLES: &[&str] = &["_sqlx_migrations"];

fn is_excluded_backup_table(name: &str) -> bool {
    EXCLUDED_TABLES.contains(&name) || name.starts_with('_')
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupArchive {
    pub version: String,
    pub schema_version: Option<i64>,
    pub created_at: String,
    pub include_secrets: bool,
    pub table_count: usize,
    pub row_count: usize,
    pub skipped: Vec<FullBackupSkippedItem>,
    pub tables: Vec<FullBackupTable>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupTable {
    pub name: String,
    pub section: FullBackupSection,
    pub columns: Vec<FullBackupColumn>,
    pub rows: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupColumn {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupSkippedItem {
    pub scope: String,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupExportRequest {
    #[serde(default)]
    pub include_secrets: bool,
    #[serde(default)]
    pub confirm_secret_export: bool,
    #[serde(default)]
    pub selection: Option<FullBackupSelection>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupImportRequest {
    pub payload: FullBackupArchive,
    #[serde(default)]
    pub confirm_full_restore: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub selection: Option<FullBackupSelection>,
    #[serde(default)]
    pub rules: Option<FullBackupImportRules>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupImportSummary {
    pub applied: bool,
    pub table_count: usize,
    pub row_count: usize,
    pub mode: FullBackupImportMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<FullBackupImportPlan>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FullBackupSection {
    Inventory,
    Connections,
    Settings,
    Access,
    Secrets,
    Automation,
    Synchronization,
    Observability,
    Assistant,
    Licensing,
    Appearance,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupSelection {
    #[serde(default)]
    pub sections: Vec<FullBackupSection>,
    #[serde(default)]
    pub tables: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupImportRules {
    #[serde(default = "default_import_action")]
    pub default_action: FullBackupImportAction,
    #[serde(default)]
    pub sections: HashMap<FullBackupSection, FullBackupImportAction>,
    #[serde(default)]
    pub tables: HashMap<String, FullBackupImportAction>,
    #[serde(default)]
    pub table_policies: HashMap<String, FullBackupTablePolicy>,
}

impl Default for FullBackupImportRules {
    fn default() -> Self {
        Self {
            default_action: default_import_action(),
            sections: HashMap::new(),
            tables: HashMap::new(),
            table_policies: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupTablePolicy {
    #[serde(default)]
    pub action: Option<FullBackupImportAction>,
    #[serde(default)]
    pub update_columns: Vec<String>,
    #[serde(default)]
    pub preserve_columns: Vec<String>,
    #[serde(default)]
    pub null_policy: FullBackupNullPolicy,
    #[serde(default)]
    pub json_merge_columns: HashMap<String, FullBackupJsonMergePolicy>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FullBackupNullPolicy {
    #[default]
    Apply,
    PreserveExisting,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupJsonMergePolicy {
    pub mode: FullBackupJsonMergeMode,
    #[serde(default)]
    pub preserve_keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FullBackupJsonMergeMode {
    ShallowObject,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupImportPlan {
    pub dry_run: bool,
    pub table_count: usize,
    pub row_count: usize,
    pub has_destructive_changes: bool,
    pub warnings: Vec<FullBackupPlanWarning>,
    pub tables: Vec<FullBackupTablePlan>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupTablePlan {
    pub name: String,
    pub section: FullBackupSection,
    pub action: FullBackupImportAction,
    pub rows: usize,
    pub inserts: i64,
    pub updates: i64,
    pub deletes: i64,
    pub unchanged: i64,
    pub policy: FullBackupAppliedTablePolicy,
    pub warnings: Vec<FullBackupPlanWarning>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupAppliedTablePolicy {
    pub update_columns: Vec<String>,
    pub preserve_columns: Vec<String>,
    pub null_policy: FullBackupNullPolicy,
    pub json_merge_columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FullBackupPlanWarning {
    pub scope: String,
    pub name: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FullBackupImportAction {
    /// Upsert rows from the archive and keep rows that are absent from it.
    #[default]
    Replace,
    /// Upsert rows from the archive and delete current rows absent from it.
    Delete,
    /// Do not import this selected section/table.
    Skip,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FullBackupImportMode {
    Full,
    Selective,
}

#[derive(Clone, Debug)]
struct TableMeta {
    name: String,
    columns: Vec<ColumnMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ColumnMeta {
    name: String,
    data_type: String,
}

#[derive(Clone, Debug)]
struct ForeignKeyMeta {
    child_table: String,
    child_column: String,
    parent_table: String,
}

#[derive(Clone, Debug)]
struct DeferredColumnUpdate {
    table_name: String,
    primary_key_columns: Vec<String>,
    columns: Vec<String>,
    rows: Value,
    policy: ResolvedTablePolicy,
    column_types: HashMap<String, String>,
}

#[derive(Clone, Debug)]
struct SelectedBackupTable {
    table: FullBackupTable,
    action: FullBackupImportAction,
    primary_key_columns: Vec<String>,
    deferrable_columns: HashSet<String>,
    policy: ResolvedTablePolicy,
    column_types: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
struct ResolvedTablePolicy {
    explicit_update_columns: Option<HashSet<String>>,
    preserve_columns: HashSet<String>,
    null_policy: FullBackupNullPolicy,
    json_merge_columns: HashMap<String, FullBackupJsonMergePolicy>,
}

#[derive(Clone)]
pub struct BackupService {
    pool: PgPool,
}

impl BackupService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn export_full_backup(
        &self,
        include_secrets: bool,
        selection: Option<FullBackupSelection>,
    ) -> Result<FullBackupArchive, AppError> {
        let schema_version = load_schema_version(&self.pool).await?;
        let tables = select_tables(ordered_backup_tables(&self.pool).await?, selection.as_ref())?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("Failed to begin backup export: {e}")))?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to configure backup export snapshot: {e}"))
            })?;
        let mut exported_tables = Vec::new();
        let mut skipped = Vec::new();
        let mut row_count = 0usize;

        for table in tables {
            if !include_secrets && is_sensitive_table(&table.name) {
                skipped.push(FullBackupSkippedItem {
                    scope: "table".to_string(),
                    name: table.name,
                    reason: "secrets_excluded".to_string(),
                });
                continue;
            }

            let mut columns = table.columns;
            if !include_secrets {
                let mut kept = Vec::with_capacity(columns.len());
                for column in columns {
                    if is_sensitive_column(&column.name) {
                        skipped.push(FullBackupSkippedItem {
                            scope: "column".to_string(),
                            name: format!("{}.{}", table.name, column.name),
                            reason: "secrets_excluded".to_string(),
                        });
                    } else {
                        kept.push(column);
                    }
                }
                columns = kept;
            }

            let mut rows = export_table_rows(&mut tx, &table.name).await?;
            if !include_secrets {
                let kept_column_names = columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<HashSet<_>>();
                for row in &mut rows {
                    if let Value::Object(object) = row {
                        object.retain(|key, _| kept_column_names.contains(key.as_str()));
                    }
                }
            }
            row_count += rows.len();

            exported_tables.push(FullBackupTable {
                section: table_section(&table.name),
                name: table.name,
                columns: columns
                    .into_iter()
                    .map(|column| FullBackupColumn {
                        name: column.name,
                        data_type: column.data_type,
                    })
                    .collect(),
                rows,
            });
        }

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to finish backup export: {e}")))?;

        Ok(FullBackupArchive {
            version: FULL_BACKUP_VERSION_V1.to_string(),
            schema_version,
            created_at: Utc::now().to_rfc3339(),
            include_secrets,
            table_count: exported_tables.len(),
            row_count,
            skipped,
            tables: exported_tables,
        })
    }

    pub async fn import_full_backup(
        &self,
        request: FullBackupImportRequest,
    ) -> Result<FullBackupImportSummary, AppError> {
        if !request.confirm_full_restore && !request.dry_run {
            return Err(AppError::Validation(
                "Full restore must be explicitly confirmed".to_string(),
            ));
        }
        let dry_run = request.dry_run;
        let archive = request.payload;
        let import_mode = if request.selection.is_some() || request.rules.is_some() {
            FullBackupImportMode::Selective
        } else {
            FullBackupImportMode::Full
        };
        if archive.version != FULL_BACKUP_VERSION_V1 {
            return Err(AppError::Validation(format!(
                "Unsupported backup version '{}'",
                archive.version
            )));
        }
        if import_mode == FullBackupImportMode::Full && !archive.include_secrets {
            return Err(AppError::Validation(
                "This archive was exported without secrets and cannot restore an instance 1:1"
                    .to_string(),
            ));
        }
        if import_mode == FullBackupImportMode::Selective {
            return self
                .import_selected_backup(archive, request.selection, request.rules, dry_run)
                .await;
        }
        let existing_tables = validate_full_backup_archive(&self.pool, &archive).await?;
        let existing_columns = load_columns(&self.pool).await?;
        let nullable_columns = load_nullable_columns(&self.pool).await?;
        let foreign_keys = load_foreign_keys(&self.pool).await?;
        archive_columns_match_instance(&archive, &existing_columns)?;

        if dry_run {
            let plan = build_full_restore_plan(&self.pool, &archive).await?;
            return Ok(FullBackupImportSummary {
                applied: false,
                table_count: archive.table_count,
                row_count: archive.row_count,
                mode: FullBackupImportMode::Full,
                plan: Some(plan),
            });
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("Failed to begin backup restore: {e}")))?;

        truncate_instance_tables(&mut tx, &existing_tables).await?;

        let (row_count, deferred_updates) = restore_full_backup_tables(
            &mut tx,
            archive.tables,
            &existing_tables,
            &nullable_columns,
            &foreign_keys,
        )
        .await?;

        apply_deferred_column_updates(&mut tx, deferred_updates).await?;

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit backup restore: {e}")))?;

        Ok(FullBackupImportSummary {
            applied: true,
            table_count: archive.table_count,
            row_count,
            mode: FullBackupImportMode::Full,
            plan: None,
        })
    }

    async fn import_selected_backup(
        &self,
        archive: FullBackupArchive,
        selection: Option<FullBackupSelection>,
        rules: Option<FullBackupImportRules>,
        dry_run: bool,
    ) -> Result<FullBackupImportSummary, AppError> {
        let existing_tables = load_table_names(&self.pool)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let existing_columns = load_columns(&self.pool).await?;
        let nullable_columns = load_nullable_columns(&self.pool).await?;
        let foreign_keys = load_foreign_keys(&self.pool).await?;
        let primary_keys = load_primary_keys(&self.pool).await?;
        let selected_names = selected_archive_table_names(&archive.tables, selection.as_ref())?;
        let rules = rules.unwrap_or_default();

        let selected_tables = prepare_selected_backup_tables(
            archive.tables,
            &selected_names,
            &rules,
            &BackupSchemaMetadata {
                existing_tables: &existing_tables,
                existing_columns: &existing_columns,
                nullable_columns: &nullable_columns,
                foreign_keys: &foreign_keys,
                primary_keys: &primary_keys,
            },
        )?;

        apply_selected_backup(&self.pool, &selected_tables, dry_run).await
    }
}

/// Restore a prepared selective backup, or plan it for a dry run.
async fn apply_selected_backup(
    pool: &PgPool,
    selected_tables: &[SelectedBackupTable],
    dry_run: bool,
) -> Result<FullBackupImportSummary, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Database(format!("Failed to begin selected backup import: {e}")))?;

    let plan = plan_selected_backup_tables(&mut tx, selected_tables, dry_run).await?;
    if dry_run {
        tx.rollback().await.map_err(|e| {
            AppError::Database(format!("Failed to finish selected backup dry-run: {e}"))
        })?;
        return Ok(FullBackupImportSummary {
            applied: false,
            table_count: plan.table_count,
            row_count: plan.row_count,
            mode: FullBackupImportMode::Selective,
            plan: Some(plan),
        });
    }

    delete_selected_backup_rows(&mut tx, selected_tables).await?;

    let mut deferred_updates = Vec::<DeferredColumnUpdate>::new();
    for selected in selected_tables {
        if selected.table.rows.is_empty() || selected.table.columns.is_empty() {
            continue;
        }
        upsert_table_rows(&mut tx, selected).await?;
        if let Some(update) = deferred_column_update(selected) {
            deferred_updates.push(update);
        }
    }

    for update in deferred_updates {
        apply_deferred_column_update(&mut tx, update).await?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("Failed to commit selected backup import: {e}")))?;

    Ok(FullBackupImportSummary {
        applied: true,
        table_count: plan.table_count,
        row_count: plan.row_count,
        mode: FullBackupImportMode::Selective,
        plan: Some(plan),
    })
}

/// Delete rows for every selected table whose action is `Delete`, children
/// first so foreign keys stay satisfiable.
async fn delete_selected_backup_rows(
    tx: &mut Transaction<'_, Postgres>,
    selected_tables: &[SelectedBackupTable],
) -> Result<(), AppError> {
    for selected in selected_tables.iter().rev() {
        if selected.action == FullBackupImportAction::Delete {
            delete_missing_rows(tx, &selected.table, &selected.primary_key_columns).await?;
        }
    }
    Ok(())
}

/// Build the deferred column update for a restored table, or `None` when the
/// table has no columns to defer.
fn deferred_column_update(selected: &SelectedBackupTable) -> Option<DeferredColumnUpdate> {
    if selected.deferrable_columns.is_empty() {
        return None;
    }
    let mut deferred_columns = selected
        .deferrable_columns
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    deferred_columns.sort();
    Some(DeferredColumnUpdate {
        table_name: selected.table.name.clone(),
        primary_key_columns: selected.primary_key_columns.clone(),
        columns: deferred_columns,
        rows: Value::Array(selected.table.rows.clone()),
        policy: selected.policy.clone(),
        column_types: selected.column_types.clone(),
    })
}

/// Truncate every table present in the instance so the archive can be restored
/// 1:1. Identity sequences restart and dependent rows cascade.
async fn truncate_instance_tables(
    tx: &mut Transaction<'_, Postgres>,
    existing_tables: &HashSet<String>,
) -> Result<(), AppError> {
    let truncate_targets = existing_tables
        .iter()
        .map(|table| quote_table(table))
        .collect::<Vec<_>>()
        .join(", ");
    if !truncate_targets.is_empty() {
        sqlx::query(&format!(
            "TRUNCATE TABLE {truncate_targets} RESTART IDENTITY CASCADE"
        ))
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to truncate instance tables: {e}")))?;
    }
    Ok(())
}

/// Insert every archived table in dependency order and return the row count
/// together with the foreign-key columns whose values must be applied after the
/// full pass.
async fn restore_full_backup_tables(
    tx: &mut Transaction<'_, Postgres>,
    tables: Vec<FullBackupTable>,
    existing_tables: &HashSet<String>,
    nullable_columns: &HashSet<(String, String)>,
    foreign_keys: &[ForeignKeyMeta],
) -> Result<(usize, Vec<DeferredColumnUpdate>), AppError> {
    let mut row_count = 0usize;
    let mut restored_tables = HashSet::<String>::new();
    let mut deferred_updates = Vec::<DeferredColumnUpdate>::new();
    for table in tables {
        if table.rows.is_empty() || table.columns.is_empty() {
            restored_tables.insert(table.name);
            continue;
        }

        let deferrable_columns = foreign_keys
            .iter()
            .filter(|foreign_key| {
                foreign_key.child_table == table.name
                    && existing_tables.contains(&foreign_key.parent_table)
                    && !restored_tables.contains(&foreign_key.parent_table)
                    && nullable_columns.contains(&(
                        foreign_key.child_table.clone(),
                        foreign_key.child_column.clone(),
                    ))
                    && table
                        .columns
                        .iter()
                        .any(|column| column.name == foreign_key.child_column)
                    && table.columns.iter().any(|column| column.name == "id")
            })
            .map(|foreign_key| foreign_key.child_column.clone())
            .collect::<HashSet<_>>();

        let columns = table
            .columns
            .iter()
            .filter(|column| !deferrable_columns.contains(&column.name))
            .map(|column| quote_ident(&column.name))
            .collect::<Vec<_>>();
        let column_list = columns.join(", ");
        let table_row_count = table.rows.len();
        let rows = Value::Array(table.rows.clone());
        let sql = format!(
            "INSERT INTO {} ({}) OVERRIDING SYSTEM VALUE SELECT {} FROM jsonb_populate_recordset(NULL::{}, $1::jsonb)",
            quote_table(&table.name),
            column_list,
            column_list,
            quote_table(&table.name),
        );
        sqlx::query(&sql)
            .bind(&rows)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to restore table '{}': {e}", table.name))
            })?;
        row_count += table_row_count;
        restored_tables.insert(table.name.clone());
        if !deferrable_columns.is_empty() {
            let mut deferred_columns = deferrable_columns.into_iter().collect::<Vec<_>>();
            deferred_columns.sort();
            deferred_updates.push(DeferredColumnUpdate {
                table_name: table.name.clone(),
                primary_key_columns: vec!["id".to_string()],
                columns: deferred_columns,
                rows,
                policy: ResolvedTablePolicy::default(),
                column_types: table
                    .columns
                    .iter()
                    .map(|column| (column.name.clone(), column.data_type.clone()))
                    .collect(),
            });
        }
    }
    Ok((row_count, deferred_updates))
}

/// Apply the deferred foreign-key column updates after every table has been
/// restored.
async fn apply_deferred_column_updates(
    tx: &mut Transaction<'_, Postgres>,
    updates: Vec<DeferredColumnUpdate>,
) -> Result<(), AppError> {
    for update in updates {
        let assignments = update
            .columns
            .iter()
            .map(|column| {
                let quoted = quote_ident(column);
                format!("{quoted} = source.{quoted}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let predicates = update
            .columns
            .iter()
            .map(|column| format!("source.{} IS NOT NULL", quote_ident(column)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let key_predicates = update
            .primary_key_columns
            .iter()
            .map(|column| {
                let quoted = quote_ident(column);
                format!("target.{quoted} = source.{quoted}")
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!(
            "UPDATE {} AS target SET {} FROM jsonb_populate_recordset(NULL::{}, $1::jsonb) AS source WHERE {} AND ({})",
            quote_table(&update.table_name),
            assignments,
            quote_table(&update.table_name),
            key_predicates,
            predicates,
        );
        sqlx::query(&sql)
            .bind(update.rows)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to restore deferred references for table '{}': {e}",
                    update.table_name
                ))
            })?;
    }
    Ok(())
}

/// Reject an archive that cannot be restored 1:1 into this instance: the
/// schema version must match and the archive must carry exactly the instance's
/// table set. Returns the instance table names so the caller can truncate them.
async fn validate_full_backup_archive(
    pool: &PgPool,
    archive: &FullBackupArchive,
) -> Result<HashSet<String>, AppError> {
    let current_schema_version = load_schema_version(pool).await?;
    if archive.schema_version != current_schema_version {
        return Err(AppError::Validation(format!(
            "Backup schema version {:?} does not match this instance schema version {:?}",
            archive.schema_version, current_schema_version
        )));
    }

    let existing_tables = load_table_names(pool)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();
    let archive_tables = archive
        .tables
        .iter()
        .map(|table| table.name.clone())
        .collect::<HashSet<_>>();
    for table in &archive.tables {
        if !existing_tables.contains(&table.name) {
            return Err(AppError::Validation(format!(
                "Backup table '{}' does not exist in this instance schema",
                table.name
            )));
        }
    }
    if archive_tables != existing_tables {
        let missing_in_archive = summarize_name_diff(&existing_tables, &archive_tables);
        let unknown_in_archive = summarize_name_diff(&archive_tables, &existing_tables);
        return Err(AppError::Validation(format!(
            "Backup schema does not match this instance. Missing in archive: {missing_in_archive}. Unknown in archive: {unknown_in_archive}"
        )));
    }
    Ok(existing_tables)
}

/// Reject an archive whose per-table column set does not match the instance.
fn archive_columns_match_instance(
    archive: &FullBackupArchive,
    existing_columns: &HashMap<String, Vec<ColumnMeta>>,
) -> Result<(), AppError> {
    for table in &archive.tables {
        let current_columns = existing_columns
            .get(&table.name)
            .cloned()
            .unwrap_or_default();
        let archive_columns = table
            .columns
            .iter()
            .map(|column| ColumnMeta {
                name: column.name.clone(),
                data_type: column.data_type.clone(),
            })
            .collect::<Vec<_>>();
        if archive_columns != current_columns {
            return Err(AppError::Validation(format!(
                "Backup columns for table '{}' do not match this instance schema",
                table.name
            )));
        }
    }
    Ok(())
}

fn default_import_action() -> FullBackupImportAction {
    FullBackupImportAction::Replace
}

pub fn full_backup_import_requires_secret_permission(
    request: &FullBackupImportRequest,
) -> Result<bool, AppError> {
    let rules = request.rules.clone().unwrap_or_default();
    let selected_names =
        selected_archive_table_names(&request.payload.tables, request.selection.as_ref())?;
    Ok(request
        .payload
        .tables
        .iter()
        .filter(|table| selected_names.contains(&table.name))
        .filter(|table| resolve_import_action(&rules, table) != FullBackupImportAction::Skip)
        .any(backup_table_has_sensitive_data))
}

async fn build_full_restore_plan(
    pool: &PgPool,
    archive: &FullBackupArchive,
) -> Result<FullBackupImportPlan, AppError> {
    let mut tables = Vec::with_capacity(archive.tables.len());
    for table in &archive.tables {
        let deletes = count_current_table_rows(pool, &table.name).await?;
        tables.push(FullBackupTablePlan {
            name: table.name.clone(),
            section: normalized_table_section(table),
            action: FullBackupImportAction::Replace,
            rows: table.rows.len(),
            inserts: table.rows.len() as i64,
            updates: 0,
            deletes,
            unchanged: 0,
            policy: FullBackupAppliedTablePolicy {
                update_columns: table
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect(),
                preserve_columns: Vec::new(),
                null_policy: FullBackupNullPolicy::Apply,
                json_merge_columns: Vec::new(),
            },
            warnings: Vec::new(),
        });
    }
    let has_destructive_changes = tables.iter().any(|table| table.deletes > 0);
    Ok(FullBackupImportPlan {
        dry_run: true,
        table_count: tables.len(),
        row_count: archive.row_count,
        has_destructive_changes,
        warnings: Vec::new(),
        tables,
    })
}

/// Instance schema metadata needed to plan a selective restore.
struct BackupSchemaMetadata<'a> {
    existing_tables: &'a HashSet<String>,
    existing_columns: &'a HashMap<String, Vec<ColumnMeta>>,
    nullable_columns: &'a HashSet<(String, String)>,
    foreign_keys: &'a [ForeignKeyMeta],
    primary_keys: &'a HashMap<String, Vec<String>>,
}

fn prepare_selected_backup_tables(
    tables: Vec<FullBackupTable>,
    selected_names: &HashSet<String>,
    rules: &FullBackupImportRules,
    schema: &BackupSchemaMetadata<'_>,
) -> Result<Vec<SelectedBackupTable>, AppError> {
    let &BackupSchemaMetadata {
        existing_tables,
        existing_columns,
        nullable_columns,
        foreign_keys,
        primary_keys,
    } = schema;
    let selected_table_names = tables
        .iter()
        .filter(|table| selected_names.contains(&table.name))
        .map(|table| table.name.clone())
        .collect::<HashSet<_>>();
    let mut selected_tables = Vec::new();
    let mut restored_tables = HashSet::<String>::new();

    for table in tables {
        if !selected_names.contains(&table.name) {
            continue;
        }
        if !existing_tables.contains(&table.name) {
            return Err(AppError::Validation(format!(
                "Backup table '{}' does not exist in this instance schema",
                table.name
            )));
        }
        validate_archive_table_columns(&table, existing_columns, false)?;
        let action = resolve_import_action(rules, &table);
        if action == FullBackupImportAction::Skip {
            continue;
        }
        let primary_key_columns = primary_keys.get(&table.name).cloned().ok_or_else(|| {
            AppError::Validation(format!(
                "Cannot selectively restore table '{}' because it has no primary key",
                table.name
            ))
        })?;
        let column_types = existing_columns
            .get(&table.name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|column| (column.name, column.data_type))
            .collect::<HashMap<_, _>>();
        let policy = resolve_table_policy(rules, &table, &primary_key_columns, &column_types)?;
        let deferrable_columns = foreign_keys
            .iter()
            .filter(|foreign_key| {
                foreign_key.child_table == table.name
                    && selected_table_names.contains(&foreign_key.parent_table)
                    && !restored_tables.contains(&foreign_key.parent_table)
                    && nullable_columns.contains(&(
                        foreign_key.child_table.clone(),
                        foreign_key.child_column.clone(),
                    ))
                    && table
                        .columns
                        .iter()
                        .any(|column| column.name == foreign_key.child_column)
                    && primary_key_columns.iter().all(|primary_key| {
                        table
                            .columns
                            .iter()
                            .any(|column| column.name == *primary_key)
                    })
            })
            .map(|foreign_key| foreign_key.child_column.clone())
            .collect::<HashSet<_>>();

        restored_tables.insert(table.name.clone());
        selected_tables.push(SelectedBackupTable {
            table,
            action,
            primary_key_columns,
            deferrable_columns,
            policy,
            column_types,
        });
    }

    if selected_tables.is_empty() {
        return Err(AppError::Validation(
            "Select at least one backup section or table to restore".to_string(),
        ));
    }

    Ok(selected_tables)
}

fn resolve_table_policy(
    rules: &FullBackupImportRules,
    table: &FullBackupTable,
    primary_key_columns: &[String],
    column_types: &HashMap<String, String>,
) -> Result<ResolvedTablePolicy, AppError> {
    let Some(policy) = rules.table_policies.get(&table.name) else {
        return Ok(ResolvedTablePolicy::default());
    };
    let archive_columns = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let primary_keys = primary_key_columns
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let update_columns = policy
        .update_columns
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let preserve_columns = policy
        .preserve_columns
        .iter()
        .cloned()
        .collect::<HashSet<_>>();

    for column in update_columns.iter().chain(preserve_columns.iter()) {
        validate_policy_column(table, column, &archive_columns, &primary_keys)?;
    }
    for column in policy.json_merge_columns.keys() {
        validate_policy_column(table, column, &archive_columns, &primary_keys)?;
        match column_types.get(column).map(String::as_str) {
            Some("json" | "jsonb") => {}
            _ => {
                return Err(AppError::Validation(format!(
                    "Backup table policy for '{}.{}' uses JSON merge on a non-json column",
                    table.name, column
                )));
            }
        }
    }

    let conflicts = update_columns
        .intersection(&preserve_columns)
        .cloned()
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        return Err(AppError::Validation(format!(
            "Backup table policy for '{}' lists columns as both update and preserve: {}",
            table.name,
            conflicts.join(", ")
        )));
    }

    Ok(ResolvedTablePolicy {
        explicit_update_columns: if update_columns.is_empty() {
            None
        } else {
            Some(update_columns)
        },
        preserve_columns,
        null_policy: policy.null_policy,
        json_merge_columns: policy.json_merge_columns.clone(),
    })
}

fn validate_policy_column(
    table: &FullBackupTable,
    column: &str,
    archive_columns: &HashSet<&str>,
    primary_keys: &HashSet<&str>,
) -> Result<(), AppError> {
    if !archive_columns.contains(column) {
        return Err(AppError::Validation(format!(
            "Backup table policy for '{}' references unknown archive column '{}'",
            table.name, column
        )));
    }
    if primary_keys.contains(column) {
        return Err(AppError::Validation(format!(
            "Backup table policy for '{}' cannot target primary key column '{}'",
            table.name, column
        )));
    }
    Ok(())
}

async fn plan_selected_backup_tables(
    tx: &mut Transaction<'_, Postgres>,
    selected_tables: &[SelectedBackupTable],
    dry_run: bool,
) -> Result<FullBackupImportPlan, AppError> {
    let mut tables = Vec::with_capacity(selected_tables.len());
    let mut row_count = 0usize;
    let mut has_destructive_changes = false;
    let mut warnings = Vec::new();

    for selected in selected_tables {
        let table_plan = plan_selected_backup_table(tx, selected).await?;
        row_count += table_plan.rows;
        has_destructive_changes |= table_plan.deletes > 0;
        warnings.extend(table_plan.warnings.iter().cloned());
        tables.push(table_plan);
    }

    Ok(FullBackupImportPlan {
        dry_run,
        table_count: tables.len(),
        row_count,
        has_destructive_changes,
        warnings,
        tables,
    })
}

async fn plan_selected_backup_table(
    tx: &mut Transaction<'_, Postgres>,
    selected: &SelectedBackupTable,
) -> Result<FullBackupTablePlan, AppError> {
    validate_primary_key_columns(&selected.table, &selected.primary_key_columns)?;
    let mut warnings = Vec::new();
    let deletes = if selected.action == FullBackupImportAction::Delete {
        if selected.table.rows.is_empty() {
            warnings.push(FullBackupPlanWarning {
                scope: "table".to_string(),
                name: selected.table.name.clone(),
                code: "delete_missing_empty_archive".to_string(),
                message:
                    "Delete missing is selected and the archive contains no rows for this table"
                        .to_string(),
            });
        }
        count_delete_missing_rows(tx, &selected.table, &selected.primary_key_columns).await?
    } else {
        0
    };
    let (inserts, updates, unchanged) = count_upsert_changes(tx, selected).await?;

    Ok(FullBackupTablePlan {
        name: selected.table.name.clone(),
        section: normalized_table_section(&selected.table),
        action: selected.action,
        rows: selected.table.rows.len(),
        inserts,
        updates,
        deletes,
        unchanged,
        policy: applied_table_policy(selected),
        warnings,
    })
}

fn applied_table_policy(selected: &SelectedBackupTable) -> FullBackupAppliedTablePolicy {
    let mut update_columns = selected
        .policy
        .explicit_update_columns
        .clone()
        .unwrap_or_else(|| effective_plan_update_columns(selected))
        .into_iter()
        .collect::<Vec<_>>();
    update_columns.sort();
    let mut preserve_columns = selected
        .policy
        .preserve_columns
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    preserve_columns.sort();
    let mut json_merge_columns = selected
        .policy
        .json_merge_columns
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    json_merge_columns.sort();
    FullBackupAppliedTablePolicy {
        update_columns,
        preserve_columns,
        null_policy: selected.policy.null_policy,
        json_merge_columns,
    }
}

fn select_tables(
    tables: Vec<TableMeta>,
    selection: Option<&FullBackupSelection>,
) -> Result<Vec<TableMeta>, AppError> {
    let Some(selection) = selection else {
        return Ok(tables);
    };
    if selection.sections.is_empty() && selection.tables.is_empty() {
        return Ok(tables);
    }
    let selected = tables
        .into_iter()
        .filter(|table| selection_matches_table(selection, &table.name, table_section(&table.name)))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(AppError::Validation(
            "Backup selection does not match any application tables".to_string(),
        ));
    }
    Ok(selected)
}

fn selected_archive_table_names(
    tables: &[FullBackupTable],
    selection: Option<&FullBackupSelection>,
) -> Result<HashSet<String>, AppError> {
    let names = tables
        .iter()
        .filter(|table| {
            selection
                .map(|selection| {
                    if selection.sections.is_empty() && selection.tables.is_empty() {
                        true
                    } else {
                        selection_matches_table(
                            selection,
                            &table.name,
                            normalized_table_section(table),
                        )
                    }
                })
                .unwrap_or(true)
        })
        .map(|table| table.name.clone())
        .collect::<HashSet<_>>();
    if names.is_empty() {
        return Err(AppError::Validation(
            "Backup selection does not match any tables in the archive".to_string(),
        ));
    }
    Ok(names)
}

fn selection_matches_table(
    selection: &FullBackupSelection,
    table_name: &str,
    section: FullBackupSection,
) -> bool {
    selection.tables.iter().any(|table| table == table_name)
        || selection.sections.contains(&section)
}

fn resolve_import_action(
    rules: &FullBackupImportRules,
    table: &FullBackupTable,
) -> FullBackupImportAction {
    rules
        .table_policies
        .get(&table.name)
        .and_then(|policy| policy.action)
        .or_else(|| rules.tables.get(&table.name).copied())
        .or_else(|| {
            rules
                .sections
                .get(&normalized_table_section(table))
                .copied()
        })
        .unwrap_or(rules.default_action)
}

fn normalized_table_section(table: &FullBackupTable) -> FullBackupSection {
    table_section(&table.name)
}

fn table_section(table_name: &str) -> FullBackupSection {
    let name = table_name.to_ascii_lowercase();
    if name.starts_with("dcim_")
        || name.starts_with("ipam_")
        || name.starts_with("circuits_")
        || name.starts_with("virtualization_")
        || name.starts_with("tenancy_")
        || name.starts_with("vpn_")
        || name.starts_with("config_context")
        || name.starts_with("custom_field")
        || name.starts_with("extras_")
        || name.starts_with("taggit_")
        || name.starts_with("contenttypes_")
        || name.starts_with("inventory_content_type")
    {
        FullBackupSection::Inventory
    } else if name.starts_with("connection")
        || name.starts_with("connections_")
        || name.starts_with("connconf_")
        || name.starts_with("proxy")
        || name.starts_with("session")
        || name.contains("workspace")
    {
        FullBackupSection::Connections
    } else if name.starts_with("access_control_")
        || name.starts_with("auth_")
        || name.starts_with("identity_")
        || name.starts_with("rbac_")
        || name.starts_with("oidc_")
        || name.starts_with("sso_")
        || name.contains("api_token")
    {
        FullBackupSection::Access
    } else if name.starts_with("secret")
        || name.starts_with("secrets_")
        || name.contains("credential")
    {
        FullBackupSection::Secrets
    } else if name.starts_with("playbook")
        || name.starts_with("automation")
        || name.starts_with("schedule")
        || name.starts_with("service_principal")
        || name.contains("ansible")
    {
        FullBackupSection::Automation
    } else if name.starts_with("sync_")
        || name.starts_with("mapping_")
        || name.starts_with("entity_mappings")
        || name.starts_with("field_mappings")
        || name.starts_with("inventory_template")
    {
        FullBackupSection::Synchronization
    } else if name.starts_with("audit")
        || name.starts_with("observability")
        || name.starts_with("monitoring")
        || name.starts_with("webhook")
        || name.starts_with("service_discovery")
        || name.starts_with("provider_request_journal")
    {
        FullBackupSection::Observability
    } else if name.starts_with("assistant_") {
        FullBackupSection::Assistant
    } else if name.starts_with("license") {
        FullBackupSection::Licensing
    } else if name.starts_with("instance_branding") {
        FullBackupSection::Appearance
    } else {
        FullBackupSection::Settings
    }
}

fn validate_archive_table_columns(
    table: &FullBackupTable,
    existing_columns: &HashMap<String, Vec<ColumnMeta>>,
    exact: bool,
) -> Result<(), AppError> {
    let current_columns = existing_columns
        .get(&table.name)
        .cloned()
        .unwrap_or_default();
    let current_by_name = current_columns
        .iter()
        .map(|column| (column.name.as_str(), column))
        .collect::<HashMap<_, _>>();
    let archive_columns = table
        .columns
        .iter()
        .map(|column| ColumnMeta {
            name: column.name.clone(),
            data_type: column.data_type.clone(),
        })
        .collect::<Vec<_>>();
    if exact {
        if archive_columns != current_columns {
            return Err(AppError::Validation(format!(
                "Backup columns for table '{}' do not match this instance schema",
                table.name
            )));
        }
        return Ok(());
    }
    for archive_column in archive_columns {
        match current_by_name.get(archive_column.name.as_str()) {
            Some(current_column) if current_column.data_type == archive_column.data_type => {}
            _ => {
                return Err(AppError::Validation(format!(
                    "Backup column '{}.{}' does not match this instance schema",
                    table.name, archive_column.name
                )));
            }
        }
    }
    Ok(())
}

async fn ordered_backup_tables(pool: &PgPool) -> Result<Vec<TableMeta>, AppError> {
    let table_names = load_table_names(pool).await?;
    let columns_by_table = load_columns(pool).await?;
    let ordered_names = order_tables_by_dependencies(&table_names, &load_dependencies(pool).await?);
    Ok(ordered_names
        .into_iter()
        .map(|name| TableMeta {
            columns: columns_by_table.get(&name).cloned().unwrap_or_default(),
            name,
        })
        .collect())
}

async fn load_table_names(pool: &PgPool) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_type = 'BASE TABLE'
        ORDER BY table_name
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to inspect backup tables: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|row| row.get::<String, _>("table_name"))
        .filter(|name| !is_excluded_backup_table(name))
        .collect())
}

async fn load_schema_version(pool: &PgPool) -> Result<Option<i64>, AppError> {
    let row = sqlx::query("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL AS exists")
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to inspect migration table: {e}")))?;
    if !row.get::<bool, _>("exists") {
        return Ok(None);
    }

    let row = sqlx::query("SELECT max(version) AS schema_version FROM public._sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to inspect schema version: {e}")))?;
    Ok(row.get::<Option<i64>, _>("schema_version"))
}

async fn load_columns(pool: &PgPool) -> Result<HashMap<String, Vec<ColumnMeta>>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT table_name, column_name, udt_name
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND is_generated = 'NEVER'
        ORDER BY table_name, ordinal_position
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to inspect backup columns: {e}")))?;

    let mut columns = HashMap::<String, Vec<ColumnMeta>>::new();
    for row in rows {
        let table_name = row.get::<String, _>("table_name");
        if EXCLUDED_TABLES.contains(&table_name.as_str()) {
            continue;
        }
        columns.entry(table_name).or_default().push(ColumnMeta {
            name: row.get::<String, _>("column_name"),
            data_type: row.get::<String, _>("udt_name"),
        });
    }
    Ok(columns)
}

async fn load_primary_keys(pool: &PgPool) -> Result<HashMap<String, Vec<String>>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT cls.relname AS table_name, attr.attname AS column_name, key_position.ordinality
        FROM pg_index idx
        JOIN pg_class cls ON cls.oid = idx.indrelid
        JOIN pg_namespace ns ON ns.oid = cls.relnamespace
        JOIN LATERAL unnest(idx.indkey) WITH ORDINALITY key_position(attnum, ordinality) ON true
        JOIN pg_attribute attr ON attr.attrelid = cls.oid AND attr.attnum = key_position.attnum
        WHERE idx.indisprimary
          AND ns.nspname = 'public'
        ORDER BY cls.relname, key_position.ordinality
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to inspect backup primary keys: {e}")))?;

    let mut primary_keys = HashMap::<String, Vec<String>>::new();
    for row in rows {
        let table_name = row.get::<String, _>("table_name");
        if EXCLUDED_TABLES.contains(&table_name.as_str()) {
            continue;
        }
        primary_keys
            .entry(table_name)
            .or_default()
            .push(row.get::<String, _>("column_name"));
    }
    Ok(primary_keys)
}

async fn load_nullable_columns(pool: &PgPool) -> Result<HashSet<(String, String)>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT table_name, column_name
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND is_nullable = 'YES'
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to inspect nullable columns: {e}")))?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let table_name = row.get::<String, _>("table_name");
            if EXCLUDED_TABLES.contains(&table_name.as_str()) {
                None
            } else {
                Some((table_name, row.get::<String, _>("column_name")))
            }
        })
        .collect())
}

async fn load_dependencies(pool: &PgPool) -> Result<Vec<(String, String)>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT child.relname AS child_table, parent.relname AS parent_table
        FROM pg_constraint c
        JOIN pg_class child ON child.oid = c.conrelid
        JOIN pg_namespace child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_class parent ON parent.oid = c.confrelid
        JOIN pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace
        WHERE c.contype = 'f'
          AND child_ns.nspname = 'public'
          AND parent_ns.nspname = 'public'
          AND child.relname <> parent.relname
        ORDER BY child.relname, parent.relname
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to inspect backup dependencies: {e}")))?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let child = row.get::<String, _>("child_table");
            let parent = row.get::<String, _>("parent_table");
            if EXCLUDED_TABLES.contains(&child.as_str())
                || EXCLUDED_TABLES.contains(&parent.as_str())
            {
                None
            } else {
                Some((child, parent))
            }
        })
        .collect())
}

async fn load_foreign_keys(pool: &PgPool) -> Result<Vec<ForeignKeyMeta>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT child.relname AS child_table,
               child_att.attname AS child_column,
               parent.relname AS parent_table
        FROM pg_constraint c
        JOIN pg_class child ON child.oid = c.conrelid
        JOIN pg_namespace child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_class parent ON parent.oid = c.confrelid
        JOIN pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace
        JOIN LATERAL unnest(c.conkey) WITH ORDINALITY child_key(attnum, ord) ON true
        JOIN LATERAL unnest(c.confkey) WITH ORDINALITY parent_key(attnum, ord) ON parent_key.ord = child_key.ord
        JOIN pg_attribute child_att ON child_att.attrelid = child.oid
            AND child_att.attnum = child_key.attnum
        WHERE c.contype = 'f'
          AND array_length(c.conkey, 1) = 1
          AND child_ns.nspname = 'public'
          AND parent_ns.nspname = 'public'
          AND child.relname <> parent.relname
        ORDER BY child.relname, child_att.attname, parent.relname
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to inspect backup foreign keys: {e}")))?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let child_table = row.get::<String, _>("child_table");
            let parent_table = row.get::<String, _>("parent_table");
            if EXCLUDED_TABLES.contains(&child_table.as_str())
                || EXCLUDED_TABLES.contains(&parent_table.as_str())
            {
                None
            } else {
                Some(ForeignKeyMeta {
                    child_table,
                    child_column: row.get::<String, _>("child_column"),
                    parent_table,
                })
            }
        })
        .collect())
}

fn order_tables_by_dependencies(
    table_names: &[String],
    dependencies: &[(String, String)],
) -> Vec<String> {
    let known = table_names.iter().cloned().collect::<HashSet<_>>();
    let mut pending = table_names.iter().cloned().collect::<HashSet<_>>();
    let mut ordered = Vec::with_capacity(table_names.len());

    while !pending.is_empty() {
        let mut ready = pending
            .iter()
            .filter(|table| {
                dependencies.iter().all(|(child, parent)| {
                    child != *table || !known.contains(parent) || ordered.contains(parent)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        ready.sort();

        if ready.is_empty() {
            let mut remaining = pending.iter().cloned().collect::<Vec<_>>();
            remaining.sort();
            ordered.extend(remaining);
            break;
        }

        for table in ready {
            pending.remove(&table);
            ordered.push(table);
        }
    }

    ordered
}

async fn export_table_rows(
    tx: &mut Transaction<'_, Postgres>,
    table_name: &str,
) -> Result<Vec<Value>, AppError> {
    let sql = format!(
        "SELECT to_jsonb(t) AS row FROM {} t",
        quote_table(table_name)
    );
    let rows = sqlx::query(&sql)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to export table '{table_name}': {e}")))?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Value, _>("row"))
        .collect())
}

async fn count_current_table_rows(pool: &PgPool, table_name: &str) -> Result<i64, AppError> {
    let sql = format!("SELECT COUNT(*) AS count FROM {}", quote_table(table_name));
    let row = sqlx::query(&sql).fetch_one(pool).await.map_err(|e| {
        AppError::Database(format!(
            "Failed to count rows for table '{table_name}': {e}"
        ))
    })?;
    Ok(row.get::<i64, _>("count"))
}

async fn count_delete_missing_rows(
    tx: &mut Transaction<'_, Postgres>,
    table: &FullBackupTable,
    primary_key_columns: &[String],
) -> Result<i64, AppError> {
    validate_primary_key_columns(table, primary_key_columns)?;
    if table.rows.is_empty() {
        let sql = format!("SELECT COUNT(*) AS count FROM {}", quote_table(&table.name));
        let row = sqlx::query(&sql).fetch_one(&mut **tx).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to plan missing-row deletes for table '{}': {e}",
                table.name
            ))
        })?;
        return Ok(row.get::<i64, _>("count"));
    }

    let rows = Value::Array(table.rows.clone());
    let predicates = primary_key_columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            format!("target.{quoted} = source.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "SELECT COUNT(*) AS count FROM {} AS target WHERE NOT EXISTS (SELECT 1 FROM jsonb_populate_recordset(NULL::{}, $1::jsonb) AS source WHERE {})",
        quote_table(&table.name),
        quote_table(&table.name),
        predicates,
    );
    let row = sqlx::query(&sql)
        .bind(rows)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to plan missing-row deletes for table '{}': {e}",
                table.name
            ))
        })?;
    Ok(row.get::<i64, _>("count"))
}

async fn count_upsert_changes(
    tx: &mut Transaction<'_, Postgres>,
    selected: &SelectedBackupTable,
) -> Result<(i64, i64, i64), AppError> {
    if selected.table.rows.is_empty() || selected.table.columns.is_empty() {
        return Ok((0, 0, 0));
    }
    let rows = Value::Array(selected.table.rows.clone());
    let join_predicates = selected
        .primary_key_columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            format!("target.{quoted} = source.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let first_key_column = selected.primary_key_columns.first().ok_or_else(|| {
        AppError::Internal(format!(
            "Missing primary key columns while planning upserts for table '{}'",
            selected.table.name
        ))
    })?;
    let first_key = quote_ident(first_key_column);
    let update_columns = effective_plan_update_columns(selected);
    let changed_predicate = changed_predicate("target", "source", &update_columns, selected);
    let sql = format!(
        "WITH source AS (SELECT * FROM jsonb_populate_recordset(NULL::{}, $1::jsonb)) \
         SELECT \
           COUNT(*) FILTER (WHERE target.{first_key} IS NULL) AS inserts, \
           COUNT(*) FILTER (WHERE target.{first_key} IS NOT NULL AND ({changed_predicate})) AS updates, \
           COUNT(*) FILTER (WHERE target.{first_key} IS NOT NULL AND NOT ({changed_predicate})) AS unchanged \
         FROM source LEFT JOIN {} AS target ON {}",
        quote_table(&selected.table.name),
        quote_table(&selected.table.name),
        join_predicates,
    );
    let row = sqlx::query(&sql)
        .bind(rows)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to plan upserts for table '{}': {e}",
                selected.table.name
            ))
        })?;
    Ok((
        row.get::<i64, _>("inserts"),
        row.get::<i64, _>("updates"),
        row.get::<i64, _>("unchanged"),
    ))
}

async fn delete_missing_rows(
    tx: &mut Transaction<'_, Postgres>,
    table: &FullBackupTable,
    primary_key_columns: &[String],
) -> Result<(), AppError> {
    validate_primary_key_columns(table, primary_key_columns)?;
    if table.rows.is_empty() {
        let sql = format!("DELETE FROM {}", quote_table(&table.name));
        sqlx::query(&sql).execute(&mut **tx).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to delete missing rows for table '{}': {e}",
                table.name
            ))
        })?;
        return Ok(());
    }

    let rows = Value::Array(table.rows.clone());
    let predicates = primary_key_columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            format!("target.{quoted} = source.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "DELETE FROM {} AS target WHERE NOT EXISTS (SELECT 1 FROM jsonb_populate_recordset(NULL::{}, $1::jsonb) AS source WHERE {})",
        quote_table(&table.name),
        quote_table(&table.name),
        predicates,
    );
    sqlx::query(&sql)
        .bind(rows)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to delete missing rows for table '{}': {e}",
                table.name
            ))
        })?;
    Ok(())
}

async fn upsert_table_rows(
    tx: &mut Transaction<'_, Postgres>,
    selected: &SelectedBackupTable,
) -> Result<(), AppError> {
    validate_primary_key_columns(&selected.table, &selected.primary_key_columns)?;
    let columns = selected
        .table
        .columns
        .iter()
        .filter(|column| !selected.deferrable_columns.contains(&column.name))
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let column_set = columns.iter().cloned().collect::<HashSet<_>>();
    if !selected
        .primary_key_columns
        .iter()
        .all(|primary_key| column_set.contains(primary_key))
    {
        return Err(AppError::Validation(format!(
            "Backup table '{}' does not include its primary key columns",
            selected.table.name
        )));
    }
    let column_list = columns
        .iter()
        .map(|column| quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    let conflict_target = selected
        .primary_key_columns
        .iter()
        .map(|column| quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    let update_columns = effective_update_columns(selected)
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            let expression = assignment_expression("target", "EXCLUDED", column, selected);
            format!("{quoted} = {expression}")
        })
        .collect::<Vec<_>>();
    let conflict_action = if update_columns.is_empty() {
        "DO NOTHING".to_string()
    } else {
        let distinct_predicate = changed_predicate(
            "target",
            "EXCLUDED",
            &effective_update_columns(selected),
            selected,
        );
        format!(
            "DO UPDATE SET {} WHERE {}",
            update_columns.join(", "),
            distinct_predicate
        )
    };
    let rows = Value::Array(selected.table.rows.clone());
    let sql = format!(
        "INSERT INTO {} AS target ({}) OVERRIDING SYSTEM VALUE SELECT {} FROM jsonb_populate_recordset(NULL::{}, $1::jsonb) ON CONFLICT ({}) {}",
        quote_table(&selected.table.name),
        column_list,
        column_list,
        quote_table(&selected.table.name),
        conflict_target,
        conflict_action,
    );
    sqlx::query(&sql)
        .bind(rows)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to selectively restore table '{}': {e}",
                selected.table.name
            ))
        })?;
    Ok(())
}

async fn apply_deferred_column_update(
    tx: &mut Transaction<'_, Postgres>,
    update: DeferredColumnUpdate,
) -> Result<(), AppError> {
    let update_columns = update
        .columns
        .iter()
        .filter(|column| update.policy.column_is_update_candidate(column))
        .cloned()
        .collect::<Vec<_>>();
    if update_columns.is_empty() {
        return Ok(());
    }
    let selected = SelectedBackupTable {
        table: FullBackupTable {
            name: update.table_name.clone(),
            section: table_section(&update.table_name),
            columns: update
                .columns
                .iter()
                .map(|column| FullBackupColumn {
                    name: column.clone(),
                    data_type: update
                        .column_types
                        .get(column)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                })
                .collect(),
            rows: Vec::new(),
        },
        action: FullBackupImportAction::Replace,
        primary_key_columns: update.primary_key_columns.clone(),
        deferrable_columns: HashSet::new(),
        policy: update.policy.clone(),
        column_types: update.column_types.clone(),
    };
    let assignments = update
        .columns
        .iter()
        .filter(|column| update.policy.column_is_update_candidate(column))
        .map(|column| {
            let quoted = quote_ident(column);
            let expression = assignment_expression("target", "source", column, &selected);
            format!("{quoted} = {expression}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let update_column_set = update_columns.iter().cloned().collect::<HashSet<_>>();
    let column_predicates = changed_predicate("target", "source", &update_column_set, &selected);
    let key_predicates = update
        .primary_key_columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            format!("target.{quoted} = source.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "UPDATE {} AS target SET {} FROM jsonb_populate_recordset(NULL::{}, $1::jsonb) AS source WHERE {} AND ({})",
        quote_table(&update.table_name),
        assignments,
        quote_table(&update.table_name),
        key_predicates,
        column_predicates,
    );
    sqlx::query(&sql)
        .bind(update.rows)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to restore deferred references for table '{}': {e}",
                update.table_name
            ))
        })?;
    Ok(())
}

#[cfg(test)]
fn conflict_update_distinct_predicate(
    target_alias: &str,
    source_alias: &str,
    columns: &[String],
    primary_key_columns: &[String],
) -> String {
    columns
        .iter()
        .filter(|column| !primary_key_columns.contains(column))
        .map(|column| {
            let quoted = quote_ident(column);
            format!("{target_alias}.{quoted} IS DISTINCT FROM {source_alias}.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl ResolvedTablePolicy {
    fn column_is_update_candidate(&self, column: &str) -> bool {
        if self.preserve_columns.contains(column) {
            return false;
        }
        self.explicit_update_columns
            .as_ref()
            .is_none_or(|columns| columns.contains(column))
    }
}

fn effective_update_columns(selected: &SelectedBackupTable) -> HashSet<String> {
    effective_update_columns_for(selected, false)
}

fn effective_plan_update_columns(selected: &SelectedBackupTable) -> HashSet<String> {
    effective_update_columns_for(selected, true)
}

fn effective_update_columns_for(
    selected: &SelectedBackupTable,
    include_deferrable: bool,
) -> HashSet<String> {
    selected
        .table
        .columns
        .iter()
        .map(|column| column.name.clone())
        .filter(|column| {
            !selected.primary_key_columns.contains(column)
                && (include_deferrable || !selected.deferrable_columns.contains(column))
                && selected.policy.column_is_update_candidate(column)
        })
        .collect::<HashSet<_>>()
}

fn changed_predicate(
    target_alias: &str,
    source_alias: &str,
    columns: &HashSet<String>,
    selected: &SelectedBackupTable,
) -> String {
    let mut columns = columns.iter().cloned().collect::<Vec<_>>();
    columns.sort();
    if columns.is_empty() {
        return "FALSE".to_string();
    }
    columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            let expression = assignment_expression(target_alias, source_alias, column, selected);
            format!("{target_alias}.{quoted} IS DISTINCT FROM {expression}")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn assignment_expression(
    target_alias: &str,
    source_alias: &str,
    column: &str,
    selected: &SelectedBackupTable,
) -> String {
    let quoted = quote_ident(column);
    let source = format!("{source_alias}.{quoted}");
    let mut expression = if selected.policy.json_merge_columns.contains_key(column) {
        json_merge_expression(target_alias, source_alias, column, selected)
    } else {
        source.clone()
    };
    if selected.policy.null_policy == FullBackupNullPolicy::PreserveExisting {
        expression = format!(
            "CASE WHEN {source} IS NULL THEN {target_alias}.{quoted} ELSE {expression} END"
        );
    }
    expression
}

fn json_merge_expression(
    target_alias: &str,
    source_alias: &str,
    column: &str,
    selected: &SelectedBackupTable,
) -> String {
    let quoted = quote_ident(column);
    let source = format!("{source_alias}.{quoted}");
    let target = format!("{target_alias}.{quoted}");
    let is_json = selected
        .column_types
        .get(column)
        .is_some_and(|data_type| data_type == "json");
    let source_jsonb = if is_json {
        format!("{source}::jsonb")
    } else {
        source.clone()
    };
    let target_jsonb = if is_json {
        format!("{target}::jsonb")
    } else {
        target.clone()
    };
    let mut merged = format!("({target_jsonb} || {source_jsonb})");
    if let Some(policy) = selected.policy.json_merge_columns.get(column) {
        let mut keys = policy.preserve_keys.clone();
        keys.sort();
        keys.dedup();
        for key in keys {
            let key_literal = sql_string_literal(&key);
            merged = format!(
                "CASE WHEN {target_jsonb} ? {key_literal} THEN ({merged} || jsonb_build_object({key_literal}, {target_jsonb} -> {key_literal})) ELSE {merged} END"
            );
        }
    }
    let expression = format!(
        "CASE WHEN jsonb_typeof({target_jsonb}) = 'object' AND jsonb_typeof({source_jsonb}) = 'object' THEN {merged} ELSE {source_jsonb} END"
    );
    if is_json {
        format!("({expression})::json")
    } else {
        expression
    }
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn validate_primary_key_columns(
    table: &FullBackupTable,
    primary_key_columns: &[String],
) -> Result<(), AppError> {
    if primary_key_columns.is_empty() {
        return Err(AppError::Validation(format!(
            "Cannot selectively restore table '{}' because it has no primary key",
            table.name
        )));
    }
    let archive_columns = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    for primary_key in primary_key_columns {
        if !archive_columns.contains(primary_key.as_str()) {
            return Err(AppError::Validation(format!(
                "Backup table '{}' does not include primary key column '{}'",
                table.name, primary_key
            )));
        }
    }
    Ok(())
}

fn is_sensitive_table(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "secret",
        "credential",
        "token",
        "session",
        "license",
        "password",
        "passphrase",
        "api_key",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn backup_table_has_sensitive_data(table: &FullBackupTable) -> bool {
    is_sensitive_table(&table.name)
        || table
            .columns
            .iter()
            .any(|column| is_sensitive_column(&column.name))
}

fn is_sensitive_column(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "secret",
        "password",
        "passphrase",
        "token",
        "private_key",
        "api_key",
        "access_key",
        "refresh",
        "encrypted_value",
        "client_secret",
        "credential",
        "cookie",
        "totp",
        "otp_seed",
        "mfa",
        "recovery_code",
        "signing_key",
        "encryption_key",
        "kdf_salt",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn quote_table(name: &str) -> String {
    format!("public.{}", quote_ident(name))
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn summarize_name_diff(left: &HashSet<String>, right: &HashSet<String>) -> String {
    let mut names = left.difference(right).cloned().collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        "none".to_string()
    } else if names.len() <= 8 {
        names.join(", ")
    } else {
        format!("{} and {} more", names[..8].join(", "), names.len() - 8)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{
        FULL_BACKUP_VERSION_V1, FullBackupArchive, FullBackupColumn, FullBackupImportAction,
        FullBackupImportRequest, FullBackupImportRules, FullBackupJsonMergeMode,
        FullBackupJsonMergePolicy, FullBackupNullPolicy, FullBackupSection, FullBackupSelection,
        FullBackupTable, FullBackupTablePolicy, ResolvedTablePolicy, SelectedBackupTable,
        TableMeta, assignment_expression, changed_predicate, conflict_update_distinct_predicate,
        full_backup_import_requires_secret_permission, is_excluded_backup_table,
        is_sensitive_column, is_sensitive_table, order_tables_by_dependencies,
        resolve_table_policy, select_tables, summarize_name_diff, table_section,
    };

    fn policy_test_table() -> FullBackupTable {
        FullBackupTable {
            name: "app_resource".to_string(),
            section: FullBackupSection::Settings,
            columns: vec![
                FullBackupColumn {
                    name: "id".to_string(),
                    data_type: "uuid".to_string(),
                },
                FullBackupColumn {
                    name: "name".to_string(),
                    data_type: "text".to_string(),
                },
                FullBackupColumn {
                    name: "metadata".to_string(),
                    data_type: "jsonb".to_string(),
                },
            ],
            rows: Vec::new(),
        }
    }

    fn policy_column_types() -> HashMap<String, String> {
        HashMap::from([
            ("id".to_string(), "uuid".to_string()),
            ("name".to_string(), "text".to_string()),
            ("metadata".to_string(), "jsonb".to_string()),
        ])
    }

    fn import_rules_for_table(policy: FullBackupTablePolicy) -> FullBackupImportRules {
        FullBackupImportRules {
            default_action: FullBackupImportAction::Replace,
            sections: HashMap::new(),
            tables: HashMap::new(),
            table_policies: HashMap::from([("app_resource".to_string(), policy)]),
        }
    }

    fn selected_policy_table(policy: ResolvedTablePolicy) -> SelectedBackupTable {
        SelectedBackupTable {
            table: policy_test_table(),
            action: FullBackupImportAction::Replace,
            primary_key_columns: vec!["id".to_string()],
            deferrable_columns: HashSet::new(),
            policy,
            column_types: policy_column_types(),
        }
    }

    fn import_request_for_table(
        table: FullBackupTable,
        selection: Option<FullBackupSelection>,
    ) -> FullBackupImportRequest {
        FullBackupImportRequest {
            payload: FullBackupArchive {
                version: FULL_BACKUP_VERSION_V1.to_string(),
                schema_version: Some(1),
                created_at: "2026-07-08T00:00:00Z".to_string(),
                include_secrets: false,
                table_count: 1,
                row_count: table.rows.len(),
                skipped: Vec::new(),
                tables: vec![table],
            },
            confirm_full_restore: false,
            dry_run: true,
            selection,
            rules: None,
        }
    }

    #[test]
    fn dependency_order_places_parents_first() {
        let tables = vec![
            "dcim_device".to_string(),
            "dcim_site".to_string(),
            "tenancy_tenant".to_string(),
        ];
        let dependencies = vec![
            ("dcim_device".to_string(), "dcim_site".to_string()),
            ("dcim_site".to_string(), "tenancy_tenant".to_string()),
        ];
        assert_eq!(
            order_tables_by_dependencies(&tables, &dependencies),
            vec!["tenancy_tenant", "dcim_site", "dcim_device"]
        );
    }

    #[test]
    fn sensitivity_detection_catches_secret_surfaces() {
        assert!(is_sensitive_table("access_control_user_api_token"));
        assert!(is_sensitive_table("auth_session"));
        assert!(is_sensitive_column("client_secret"));
        assert!(is_sensitive_column("encrypted_value"));
        assert!(!is_sensitive_column("site_id"));
    }

    #[test]
    fn backup_export_skips_migration_and_scratch_tables() {
        assert!(is_excluded_backup_table("_sqlx_migrations"));
        assert!(is_excluded_backup_table("_pb_inv_row_bak"));
        assert!(!is_excluded_backup_table("dcim_device"));
    }

    #[test]
    fn name_diff_summary_stays_compact() {
        let left = (0..10)
            .map(|index| format!("table_{index}"))
            .collect::<HashSet<_>>();
        let right = HashSet::new();
        let summary = summarize_name_diff(&left, &right);
        assert!(summary.contains("and 2 more"));
    }

    #[test]
    fn table_section_classifies_core_backup_tables() {
        assert_eq!(table_section("dcim_device"), FullBackupSection::Inventory);
        assert_eq!(
            table_section("connection_target"),
            FullBackupSection::Connections
        );
        assert_eq!(
            table_section("access_control_user"),
            FullBackupSection::Access
        );
        assert_eq!(
            table_section("playbook_schedule"),
            FullBackupSection::Automation
        );
        assert_eq!(table_section("license_state"), FullBackupSection::Licensing);
    }

    #[test]
    fn conflict_update_predicate_compares_only_non_primary_key_columns() {
        let columns = vec![
            "id".to_string(),
            "name".to_string(),
            "updated_at".to_string(),
        ];
        let primary_keys = vec!["id".to_string()];

        assert_eq!(
            conflict_update_distinct_predicate("target", "EXCLUDED", &columns, &primary_keys),
            "target.\"name\" IS DISTINCT FROM EXCLUDED.\"name\" OR target.\"updated_at\" IS DISTINCT FROM EXCLUDED.\"updated_at\""
        );
    }

    #[test]
    fn table_policy_validation_rejects_invalid_column_targets() {
        let table = policy_test_table();
        let primary_keys = vec!["id".to_string()];
        let column_types = policy_column_types();

        let unknown_column = resolve_table_policy(
            &import_rules_for_table(FullBackupTablePolicy {
                update_columns: vec!["missing".to_string()],
                ..Default::default()
            }),
            &table,
            &primary_keys,
            &column_types,
        )
        .expect_err("unknown columns must fail");
        assert!(format!("{unknown_column:?}").contains("unknown archive column"));

        let primary_key_column = resolve_table_policy(
            &import_rules_for_table(FullBackupTablePolicy {
                update_columns: vec!["id".to_string()],
                ..Default::default()
            }),
            &table,
            &primary_keys,
            &column_types,
        )
        .expect_err("primary key columns must fail");
        assert!(format!("{primary_key_column:?}").contains("primary key column"));

        let conflicting_column = resolve_table_policy(
            &import_rules_for_table(FullBackupTablePolicy {
                update_columns: vec!["name".to_string()],
                preserve_columns: vec!["name".to_string()],
                ..Default::default()
            }),
            &table,
            &primary_keys,
            &column_types,
        )
        .expect_err("update and preserve conflict must fail");
        assert!(format!("{conflicting_column:?}").contains("both update and preserve"));

        let non_json_merge = resolve_table_policy(
            &import_rules_for_table(FullBackupTablePolicy {
                json_merge_columns: HashMap::from([(
                    "name".to_string(),
                    FullBackupJsonMergePolicy {
                        mode: FullBackupJsonMergeMode::ShallowObject,
                        preserve_keys: Vec::new(),
                    },
                )]),
                ..Default::default()
            }),
            &table,
            &primary_keys,
            &column_types,
        )
        .expect_err("json merge on text columns must fail");
        assert!(format!("{non_json_merge:?}").contains("non-json column"));
    }

    #[test]
    fn assignment_expression_applies_null_and_json_merge_policies() {
        let null_preserve = selected_policy_table(ResolvedTablePolicy {
            null_policy: FullBackupNullPolicy::PreserveExisting,
            ..Default::default()
        });

        assert_eq!(
            assignment_expression("target", "source", "name", &null_preserve),
            "CASE WHEN source.\"name\" IS NULL THEN target.\"name\" ELSE source.\"name\" END"
        );
        assert_eq!(
            changed_predicate(
                "target",
                "source",
                &HashSet::from(["name".to_string()]),
                &null_preserve,
            ),
            "target.\"name\" IS DISTINCT FROM CASE WHEN source.\"name\" IS NULL THEN target.\"name\" ELSE source.\"name\" END"
        );

        let json_merge = selected_policy_table(ResolvedTablePolicy {
            json_merge_columns: HashMap::from([(
                "metadata".to_string(),
                FullBackupJsonMergePolicy {
                    mode: FullBackupJsonMergeMode::ShallowObject,
                    preserve_keys: vec!["server".to_string()],
                },
            )]),
            ..Default::default()
        });
        let expression = assignment_expression("target", "source", "metadata", &json_merge);

        assert!(expression.contains("target.\"metadata\" || source.\"metadata\""));
        assert!(expression.contains("jsonb_build_object('server'"));
    }

    #[test]
    fn secret_permission_detection_uses_actual_selected_tables() {
        let mut table = policy_test_table();
        table.name = "secret_token".to_string();
        table.section = FullBackupSection::Settings;
        let request = import_request_for_table(
            table,
            Some(FullBackupSelection {
                sections: Vec::new(),
                tables: vec!["secret_token".to_string()],
            }),
        );

        assert!(
            full_backup_import_requires_secret_permission(&request)
                .expect("selected secret table should be classified")
        );
    }

    #[test]
    fn import_selection_recomputes_section_from_table_name() {
        let mut table = policy_test_table();
        table.name = "secret_token".to_string();
        table.section = FullBackupSection::Settings;
        let request = import_request_for_table(
            table,
            Some(FullBackupSelection {
                sections: vec![FullBackupSection::Settings],
                tables: Vec::new(),
            }),
        );

        let error = full_backup_import_requires_secret_permission(&request)
            .expect_err("forged section metadata must not match settings selection");
        assert!(format!("{error:?}").contains("does not match"));
    }

    #[test]
    fn selection_filters_tables_by_section_and_exact_name() {
        let tables = vec![
            TableMeta {
                name: "dcim_device".to_string(),
                columns: Vec::new(),
            },
            TableMeta {
                name: "connection_target".to_string(),
                columns: Vec::new(),
            },
            TableMeta {
                name: "license_state".to_string(),
                columns: Vec::new(),
            },
        ];
        let selection = FullBackupSelection {
            sections: vec![FullBackupSection::Inventory],
            tables: vec!["license_state".to_string()],
        };
        let names = select_tables(tables, Some(&selection))
            .expect("selection should match tables")
            .into_iter()
            .map(|table| table.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["dcim_device", "license_state"]);
    }
}
