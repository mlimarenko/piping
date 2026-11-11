//! Guarded DELETE for any entity whose removal can be blocked by inbound
//! foreign keys.
//!
//! Postgres reports a blocked delete as a bare FK violation, which the generic
//! error classifier can only render as a 400 "Referenced resource does not
//! exist or cannot be used here" — actively wrong for a delete (the record
//! exists; it is *in use*). This module turns that failure into
//! [`AppError::DeleteBlocked`] with structured details: which tables still
//! reference the record, how many rows in each, and up to three labeled
//! examples. The REST layer maps it to `409 DELETE_BLOCKED` and the frontend
//! renders the blockers.
//!
//! The engine is context-agnostic: inbound references are discovered from the
//! catalog at runtime, and labels are derived from the referencing table's own
//! columns. Context-specific niceties (parent joins for "port @ device"
//! labels, UI entity keys for links) come from the caller through
//! [`BlockerMetadataResolver`]; contexts without that metadata pass
//! [`no_blocker_metadata`].
//!
//! Only constraints that actually block a delete (`NO ACTION`, `RESTRICT`)
//! are treated as blockers. `SET NULL` / `SET DEFAULT` / `CASCADE` references
//! resolve themselves and must not appear in the report — a proxy node held
//! by one assignment must not list its (self-detaching) sessions as blockers.

use std::collections::{BTreeSet, HashMap};

use sqlx::{Acquire, PgConnection, PgPool, Postgres, Row, Transaction};
use tracing::warn;
use uuid::Uuid;

use crate::errors::app_error::{DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup};
use crate::errors::{AppError, classify_sqlx_error_owned};

const MAX_BLOCKER_EXAMPLES: i64 = 3;

/// Optional per-table display metadata supplied by the calling context.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockerTableMetadata {
    /// Join to a parent for richer labels ("eth0 @ core-sw-1").
    pub parent_join: Option<BlockerParentJoin>,
    /// Stable UI key so the frontend can deep-link the blocker group.
    pub entity_key: Option<&'static str>,
    /// A relationship row represents this owning resource, not itself.
    pub owner_join: Option<BlockerParentJoin>,
}

#[derive(Debug, Clone, Copy)]
pub struct BlockerParentJoin {
    pub fk: &'static str,
    pub table: &'static str,
}

/// Maps a referencing table name to its display metadata.
pub type BlockerMetadataResolver = fn(&str) -> BlockerTableMetadata;

/// Resolver for contexts with no display metadata: plain labels, no links.
pub fn no_blocker_metadata(_table: &str) -> BlockerTableMetadata {
    BlockerTableMetadata::default()
}

/// Delete one row by id, translating an FK-blocked delete into
/// [`AppError::DeleteBlocked`]. Returns the number of rows deleted so the
/// caller can distinguish "deleted" from "was not there" (404).
///
/// # Errors
///
/// [`AppError::DeleteBlocked`] when inbound `NO ACTION`/`RESTRICT` references
/// hold the row; otherwise the classified database error.
pub async fn delete_record_guarded(
    pool: &PgPool,
    target_table: &'static str,
    id: Uuid,
    resolver: BlockerMetadataResolver,
) -> Result<u64, AppError> {
    let mut connection = pool.acquire().await.map_err(classify_sqlx_error_owned)?;
    let mut query = sqlx::QueryBuilder::<Postgres>::new("DELETE FROM ");
    query.push(target_table).push(" WHERE id = $1");
    match query.build().bind(id).execute(&mut *connection).await {
        Ok(result) => Ok(result.rows_affected()),
        Err(error) => {
            Err(translate_delete_error(&mut connection, target_table, id, resolver, error).await)
        }
    }
}

