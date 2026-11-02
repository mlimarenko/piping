use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::contexts::inventory::infrastructure::persistence::repositories::delete_guard::{
    InventoryResourceIdentity, delete_inventory_record_in_transaction, inventory_delete_cascade,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::{
    lock_tag_catalog_for_definition, validate_scope_registry_in_transaction,
};
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
        prepare_backup_catalog_mutation(&mut tx).await?;
        prepare_backup_hardware_replay(&mut tx).await?;
        truncate_instance_tables(&mut tx, &existing_tables).await?;

        let device_bays = archive
            .tables
            .iter()
            .find(|table| table.name == "dcim_devicebay")
            .map(|table| table.rows.clone())
            .unwrap_or_default();
        let (row_count, deferred_updates) = restore_full_backup_tables(
            &mut tx,
            archive.tables,
            &existing_tables,
            &nullable_columns,
            &foreign_keys,
        )
        .await?;

        apply_deferred_column_updates(&mut tx, deferred_updates).await?;
        finish_archived_device_bays(&mut tx, device_bays, true).await?;
        validate_scope_registry_in_transaction(&mut tx).await?;
        finish_backup_hardware_replay(&mut tx).await?;

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

        let mut selected_tables = prepare_selected_backup_tables(
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
        // Tag references are canonical UUID strings, not SQL foreign keys.
        // Install the complete scope before memberships; reverse-order deletion
        // clears selected owners and scopes before definitions.
        selected_tables.sort_by_key(|selected| tag_catalog_replay_order(&selected.table.name));

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
    if !dry_run {
        prepare_backup_catalog_mutation(&mut tx).await?;
        prepare_backup_hardware_replay(&mut tx).await?;
    }
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

    // Release absent rows' immediate unique slots before replay, except where
    // their current cascade would retire an identity that replay must retain.
    let deferred_deletes = delete_selected_backup_rows(&mut tx, selected_tables).await?;
    replay_selected_backup_rows(&mut tx, selected_tables).await?;
    for (table, id) in deferred_deletes {
        delete_inventory_record_in_transaction(&mut tx, table, id).await?;
    }
    validate_replayed_identities(&mut tx, selected_tables).await?;
    validate_scope_registry_in_transaction(&mut tx).await?;
    sqlx::query("SELECT dcim_validate_module_graph(id) FROM dcim_device")
        .execute(&mut *tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    finish_backup_hardware_replay(&mut tx).await?;

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

/// Prepare existing installation dependencies before occupants, and stage new
/// bays as temporary roots like full restore. Finalize deferred references with
/// INSERT semantics for new rows and update policies for existing rows.
async fn replay_selected_backup_rows(
    tx: &mut Transaction<'_, Postgres>,
    selected_tables: &[SelectedBackupTable],
) -> Result<(), AppError> {
    let mut deferred_updates = Vec::<DeferredColumnUpdate>::new();
    let mut staged_device_bays = Vec::new();
    let mut staged_existing_device_bays = Vec::new();
    for selected in selected_tables {
        if matches!(
            selected.table.name.as_str(),
            "dcim_module" | "dcim_modulebay" | "dcim_modulebay_module_bay_types"
        ) {
            continue;
        }
        if selected.table.rows.is_empty() || selected.table.columns.is_empty() {
            continue;
        }
        let mut selected = selected.clone();
        // Module insertion is delayed until its bays exist, regardless of the
        // archive's original table order. Native component module_id FKs are
        // nullable and are restored after the explicit graph is present.
        if selected.table.name.starts_with("dcim_")
            && selected
                .table
                .columns
                .iter()
                .any(|column| column.name == "module_id")
        {
            selected.deferrable_columns.insert("module_id".into());
        }
        if selected.table.name == "dcim_devicebay"
            && selected
                .table
                .columns
                .iter()
                .any(|column| column.name == "enabled")
        {
            let (mut existing, mut inserted) = partition_selected_rows(tx, &selected).await?;
            if selected.policy.column_is_update_candidate("enabled")
                && selected
                    .policy
                    .column_is_update_candidate("installed_device_id")
                && selected
                    .table
                    .columns
                    .iter()
                    .any(|column| column.name == "installed_device_id")
            {
                // Only authoritative changes to both fields may temporarily
                // enable an existing bay. Kept fields retain the native guard.
                let replacements: Vec<Value> = sqlx::query_scalar("SELECT item.row FROM jsonb_array_elements($1::jsonb) item(row) CROSS JOIN LATERAL jsonb_populate_record(NULL::dcim_devicebay,item.row) source JOIN dcim_devicebay target ON target.id=source.id WHERE source.enabled=false AND source.installed_device_id IS NOT NULL AND source.installed_device_id IS DISTINCT FROM target.installed_device_id")
                    .bind(Value::Array(existing.table.rows.clone())).fetch_all(&mut **tx).await?;
                if !replacements.is_empty() {
                    existing.table.rows.retain(|row| {
                        !replacements
                            .iter()
                            .any(|replacement| replacement["id"] == row["id"])
                    });
                    let mut staged = existing.clone();
                    staged.table.rows = staged_device_bay_rows(&replacements)?;
                    replay_selected_table_rows(tx, &staged, &mut deferred_updates).await?;
                    staged_existing_device_bays.extend(replacements);
                }
            }
            replay_selected_table_rows(tx, &existing, &mut deferred_updates).await?;
            if !inserted.table.rows.is_empty() {
                inserted.policy = ResolvedTablePolicy::default();
                let mut staged = inserted.clone();
                staged.table.rows = staged_device_bay_rows(&inserted.table.rows)?;
                upsert_table_rows(tx, &staged).await?;
                if let Some(update) = deferred_column_update(&inserted) {
                    deferred_updates.push(update);
                }
                staged_device_bays.extend(inserted.table.rows);
            }
        } else {
            replay_selected_table_rows(tx, &selected, &mut deferred_updates).await?;
        }
    }
    let selected_bays = selected_tables
        .iter()
        .find(|selected| selected.table.name == "dcim_modulebay");
    let mut new_bays = None;
    if let Some(selected) = selected_bays {
        validate_selected_bay_containment(tx, selected, selected_tables).await?;
        let (existing, inserted) = partition_selected_rows(tx, selected).await?;
        // Installation guards must see the selected effective enabled state,
        // not the destination's stale value. Only UPDATE policy candidates are
        // prepared; preserved disabled bays must still reject new installations.
        let mut dependencies = existing.clone();
        dependencies.deferrable_columns = dependencies
            .table
            .columns
            .iter()
            .filter(|column| column.name == "enabled")
            .map(|column| column.name.clone())
            .collect();
        if let Some(update) = deferred_column_update(&dependencies) {
            apply_deferred_column_update(tx, update).await?;
        }
        if !inserted.table.rows.is_empty() {
            let mut staged = inserted.clone();
            staged.policy = ResolvedTablePolicy::default();
            staged.deferrable_columns.remove("module_id");
            staged.deferrable_columns.remove("parent_id");
            staged.table.rows = staged_module_bay_rows(&inserted.table.rows);
            upsert_table_rows(tx, &staged).await?;
        }
        new_bays = Some(inserted);
    }
    // Existing bays already carry live category restrictions. Apply selected
    // memberships before installation so guards consult their effective set.
    // New staged bays retain full-restore ordering (categories after occupants).
    for selected in selected_tables
        .iter()
        .filter(|selected| selected.table.name == "dcim_modulebay_module_bay_types")
    {
        let mut dependencies = selected.clone();
        if let Some(inserted) = &new_bays {
            dependencies.table.rows.retain(|row| {
                !inserted
                    .table
                    .rows
                    .iter()
                    .any(|bay| bay["id"] == row["modulebay_id"])
            });
        }
        upsert_table_rows(tx, &dependencies).await?;
    }
    for selected in selected_tables
        .iter()
        .filter(|selected| selected.table.name == "dcim_module")
    {
        replay_selected_table_rows(tx, selected, &mut deferred_updates).await?;
    }
    if let Some(selected) = selected_bays {
        let mut final_rows = selected.clone();
        final_rows.deferrable_columns.remove("module_id");
        final_rows.deferrable_columns.remove("parent_id");
        if let Some(inserted) = &new_bays {
            let inserted_ids = inserted
                .table
                .rows
                .iter()
                .map(|row| row["id"].clone())
                .collect::<Vec<_>>();
            final_rows
                .table
                .rows
                .retain(|row| !inserted_ids.contains(&row["id"]));
            // Preserve/update policies apply to pre-existing rows, not to a new
            // archive INSERT. Finalize every staged insert field so no temporary
            // name or enabled state can survive a selective preserve policy.
            let mut final_inserts = inserted.clone();
            final_inserts.policy = ResolvedTablePolicy::default();
            final_inserts.deferrable_columns.remove("module_id");
            final_inserts.deferrable_columns.remove("parent_id");
            replay_selected_table_rows(tx, &final_inserts, &mut deferred_updates).await?;
        }
        replay_selected_table_rows(tx, &final_rows, &mut deferred_updates).await?;
    }
    for selected in selected_tables
        .iter()
        .filter(|selected| selected.table.name == "dcim_modulebay_module_bay_types")
    {
        upsert_table_rows(tx, selected).await?;
    }
    for update in deferred_updates {
        apply_deferred_column_update(tx, update).await?;
    }
    finish_archived_device_bays(tx, staged_device_bays, true).await?;
    finish_archived_device_bays(tx, staged_existing_device_bays, false).await?;
    Ok(())
}

/// A cascade may remove only identities absent from the actual replay plan.
/// Check the same prepared actions, rows and composite keys the replayer uses,
/// not all archive rows (unselected/Skip/delete-only inputs are not survivors).
/// Missing survivors abort the transaction, rolling back canonical retirement.
async fn validate_replayed_identities(
    tx: &mut Transaction<'_, Postgres>,
    selected_tables: &[SelectedBackupTable],
) -> Result<(), AppError> {
    for selected in selected_tables {
        if selected.action == FullBackupImportAction::Skip
            || selected.table.rows.is_empty()
            || selected.table.columns.is_empty()
        {
            continue;
        }
        validate_primary_key_columns(&selected.table, &selected.primary_key_columns)?;
        let predicates = selected
            .primary_key_columns
            .iter()
            .map(|column| {
                let column = quote_ident(column);
                format!("target.{column} = source.{column}")
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let table = quote_table(&selected.table.name);
        let statement = format!(
            "SELECT EXISTS (SELECT 1 FROM jsonb_populate_recordset(NULL::{table}, $1::jsonb) source WHERE NOT EXISTS (SELECT 1 FROM {table} target WHERE {predicates}))"
        );
        let missing: bool = sqlx::query_scalar(&statement)
            .bind(Value::Array(selected.table.rows.clone()))
            .fetch_one(&mut **tx)
            .await?;
        if missing {
            return Err(AppError::Conflict(format!(
                "Selective restore cascade removed a replayed identity from '{}'; no changes were applied",
                selected.table.name
            )));
        }
    }
    Ok(())
}

async fn prepare_backup_hardware_replay(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    crate::contexts::inventory::infrastructure::persistence::repositories::dcim::hardware_graph::prepare_explicit_hardware_replay(tx).await?;
    crate::contexts::inventory::infrastructure::persistence::repositories::circuits::circuit_group_assignment_repository::lock_graph(tx).await?;
    // An existing nullable edge omitted from UPSERT remains live until deferred
    // references are applied. A valid archived graph can therefore pass through
    // a temporary cycle. Defer only this owned constraint during explicit replay;
    // native writes retain immediate validation and all other guards still run.
    sqlx::query("SET CONSTRAINTS cooling_graph_valid DEFERRED")
        .execute(&mut **tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    Ok(())
}

async fn finish_backup_hardware_replay(tx: &mut Transaction<'_, Postgres>) -> Result<(), AppError> {
    sqlx::query("SELECT dcim_validate_cooling_graph()")
        .execute(&mut **tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    // Drain the queued constraint checks now, before returning success or commit.
    sqlx::query("SET CONSTRAINTS cooling_graph_valid IMMEDIATE")
        .execute(&mut **tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    Ok(())
}

async fn validate_selected_bay_containment(
    tx: &mut Transaction<'_, Postgres>,
    bays: &SelectedBackupTable,
    selected_tables: &[SelectedBackupTable],
) -> Result<(), AppError> {
    for bay in &bays.table.rows {
        let expected = if let Some(module) = bay["module_id"].as_str() {
            let module_id = uuid::Uuid::parse_str(module)?;
            let selected = selected_tables
                .iter()
                .find(|selected| selected.table.name == "dcim_module")
                .and_then(|selected| {
                    selected
                        .table
                        .rows
                        .iter()
                        .find(|row| row["id"].as_str() == Some(module))
                });
            let module = if let Some(selected) = selected {
                selected.clone()
            } else {
                sqlx::query_scalar::<_, Value>("SELECT to_jsonb(m) FROM dcim_module m WHERE id=$1")
                    .bind(module_id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or_else(|| {
                        AppError::Validation("Archived containing module does not exist".into())
                    })?
            };
            if module["device_id"] != bay["device_id"] {
                return Err(AppError::Validation(
                    "Archived bay and containing module must share a device".into(),
                ));
            }
            module["module_bay_id"].clone()
        } else if bay["module_id"].is_null() {
            Value::Null
        } else {
            return Err(AppError::Validation(
                "Invalid archived containing module id".into(),
            ));
        };
        if bays
            .table
            .columns
            .iter()
            .any(|column| column.name == "parent_id")
            && bay["parent_id"] != expected
        {
            return Err(AppError::Validation(
                "Archived module graph has invalid containment".into(),
            ));
        }
    }
    Ok(())
}

/// Derive identities only from prepared inputs that the replayer will apply.
/// Other key shapes are outside native inventory's table+UUID identity contract.
fn replayed_inventory_identities(
    selected_tables: &[SelectedBackupTable],
) -> Result<HashSet<InventoryResourceIdentity>, AppError> {
    let mut survivors = HashSet::new();
    for selected in selected_tables {
        if selected.action == FullBackupImportAction::Skip
            || selected.table.rows.is_empty()
            || selected.table.columns.is_empty()
            || selected.primary_key_columns.len() != 1
            || selected.primary_key_columns[0] != "id"
            || !selected
                .column_types
                .get("id")
                .is_some_and(|kind| kind == "uuid")
        {
            continue;
        }
        validate_primary_key_columns(&selected.table, &selected.primary_key_columns)?;
        for row in &selected.table.rows {
            let id = row
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .ok_or_else(|| {
                    AppError::Validation(format!(
                        "Replay row in '{}' requires a UUID primary key",
                        selected.table.name
                    ))
                })?;
            survivors.insert(InventoryResourceIdentity {
                table: selected.table.name.clone(),
                id,
            });
        }
    }
    Ok(survivors)
}

/// Delete missing rows children-first as before, deferring only native roots
/// whose canonical cascade currently reaches a replay survivor. Both phases
/// retain the same guard, blockers and consumer retirement implementation.
async fn delete_selected_backup_rows(
    tx: &mut Transaction<'_, Postgres>,
    selected_tables: &[SelectedBackupTable],
) -> Result<Vec<(&'static str, uuid::Uuid)>, AppError> {
    let survivors = replayed_inventory_identities(selected_tables)?;
    let mut deferred = Vec::new();
    for selected in selected_tables.iter().rev() {
        if selected.action == FullBackupImportAction::Delete {
            if let Some(table) = guarded_restore_table(&selected.table.name) {
                let ids = missing_native_row_ids(
                    tx,
                    &selected.table,
                    &selected.primary_key_columns,
                    table,
                )
                .await?;
                for id in ids {
                    let cascade = inventory_delete_cascade(tx, table, id).await?;
                    if cascade.iter().any(|resource| survivors.contains(resource)) {
                        deferred.push((table, id));
                    } else {
                        delete_inventory_record_in_transaction(tx, table, id).await?;
                    }
                }
            } else {
                // Other existing missing-row deletions retain their prior path.
                delete_missing_rows(tx, &selected.table, &selected.primary_key_columns).await?;
            }
        }
    }
    Ok(deferred)
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

/// Classify against the destination before replay. Update-only policies must
/// never suppress supplied INSERT fields, including references written later.
async fn partition_selected_rows(
    tx: &mut Transaction<'_, Postgres>,
    selected: &SelectedBackupTable,
) -> Result<(SelectedBackupTable, SelectedBackupTable), AppError> {
    validate_primary_key_columns(&selected.table, &selected.primary_key_columns)?;
    let target = quote_table(&selected.table.name);
    let predicates = selected
        .primary_key_columns
        .iter()
        .map(|column| {
            let column = quote_ident(column);
            format!(
                "target.{column} = (jsonb_populate_record(NULL::{target}, source.row)).{column}"
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let statement = format!(
        "SELECT source.row, EXISTS(SELECT 1 FROM {target} target WHERE {predicates}) FROM jsonb_array_elements($1::jsonb) AS source(row)"
    );
    let rows: Vec<(Value, bool)> = sqlx::query_as(&statement)
        .bind(Value::Array(selected.table.rows.clone()))
        .fetch_all(&mut **tx)
        .await?;
    let mut existing = selected.clone();
    let mut inserted = selected.clone();
    existing.table.rows.clear();
    inserted.table.rows.clear();
    for (row, exists) in rows {
        if exists {
            existing.table.rows.push(row);
        } else {
            inserted.table.rows.push(row);
        }
    }
    Ok((existing, inserted))
}

async fn replay_selected_table_rows(
    tx: &mut Transaction<'_, Postgres>,
    selected: &SelectedBackupTable,
    deferred_updates: &mut Vec<DeferredColumnUpdate>,
) -> Result<(), AppError> {
    if selected.deferrable_columns.is_empty() {
        return upsert_table_rows(tx, selected).await;
    }
    let (existing, mut inserted) = partition_selected_rows(tx, selected).await?;
    upsert_table_rows(tx, selected).await?;
    inserted.policy = ResolvedTablePolicy::default();
    for rows in [&existing, &inserted] {
        if !rows.table.rows.is_empty()
            && let Some(update) = deferred_column_update(rows)
        {
            deferred_updates.push(update);
        }
    }
    Ok(())
}

async fn prepare_backup_catalog_mutation(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    // Choose isolation before the first query. Definition and membership writes
    // share a transaction; acquire the exclusive catalog lock before either.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut **tx)
        .await?;
    lock_tag_catalog_for_definition(tx).await
}

/// Clear semantic membership owners before guarded deletion of Tag definitions.
/// The catalog's live TRUNCATE/identity/reference guards are never bypassed.
async fn truncate_instance_tables(
    tx: &mut Transaction<'_, Postgres>,
    existing_tables: &HashSet<String>,
) -> Result<(), AppError> {
    let truncate_targets = existing_tables
        .iter()
        .filter(|table| table.as_str() != "inventory_tag")
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
    if existing_tables.contains("inventory_tag") {
        // All inventory membership arrays and executable selectors are now
        // empty. The definition guard still checks them for every deleted id.
        sqlx::query("DELETE FROM inventory_tag")
            .execute(&mut **tx)
            .await
            .map_err(crate::errors::classify_sqlx_error_owned)?;
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
    let mut graph_tables = HashMap::new();
    let mut graph_memberships = Vec::new();
    let mut ordinary_tables = Vec::new();
    for table in tables {
        match table.name.as_str() {
            "dcim_module" | "dcim_modulebay" => {
                graph_tables.insert(table.name.clone(), table);
            }
            "dcim_modulebay_module_bay_types" => graph_memberships.push(table),
            _ => ordinary_tables.push(table),
        }
    }
    validate_archived_module_graph(&graph_tables)?;
    ordinary_tables.sort_by_key(|table| tag_catalog_replay_order(&table.name));
    let mut row_count = 0usize;
    let mut restored_tables = HashSet::<String>::new();
    let mut deferred_updates = Vec::<DeferredColumnUpdate>::new();
    for table in ordinary_tables {
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
        let insert_rows = if table.name == "dcim_devicebay" {
            Value::Array(staged_device_bay_rows(&table.rows)?)
        } else {
            rows.clone()
        };
        sqlx::query(&sql)
            .bind(&insert_rows)
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
    row_count += restore_archived_module_graph(tx, graph_tables).await?;
    for table in graph_memberships {
        insert_full_backup_rows(tx, &table, Value::Array(table.rows.clone())).await?;
        row_count += table.rows.len();
    }
    Ok((row_count, deferred_updates))
}

fn staged_device_bay_rows(rows: &[Value]) -> Result<Vec<Value>, AppError> {
    let mut staged = rows.to_vec();
    for row in &mut staged {
        if row.get("enabled").and_then(Value::as_bool).is_none() {
            return Err(AppError::Validation(
                "Archived device bay requires a boolean enabled state".into(),
            ));
        }
        row["enabled"] = Value::Bool(true);
    }
    Ok(staged)
}

async fn finish_archived_device_bays(
    tx: &mut Transaction<'_, Postgres>,
    rows: Vec<Value>,
    require_archived_owner: bool,
) -> Result<(), AppError> {
    let rows = Value::Array(rows);
    // Installing while enabled exercises the native guard. Disabling afterwards
    // preserves a valid existing occupant without permitting a new installation.
    sqlx::query("UPDATE dcim_devicebay target SET enabled=source.enabled FROM jsonb_populate_recordset(NULL::dcim_devicebay,$1) source WHERE target.id=source.id")
        .bind(&rows).execute(&mut **tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
    let invalid: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM jsonb_populate_recordset(NULL::dcim_devicebay,$1) source LEFT JOIN dcim_devicebay target ON target.id=source.id WHERE target.id IS NULL OR target.enabled IS DISTINCT FROM source.enabled OR target.installed_device_id IS DISTINCT FROM source.installed_device_id OR ($2 AND target.device_id IS DISTINCT FROM source.device_id) OR target.installed_device_id=target.device_id)")
        .bind(&rows).bind(require_archived_owner).fetch_one(&mut **tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
    if invalid {
        return Err(AppError::Validation(
            "Archived device bay graph was not restored exactly".into(),
        ));
    }
    Ok(())
}

/// Validate archive references before staging can normalize parent projections.
/// Disabled/incompatible existing occupants are valid archived state, unlike a
/// fresh live installation. Structural errors still roll the entire restore back.
fn validate_archived_module_graph(
    tables: &HashMap<String, FullBackupTable>,
) -> Result<(), AppError> {
    let bays = tables
        .get("dcim_modulebay")
        .map(|table| table.rows.as_slice())
        .unwrap_or_default();
    let modules = tables
        .get("dcim_module")
        .map(|table| table.rows.as_slice())
        .unwrap_or_default();
    fn indexed(rows: &[Value]) -> Result<HashMap<&str, &Value>, AppError> {
        let mut result = HashMap::new();
        for row in rows {
            let id = row["id"].as_str().ok_or_else(|| {
                AppError::Validation("Archived module graph requires UUID ids".into())
            })?;
            uuid::Uuid::parse_str(id)
                .map_err(|_| AppError::Validation("Invalid archived module graph id".into()))?;
            if result.insert(id, row).is_some() {
                return Err(AppError::Validation(
                    "Duplicate archived module graph id".into(),
                ));
            }
        }
        Ok(result)
    }
    let bays = indexed(bays)?;
    let modules = indexed(modules)?;
    let invalid = || AppError::Validation("Archived module graph has invalid containment".into());
    let mut occupied = HashSet::new();
    for module in modules.values() {
        let bay_id = module["module_bay_id"].as_str().ok_or_else(invalid)?;
        let bay = bays.get(bay_id).ok_or_else(invalid)?;
        if module["device_id"] != bay["device_id"] || !occupied.insert(bay_id) {
            return Err(invalid());
        }
    }
    for (id, bay) in &bays {
        let expected_parent = if let Some(module_id) = bay["module_id"].as_str() {
            let module = modules.get(module_id).ok_or_else(invalid)?;
            if module["device_id"] != bay["device_id"] {
                return Err(invalid());
            }
            &module["module_bay_id"]
        } else if bay["module_id"].is_null() {
            &Value::Null
        } else {
            return Err(invalid());
        };
        if &bay["parent_id"] != expected_parent {
            return Err(invalid());
        }
        let mut seen = HashSet::from([*id]);
        let mut parent = expected_parent.as_str();
        while let Some(parent_id) = parent {
            if !seen.insert(parent_id) {
                return Err(AppError::Validation(
                    "Archived module graph contains a cycle".into(),
                ));
            }
            parent = bays.get(parent_id).ok_or_else(invalid)?["parent_id"].as_str();
        }
    }
    Ok(())
}

async fn insert_full_backup_rows(
    tx: &mut Transaction<'_, Postgres>,
    table: &FullBackupTable,
    rows: Value,
) -> Result<(), AppError> {
    if table.rows.is_empty() {
        return Ok(());
    }
    let columns = table
        .columns
        .iter()
        .map(|column| quote_ident(&column.name))
        .collect::<Vec<_>>()
        .join(",");
    let target = quote_table(&table.name);
    let statement = format!(
        "INSERT INTO {target} ({columns}) OVERRIDING SYSTEM VALUE SELECT {columns} FROM jsonb_populate_recordset(NULL::{target},$1)"
    );
    sqlx::query(&statement)
        .bind(rows)
        .execute(&mut **tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    Ok(())
}

async fn restore_archived_module_graph(
    tx: &mut Transaction<'_, Postgres>,
    mut tables: HashMap<String, FullBackupTable>,
) -> Result<usize, AppError> {
    let mut count = 0;
    let bays = tables.remove("dcim_modulebay");
    if let Some(table) = &bays {
        let staged = staged_module_bay_rows(&table.rows);
        insert_full_backup_rows(tx, table, Value::Array(staged)).await?;
        count += table.rows.len();
    }
    if let Some(table) = tables.remove("dcim_module") {
        // Bays now exist and are temporarily enabled. Bay categories are not
        // restored until after installation; the live guard is still executed.
        insert_full_backup_rows(tx, &table, Value::Array(table.rows.clone())).await?;
        count += table.rows.len();
    }
    if let Some(table) = bays {
        // Keep temporary names until every bay has its final containing module,
        // avoiding false root-name collisions between separate nested modules.
        sqlx::query("UPDATE dcim_modulebay target SET module_id=source.module_id,parent_id=source.parent_id FROM jsonb_populate_recordset(NULL::dcim_modulebay,$1) source WHERE target.id=source.id")
            .bind(Value::Array(table.rows.clone())).execute(&mut **tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
        // Staging/containment writes rebuild the native tree projection. Restore
        // the archive's bookkeeping only after those internal mutations: valid
        // sibling order need not equal the current writer's UUID-path order.
        // Schema checks and final containment/cycle validation still apply.
        sqlx::query("UPDATE dcim_modulebay target SET name=source.name,enabled=source.enabled,tree_id=source.tree_id,level=source.level,lft=source.lft,rght=source.rght FROM jsonb_populate_recordset(NULL::dcim_modulebay,$1) source WHERE target.id=source.id")
            .bind(Value::Array(table.rows)).execute(&mut **tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
    }
    sqlx::query("SELECT dcim_validate_module_graph(id) FROM dcim_device")
        .execute(&mut **tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    Ok(count)
}

fn staged_module_bay_rows(rows: &[Value]) -> Vec<Value> {
    let mut staged = rows.to_vec();
    let mut reserved_names = rows
        .iter()
        .filter_map(|row| row["name"].as_str().map(str::to_owned))
        .collect::<HashSet<_>>();
    for row in &mut staged {
        let name = loop {
            let candidate = format!("restore-{}", uuid::Uuid::now_v7());
            if reserved_names.insert(candidate.clone()) {
                break candidate;
            }
        };
        row["name"] = Value::String(name);
        row["module_id"] = Value::Null;
        row["parent_id"] = Value::Null;
        row["enabled"] = Value::Bool(true);
    }
    staged
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

/// Scope is an attachment dependency even though memberships use UUID arrays
/// rather than foreign keys. Keep all other dependency ordering stable.
fn tag_catalog_replay_order(table_name: &str) -> u8 {
    match table_name {
        "inventory_tag" | "inventory_content_type" => 0,
        "inventory_tag_object_type" => 1,
        _ => 2,
    }
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
        || name == "inventory_tag"
        || name == "inventory_tag_object_type"
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

fn guarded_restore_table(table: &str) -> Option<&'static str> {
    match table {
        "ipam_vlantranslationpolicy" => Some("ipam_vlantranslationpolicy"),
        "ipam_vlantranslationrule" => Some("ipam_vlantranslationrule"),
        "circuits_virtualcircuittermination" => Some("circuits_virtualcircuittermination"),
        "circuits_virtualcircuit" => Some("circuits_virtualcircuit"),
        "vpn_l2vpn" => Some("vpn_l2vpn"),
        "vpn_l2vpntermination" => Some("vpn_l2vpntermination"),
        "tenancy_contactassignment" => Some("tenancy_contactassignment"),
        "dcim_interface" => Some("dcim_interface"),
        "virtualization_vminterface" => Some("virtualization_vminterface"),
        "ipam_vlan" => Some("ipam_vlan"),
        "ipam_routetarget" => Some("ipam_routetarget"),
        "dcim_device" => Some("dcim_device"),
        "virtualization_virtualmachine" => Some("virtualization_virtualmachine"),
        _ => None,
    }
}

async fn missing_native_row_ids(
    tx: &mut Transaction<'_, Postgres>,
    table: &FullBackupTable,
    primary_key_columns: &[String],
    native_table: &'static str,
) -> Result<Vec<uuid::Uuid>, AppError> {
    validate_primary_key_columns(table, primary_key_columns)?;
    let predicates = primary_key_columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            format!("target.{quoted} = source.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "SELECT target.id FROM {} AS target WHERE NOT EXISTS (SELECT 1 FROM jsonb_populate_recordset(NULL::{}, $1::jsonb) AS source WHERE {}) ORDER BY target.id",
        quote_table(native_table),
        quote_table(native_table),
        predicates,
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(Value::Array(table.rows.clone()))
        .fetch_all(&mut **tx)
        .await?)
}

async fn delete_missing_rows(
    tx: &mut Transaction<'_, Postgres>,
    table: &FullBackupTable,
    primary_key_columns: &[String],
) -> Result<(), AppError> {
    validate_primary_key_columns(table, primary_key_columns)?;
    let predicates = primary_key_columns
        .iter()
        .map(|column| {
            let quoted = quote_ident(column);
            format!("target.{quoted} = source.{quoted}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    if let Some(native_table) = guarded_restore_table(&table.name) {
        let ids = missing_native_row_ids(tx, table, primary_key_columns, native_table).await?;
        // Native cascades own child secret consumers as well. Keep retirement
        // and blocked-delete handling in the canonical guard and restore tx.
        for id in ids {
            delete_inventory_record_in_transaction(tx, native_table, id).await?;
        }
        return Ok(());
    }
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
    let mut replay_rows = selected.table.rows.clone();
    if matches!(
        selected.table.name.as_str(),
        "dcim_module" | "dcim_devicebay"
    ) {
        // INSERT ... ON CONFLICT still runs BEFORE INSERT installation guards.
        // Existing occupants may legitimately be disabled/incompatible; use a
        // policy-aware UPDATE so native update guards evaluate the actual change.
        let ids = replay_rows
            .iter()
            .map(|row| {
                row["id"]
                    .as_str()
                    .ok_or_else(|| AppError::Validation("Archived module requires an id".into()))
                    .and_then(|id| uuid::Uuid::parse_str(id).map_err(AppError::from))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let existing: Vec<uuid::Uuid> = sqlx::query_scalar(&format!(
            "SELECT id FROM {} WHERE id=ANY($1)",
            quote_table(&selected.table.name)
        ))
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
        let existing_rows = replay_rows
            .iter()
            .filter(|row| {
                existing.iter().any(|id| {
                    row["id"]
                        .as_str()
                        .is_some_and(|value| value == id.to_string())
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !existing_rows.is_empty() {
            let effective = effective_update_columns(selected);
            if !effective.is_empty() {
                let assignments = effective
                    .iter()
                    .map(|column| {
                        format!(
                            "{} = {}",
                            quote_ident(column),
                            assignment_expression("target", "source", column, selected)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let changed = changed_predicate("target", "source", &effective, selected);
                let statement = format!(
                    "UPDATE {table} target SET {assignments} FROM jsonb_populate_recordset(NULL::{table},$1) source WHERE target.id=source.id AND ({changed})",
                    table = quote_table(&selected.table.name)
                );
                sqlx::query(&statement)
                    .bind(Value::Array(existing_rows.clone()))
                    .execute(&mut **tx)
                    .await
                    .map_err(crate::errors::classify_sqlx_error_owned)?;
            }
            let existing_ids = existing_rows
                .iter()
                .map(|row| row["id"].clone())
                .collect::<Vec<_>>();
            replay_rows.retain(|row| !existing_ids.contains(&row["id"]));
        }
    }
    let rows = Value::Array(replay_rows);
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
