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
    let statement = format!("DELETE FROM {target_table} WHERE id = $1");
    match sqlx::query(statement.as_str())
        .bind(id)
        .execute(&mut *connection)
        .await
    {
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
    let statement = format!("DELETE FROM {target_table} WHERE id = $1");
    let deletion = sqlx::query(statement.as_str())
        .bind(id)
        .execute(&mut *savepoint)
        .await;
    let deletion = match deletion {
        Ok(result) => {
            // Native inventory FKs are initially deferred. Check them inside
            // this savepoint so a blocker is translated before outer commit,
            // and all preparatory secret writes can still roll back together.
            match validate_deferred_delete_constraints(&mut savepoint, target_table).await {
                Ok(_) => Ok(result),
                Err(error) => Err(error),
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

async fn validate_deferred_delete_constraints(
    connection: &mut PgConnection,
    target_table: &str,
) -> Result<(), sqlx::Error> {
    let constraints = sqlx::query!("SELECT c.conname::text AS \"name!\" FROM pg_constraint c WHERE c.confrelid = to_regclass($1) AND c.contype = 'f' AND c.confdeltype IN ('a', 'r') AND c.condeferrable AND c.condeferred ORDER BY c.conname", target_table)
        .fetch_all(&mut *connection).await?;
    if constraints.is_empty() {
        return Ok(());
    }
    // Constraint identifiers come from PostgreSQL's catalog, not a request.
    let names = constraints
        .iter()
        .map(|row| format!("public.\"{}\"", row.name.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(&format!("SET CONSTRAINTS {names} IMMEDIATE"))
        .execute(&mut *connection)
        .await?;
    sqlx::query(&format!("SET CONSTRAINTS {names} DEFERRED"))
        .execute(connection)
        .await?;
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
    let display_plan = build_display_plan(&reference.table, &columns, metadata.parent_join);
    let where_clause = reference
        .columns
        .iter()
        .map(|column| format!("t.{column} = $1"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let count_expr = if display_plan.has_id {
        "COUNT(DISTINCT t.id)"
    } else {
        "COUNT(*)"
    };

    let count_sql = format!(
        "SELECT {count_expr} AS blocker_count FROM {table} t WHERE {where_clause}",
        table = reference.table,
    );
    let count_row = sqlx::query(count_sql.as_str())
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

    let examples_sql = format!(
        "SELECT id, label \
         FROM ( \
           SELECT DISTINCT {id_expr}, {label_expr} AS label \
           FROM {table} t \
           {join_clause} \
           WHERE {where_clause} \
         ) blocker_examples \
         ORDER BY label, {fallback_order} \
         LIMIT {limit}",
        id_expr = display_plan.id_expr,
        label_expr = display_plan.label_expr,
        table = reference.table,
        join_clause = parent_join_clause(metadata.parent_join, &columns),
        fallback_order = if display_plan.has_id { "id" } else { "label" },
        limit = MAX_BLOCKER_EXAMPLES,
    );
    let example_rows = sqlx::query(examples_sql.as_str())
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
        table: reference.table.clone(),
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