/// Transactional form of [`delete_record_guarded`].
///
/// The DELETE runs in a savepoint because PostgreSQL aborts the current
/// transaction after an FK violation. Rolling back only the savepoint makes it
/// safe to inspect blockers while preserving the caller's outer transaction;
/// the caller can then return [`AppError::DeleteBlocked`] and roll back all
/// preparatory writes atomically.
///
/// # Errors
///
/// [`AppError::DeleteBlocked`] when inbound `NO ACTION`/`RESTRICT` references
/// hold the row; otherwise the classified database error.
pub async fn delete_record_guarded_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    target_table: &'static str,
    id: Uuid,
    resolver: BlockerMetadataResolver,
) -> Result<u64, AppError> {
    let mut savepoint = transaction
        .begin()
        .await
        .map_err(classify_sqlx_error_owned)?;
    let mut query = sqlx::QueryBuilder::<Postgres>::new("DELETE FROM ");
    query.push(target_table).push(" WHERE id = $1");
    let deletion = query.build().bind(id).execute(&mut *savepoint).await;
    let deletion = match deletion {
        Ok(result) => {
            // Native inventory FKs are initially deferred. Check them inside
            // this savepoint so a blocker is translated before outer commit,
            // and all preparatory secret writes can still roll back together.
            match validate_deferred_delete_constraints(&mut savepoint, target_table).await {
                Ok(()) => Ok(result),
                Err(DeferredConstraintFailure::Identifier(error)) => {
                    savepoint
                        .rollback()
                        .await
                        .map_err(classify_sqlx_error_owned)?;
                    return Err(error);
                }
                Err(DeferredConstraintFailure::Database(error)) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    match deletion {
        Ok(result) => {
            let rows_affected = result.rows_affected();
            savepoint
                .commit()
                .await
                .map_err(classify_sqlx_error_owned)?;
            Ok(rows_affected)
        }
        Err(error) => {
            savepoint
                .rollback()
                .await
                .map_err(classify_sqlx_error_owned)?;
            Err(translate_delete_error(transaction, target_table, id, resolver, error).await)
        }
    }
}

/// One inbound foreign key of a parent table, with the delete action PostgreSQL
/// will actually apply and whether the check can be deferred.
///
/// Reference discovery is catalog-derived, so a new constraint needs no list
/// update. An effect whose shape cannot be evaluated (a composite reference, an
/// unknown action) is reported as such instead of being assumed harmless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundDeleteEffect {
    pub constraint: String,
    pub child_table: String,
    /// `None` when this is not a single-column reference to the parent's `id`.
    pub child_column: Option<String>,
    pub action: DeleteAction,
    pub deferrable: bool,
    pub initially_deferred: bool,
}

/// What PostgreSQL does to an inbound reference when the parent row is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl DeleteAction {
    fn from_code(code: &str) -> Option<Self> {
        match code {
            "a" => Some(Self::NoAction),
            "r" => Some(Self::Restrict),
            "c" => Some(Self::Cascade),
            "n" => Some(Self::SetNull),
            "d" => Some(Self::SetDefault),
            _ => None,
        }
    }

    /// Whether the child row survives the parent delete by being rewritten.
    #[must_use]
    pub fn rewrites_child(self) -> bool {
        matches!(self, Self::SetNull | Self::SetDefault)
    }

    /// Whether PostgreSQL removes the child row with the parent.
    #[must_use]
    pub fn removes_child(self) -> bool {
        matches!(self, Self::Cascade)
    }
}

/// Read every inbound foreign key of `parent_table` from the catalog.
///
/// # Errors
///
/// [`AppError::Database`] when the catalog cannot be read; an unknown delete
/// action is reported as an unevaluable effect rather than dropped.
pub async fn inbound_delete_effects(
    connection: &mut PgConnection,
    parent_table: &str,
) -> Result<Vec<InboundDeleteEffect>, AppError> {
    validate_identifier(parent_table)?;
    let rows = sqlx::query(
        r#"
        SELECT con.conname AS constraint_name,
               child.relname AS child_table,
               con.confdeltype::text AS delete_action,
               con.condeferrable,
               con.condeferred,
               array_length(con.conkey, 1) AS key_columns,
               child_col.attname AS child_column,
               parent_col.attname AS parent_column
        FROM pg_constraint con
        JOIN pg_class child ON child.oid = con.conrelid
        JOIN pg_namespace child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_class parent ON parent.oid = con.confrelid
        JOIN pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace
        JOIN unnest(con.conkey) WITH ORDINALITY AS child_keys(attnum, ord) ON TRUE
        JOIN unnest(con.confkey) WITH ORDINALITY AS parent_keys(attnum, ord)
            ON parent_keys.ord = child_keys.ord
        JOIN pg_attribute child_col
            ON child_col.attrelid = con.conrelid AND child_col.attnum = child_keys.attnum
        JOIN pg_attribute parent_col
            ON parent_col.attrelid = con.confrelid AND parent_col.attnum = parent_keys.attnum
        WHERE con.contype = 'f'
          AND child_ns.nspname = 'public'
          AND parent_ns.nspname = 'public'
          AND parent.relname = $1
        ORDER BY child.relname, con.conname, child_col.attname
        "#,
    )
    .bind(parent_table)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to inspect inbound delete effects for {parent_table}: {error}"
        ))
    })?;

    let mut effects: Vec<InboundDeleteEffect> = Vec::with_capacity(rows.len());
    for row in rows {
        let constraint: String = row.try_get("constraint_name")?;
        let child_table: String = row.try_get("child_table")?;
        let action_code: String = row.try_get("delete_action")?;
        let key_columns: i32 = row.try_get("key_columns")?;
        let child_column: Option<String> = row.try_get("child_column").ok();
        let parent_column: String = row.try_get("parent_column")?;
        validate_identifier(&child_table)?;
        if let Some(column) = child_column.as_deref() {
            validate_identifier(column)?;
        }
        let child_column = child_column.filter(|_| key_columns == 1 && parent_column == "id");
        let Some(action) = DeleteAction::from_code(&action_code) else {
            // Fail closed: an effect form this build cannot classify is reported
            // with an unevaluable column and a conservative action.
            effects.push(InboundDeleteEffect {
                constraint,
                child_table,
                child_column: None,
                action: DeleteAction::Restrict,
                deferrable: row.try_get("condeferrable")?,
                initially_deferred: row.try_get("condeferred")?,
            });
            continue;
        };
        let effect = InboundDeleteEffect {
            constraint,
            child_table,
            child_column,
            action,
            deferrable: row.try_get("condeferrable")?,
            initially_deferred: row.try_get("condeferred")?,
        };
        match effects.iter_mut().find(|existing| {
            existing.constraint == effect.constraint && existing.child_table == effect.child_table
        }) {
            Some(existing) => {
                // A multi-column constraint reaches here once per key column; it
                // is evaluable only through its single-column form.
                if existing.child_column.is_some() && effect.child_column.is_some() {
                    existing.child_column = None;
                }
            }
            None => effects.push(effect),
        }
    }
    effects.sort_by(|left, right| {
        (left.child_table.as_str(), left.constraint.as_str())
            .cmp(&(right.child_table.as_str(), right.constraint.as_str()))
    });
    Ok(effects)
}

/// Delete a set of rows children-first in one savepoint and validate deferred
/// constraints only after the complete set.
///
/// A per-row check cannot distinguish "this child is a retained dependent" from
/// "this child is part of the same approved removal set", and it leaves the
/// caller without an all-or-nothing outcome. The savepoint covers every delete,
/// so a late failure rolls all native effects of the set back together. No
/// constraint is disabled and no failure is downgraded.
///
/// # Errors
///
/// [`AppError::DeleteBlocked`] when a retained row still references one of the
/// targets after the whole set; otherwise the classified database error.
pub async fn delete_records_guarded_in_savepoint(
    transaction: &mut Transaction<'_, Postgres>,
    deletions: &[(String, Uuid)],
    resolver: BlockerMetadataResolver,
) -> Result<u64, AppError> {
    if deletions.is_empty() {
        return Ok(0);
    }
    let mut tables: Vec<String> = Vec::new();
    for (table, _) in deletions {
        validate_identifier(table)?;
        if !tables.iter().any(|known| known == table) {
            tables.push(table.clone());
        }
    }
    let mut savepoint = transaction
        .begin()
        .await
        .map_err(classify_sqlx_error_owned)?;
    let mut deleted = 0_u64;
    for (table, id) in deletions {
        let mut query = sqlx::QueryBuilder::<Postgres>::new("DELETE FROM ");
        query.push(table.as_str()).push(" WHERE id = $1");
        match query.build().bind(id).execute(&mut *savepoint).await {
            Ok(result) => deleted += result.rows_affected(),
            Err(error) => {
                savepoint
                    .rollback()
                    .await
                    .map_err(classify_sqlx_error_owned)?;
                return Err(
                    translate_bulk_delete_error(transaction, deletions, resolver, error).await,
                );
            }
        }
    }
    let parent_tables: Vec<&str> = tables.iter().map(String::as_str).collect();
    if let Err(error) =
        validate_deferred_delete_constraints_for(&mut savepoint, &parent_tables).await
    {
        savepoint
            .rollback()
            .await
            .map_err(classify_sqlx_error_owned)?;
        return Err(match error {
            DeferredConstraintFailure::Identifier(error) => error,
            DeferredConstraintFailure::Database(error) => {
                translate_bulk_delete_error(transaction, deletions, resolver, error).await
            }
        });
    }
    savepoint
        .commit()
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(deleted)
}

async fn translate_bulk_delete_error(
    connection: &mut PgConnection,
    deletions: &[(String, Uuid)],
    resolver: BlockerMetadataResolver,
    error: sqlx::Error,
) -> AppError {
    if !is_foreign_key_violation(&error) {
        return classify_sqlx_error_owned(error);
    }
    let mut tables: Vec<&str> = Vec::new();
    for (table, _) in deletions {
        if !tables.contains(&table.as_str()) {
            tables.push(table.as_str());
        }
    }
    for table in tables {
        let ids: Vec<Uuid> = deletions
            .iter()
            .filter(|(candidate, _)| candidate == table)
            .map(|(_, id)| *id)
            .collect();
        for id in ids {
            if let Ok(details) = load_delete_blockers(connection, table, id, resolver).await {
                if details.total_count > 0 {
                    return AppError::DeleteBlocked(details);
                }
            }
        }
    }
    classify_sqlx_error_owned(error)
}

async fn validate_deferred_delete_constraints(
    connection: &mut PgConnection,
    target_table: &str,
) -> Result<(), DeferredConstraintFailure> {
    validate_deferred_delete_constraints_for(connection, &[target_table]).await
}

/// Failure of one deferred-constraint pass.
///
/// PostgreSQL reports a still-retained reference here as a database error the
/// caller translates into structured blockers, while a catalog identifier that
/// [`validate_identifier`] refuses is a typed refusal that reaches the caller
/// unchanged instead of being reported as a database failure.
enum DeferredConstraintFailure {
    Identifier(AppError),
    Database(sqlx::Error),
}

/// Force every deferred inbound check of the given parent tables to run now.
///
/// The bulk executor calls this once, after the whole set is deleted, so an
/// accepted removal set is validated as a set rather than row by row.
async fn validate_deferred_delete_constraints_for(
    connection: &mut PgConnection,
    target_tables: &[&str],
) -> Result<(), DeferredConstraintFailure> {
    let mut names: Vec<String> = Vec::new();
    for target_table in target_tables {
        let constraints = sqlx::query("SELECT c.conname::text AS name FROM pg_constraint c WHERE c.confrelid = to_regclass($1) AND c.contype = 'f' AND c.confdeltype IN ('a', 'r') AND c.condeferrable AND c.condeferred ORDER BY c.conname")
            .bind(*target_table)
            .fetch_all(&mut *connection)
            .await
            .map_err(DeferredConstraintFailure::Database)?;
        for row in constraints {
            let name: String = row
                .try_get("name")
                .map_err(DeferredConstraintFailure::Database)?;
            validate_identifier(&name).map_err(DeferredConstraintFailure::Identifier)?;
            let qualified = format!("public.\"{name}\"");
            if !names.contains(&qualified) {
                names.push(qualified);
            }
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    // Constraint identifiers come from PostgreSQL's catalog, not a request.
    let names = names.join(", ");
    let mut immediate = sqlx::QueryBuilder::<Postgres>::new("SET CONSTRAINTS ");
    immediate.push(&names).push(" IMMEDIATE");
    immediate
        .build()
        .execute(&mut *connection)
        .await
        .map_err(DeferredConstraintFailure::Database)?;
    let mut deferred = sqlx::QueryBuilder::<Postgres>::new("SET CONSTRAINTS ");
    deferred.push(&names).push(" DEFERRED");
    deferred
        .build()
        .execute(connection)
        .await
        .map_err(DeferredConstraintFailure::Database)?;
    Ok(())
}

async fn translate_delete_error(
    connection: &mut PgConnection,
    target_table: &str,
    id: Uuid,
    resolver: BlockerMetadataResolver,
    error: sqlx::Error,
) -> AppError {
    if !is_foreign_key_violation(&error) {
        return classify_sqlx_error_owned(error);
    }

    match load_delete_blockers(connection, target_table, id, resolver).await {
        Ok(details) if details.total_count > 0 => AppError::DeleteBlocked(details),
        Ok(_) => classify_sqlx_error_owned(error),
        Err(load_error) => load_error,
    }
}

async fn load_delete_blockers(
    connection: &mut PgConnection,
    target_table: &str,
    target_id: Uuid,
    resolver: BlockerMetadataResolver,
) -> Result<DeleteBlockedDetails, AppError> {
    let inbound_refs = load_inbound_foreign_keys(connection, target_table).await?;
    let mut blockers = Vec::new();
    let mut total_count = 0_i64;

    for reference in inbound_refs {
        if let Some(blocker) =
            load_blocker_group(connection, &reference, target_id, resolver).await?
        {
            total_count += blocker.count;
            blockers.push(blocker);
        }
    }

    blockers.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.table.cmp(&right.table))
    });

    Ok(DeleteBlockedDetails {
        total_count,
        blockers,
    })
}

#[derive(Debug)]
struct ForeignKeyReference {
    table: String,
    columns: Vec<String>,
}

#[derive(Debug)]
struct DisplayPlan {
    label_expr: String,
    id_expr: &'static str,
    has_id: bool,
}

async fn load_inbound_foreign_keys(
    connection: &mut PgConnection,
    target_table: &str,
) -> Result<Vec<ForeignKeyReference>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT child.relname AS table_name, child_col.attname AS column_name
        FROM pg_constraint con
        JOIN pg_class child ON child.oid = con.conrelid
        JOIN pg_namespace child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_class parent ON parent.oid = con.confrelid
        JOIN pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace
        JOIN unnest(con.conkey) WITH ORDINALITY AS child_keys(attnum, ord) ON TRUE
        JOIN unnest(con.confkey) WITH ORDINALITY AS parent_keys(attnum, ord)
            ON parent_keys.ord = child_keys.ord
        JOIN pg_attribute child_col
            ON child_col.attrelid = con.conrelid AND child_col.attnum = child_keys.attnum
        JOIN pg_attribute parent_col
            ON parent_col.attrelid = con.confrelid AND parent_col.attnum = parent_keys.attnum
        WHERE con.contype = 'f'
          -- Only delete-blocking actions. SET NULL / SET DEFAULT / CASCADE
          -- references resolve themselves and are not blockers.
          AND con.confdeltype IN ('a', 'r')
          AND child_ns.nspname = 'public'
          AND parent_ns.nspname = 'public'
          AND parent.relname = $1
          AND parent_col.attname = 'id'
        ORDER BY child.relname, child_col.attname
        "#,
    )
    .bind(target_table)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to inspect inbound foreign keys for {target_table}: {error}"
        ))
    })?;

    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let table: String = row.try_get("table_name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read inbound foreign-key table for {target_table}: {error}"
            ))
        })?;
        let column: String = row.try_get("column_name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read inbound foreign-key column for {target_table}: {error}"
            ))
        })?;
        validate_identifier(&table)?;
        validate_identifier(&column)?;
        grouped.entry(table).or_default().push(column);
    }

    let mut references = grouped
        .into_iter()
        .map(|(table, mut columns)| {
            columns.sort();
            columns.dedup();
            ForeignKeyReference { table, columns }
        })
        .collect::<Vec<_>>();
    references.sort_by(|left, right| left.table.cmp(&right.table));
    Ok(references)
}

async fn load_blocker_group(
    connection: &mut PgConnection,
    reference: &ForeignKeyReference,
    target_id: Uuid,
    resolver: BlockerMetadataResolver,
) -> Result<Option<DeleteBlockedGroup>, AppError> {
    let metadata = resolver(&reference.table);
    let columns = load_table_columns(connection, &reference.table).await?;
    let (display_plan, join_clause, count_expr, blocker_table) =
        if let Some(owner) = metadata.owner_join {
            validate_identifier(owner.table)?;
            validate_identifier(owner.fk)?;
            if !columns.contains(owner.fk) {
                return Err(AppError::Database(format!(
                    "Missing blocker owner column {}.{}",
                    reference.table, owner.fk
                )));
            }
            let owner_columns = load_table_columns(connection, owner.table).await?;
            let mut plan = build_display_plan(owner.table, &owner_columns, None);
            plan.label_expr = plan.label_expr.replace("t.", "p.");
            plan.id_expr = "p.id AS id";
            plan.has_id = true;
            (
                plan,
                format!("LEFT JOIN {} p ON t.{} = p.id", owner.table, owner.fk),
                format!("COUNT(DISTINCT t.{})", owner.fk),
                owner.table.to_string(),
            )
        } else {
            let plan = build_display_plan(&reference.table, &columns, metadata.parent_join);
            let count = if plan.has_id {
                "COUNT(DISTINCT t.id)"
            } else {
                "COUNT(*)"
            };
            let join = parent_join_clause(metadata.parent_join, &columns);
            (plan, join, count.to_string(), reference.table.clone())
        };
    let where_clause = reference
        .columns
        .iter()
        .map(|column| format!("t.{column} = $1"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut count_query = sqlx::QueryBuilder::<Postgres>::new("SELECT ");
    count_query
        .push(&count_expr)
        .push(" AS blocker_count FROM ")
        .push(&reference.table)
        .push(" t WHERE ")
        .push(&where_clause);
    let count_row = count_query
        .build()
        .bind(target_id)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to count blockers in {}: {error}",
                reference.table
            ))
        })?;
    let count: i64 = count_row.try_get("blocker_count").map_err(|error| {
        AppError::Database(format!(
            "Failed to read blocker count in {}: {error}",
            reference.table
        ))
    })?;
    if count == 0 {
        return Ok(None);
    }

    let mut examples_query =
        sqlx::QueryBuilder::<Postgres>::new("SELECT id, label FROM ( SELECT DISTINCT ");
    examples_query
        .push(display_plan.id_expr)
        .push(", ")
        .push(&display_plan.label_expr)
        .push(" AS label FROM ")
        .push(&reference.table)
        .push(" t ")
        .push(&join_clause)
        .push(" WHERE ")
        .push(&where_clause)
        .push(" ) blocker_examples ORDER BY label, ")
        .push(if display_plan.has_id { "id" } else { "label" })
        .push(" LIMIT ")
        .push(MAX_BLOCKER_EXAMPLES);
    let example_rows = examples_query
        .build()
        .bind(target_id)
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            warn!(
                table = %reference.table,
                ?target_id,
                error = %error,
                "failed to load delete blocker examples; returning blocker counts without examples"
            );
            error
        })
        .ok()
        .unwrap_or_default();

    let examples = example_rows
        .into_iter()
        .filter_map(|row| {
            let label = match row.try_get::<String, _>("label") {
                Ok(label) => label,
                Err(error) => {
                    warn!(
                        table = %reference.table,
                        ?target_id,
                        error = %error,
                        "failed to read delete blocker label; skipping malformed blocker example row"
                    );
                    return None;
                }
            };
            let id = row.try_get::<Option<Uuid>, _>("id").ok().flatten();
            Some(DeleteBlockedExample { id, label })
        })
        .collect::<Vec<_>>();

    Ok(Some(DeleteBlockedGroup {
        table: blocker_table,
        entity_key: metadata.entity_key.map(ToString::to_string),
        count,
        examples,
    }))
}

async fn load_table_columns(
    connection: &mut PgConnection,
    table: &str,
) -> Result<BTreeSet<String>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT column_name
        FROM information_schema.columns
        WHERE table_schema = 'public' AND table_name = $1
        ORDER BY ordinal_position
        "#,
    )
    .bind(table)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to inspect columns for {table}: {error}"))
    })?;

    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("column_name").ok())
        .collect())
}

/// Column preference for the human-readable label of a blocking row, best
/// first.
///
/// This is a lookup, not logic: a table may carry several of these (a device
/// has both `name` and `serial`), and the first match wins — so the ORDER here
/// is the product decision about what an operator should see, and the only
/// thing worth reviewing.
const DISPLAY_COLUMNS: &[(&str, &str)] = &[
    ("name", "NULLIF(t.name, '')"),
    ("model", "NULLIF(t.model, '')"),
    ("label", "NULLIF(t.label, '')"),
    ("prefix", "NULLIF(t.prefix::text, '')"),
    ("address", "NULLIF(t.address::text, '')"),
    ("rd", "NULLIF(t.rd, '')"),
    ("slug", "NULLIF(t.slug, '')"),
    ("facility", "NULLIF(t.facility, '')"),
    ("asn", "NULLIF(t.asn::text, '')"),
    ("vid", "NULLIF(t.vid::text, '')"),
    ("serial", "NULLIF(t.serial, '')"),
];

fn single_column_display_expr(columns: &BTreeSet<String>) -> Option<&'static str> {
    DISPLAY_COLUMNS
        .iter()
        .find(|(column, _)| columns.contains(*column))
        .map(|(_, expr)| *expr)
}

fn build_display_plan(
    table: &str,
    columns: &BTreeSet<String>,
    parent_join: Option<BlockerParentJoin>,
) -> DisplayPlan {
    let has_id = columns.contains("id");
    let fallback_expr = if has_id {
        "t.id::text".to_string()
    } else {
        format!("'{}'", table)
    };

    let base_expr = single_column_display_expr(columns)
        .map(str::to_string)
        .or_else(|| {
            // The only pair: a range reads as "start - end", so it cannot come
            // from the single-column table above.
            (columns.contains("start_address") && columns.contains("end_address")).then(|| {
                "NULLIF(concat_ws(' - ', t.start_address::text, t.end_address::text), '')"
                    .to_string()
            })
        })
        .unwrap_or_else(|| fallback_expr.clone());

    let label_expr = match parent_join {
        Some(parent) if columns.contains(parent.fk) => format!(
            "COALESCE(CASE \
                WHEN p.name IS NULL THEN {base_expr} \
                WHEN {base_expr} IS NULL THEN p.name \
                ELSE {base_expr} || ' @ ' || p.name \
            END, {fallback_expr})"
        ),
        _ => format!("COALESCE({base_expr}, {fallback_expr})"),
    };

    let id_expr = if has_id {
        "t.id AS id"
    } else {
        "NULL::uuid AS id"
    };

    DisplayPlan {
        label_expr,
        id_expr,
        has_id,
    }
}

fn parent_join_clause(
    parent_join: Option<BlockerParentJoin>,
    columns: &BTreeSet<String>,
) -> String {
    match parent_join {
        Some(parent) if columns.contains(parent.fk) => {
            format!("LEFT JOIN {} p ON t.{} = p.id", parent.table, parent.fk)
        }
        _ => String::new(),
    }
}

fn is_foreign_key_violation(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(database) => {
            database.kind() == sqlx::error::ErrorKind::ForeignKeyViolation
                // Postgres reports an FK violation as 23503 for NO ACTION but
                // as 23001 (restrict_violation) for ON DELETE RESTRICT — and
                // sqlx's ErrorKind only covers the former. Without 23001 a
                // RESTRICT-blocked delete skips the guard entirely.
                || matches!(database.code().as_deref(), Some("23503" | "23001"))
        }
        _ => false,
    }
}

fn validate_identifier(identifier: &str) -> Result<(), AppError> {
    if identifier
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        Ok(())
    } else {
        Err(AppError::Database(format!(
            "Unsafe SQL identifier discovered during delete guard: {identifier}"
        )))
    }
}

#[cfg(test)]
mod display_column_tests {
    use super::{DISPLAY_COLUMNS, single_column_display_expr};
    use std::collections::BTreeSet;

    fn columns(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn a_row_with_several_candidates_is_labelled_by_the_best_one() {
        // A device carries name, slug and serial. The operator is being told
        // WHICH row blocks their delete, so it has to be the name — falling
        // back to a serial number when a name exists would be a worse answer,
        // not merely a different one.
        assert_eq!(
            single_column_display_expr(&columns(&["serial", "slug", "name"])),
            Some("NULLIF(t.name, '')"),
        );
        assert_eq!(
            single_column_display_expr(&columns(&["serial", "slug"])),
            Some("NULLIF(t.slug, '')"),
        );
    }

    #[test]
    fn a_row_with_no_candidate_column_has_no_expression() {
        // The caller then falls back to the id or the table name — this
        // function must not invent one.
        assert_eq!(single_column_display_expr(&columns(&["created_at"])), None);
        assert_eq!(single_column_display_expr(&BTreeSet::new()), None);
    }

    #[test]
    fn every_candidate_reads_its_own_column() {
        // Guards the copy-paste hazard in a table of near-identical SQL: an
        // entry whose expression names a different column than its key would
        // silently label rows with the wrong field.
        for (column, expr) in DISPLAY_COLUMNS {
            assert!(
                expr.contains(&format!("t.{column}")),
                "entry {column:?} renders {expr:?}, which does not read t.{column}",
            );
        }
    }
}
