//! Batch resolver: `(content_type_id, object_id)` pairs → materialized
//! [`ObjectRef`] projections, with no N+1.
//!
//! Query budget is bounded by the number of *distinct registered models* on a
//! page (≤ the registry size), never by the number of rows: list handlers
//! collect every pair on the page, call [`SqlxObjectRefResolver::resolve`] once,
//! then map each row against the returned map.
//!
//! ## Closed-world dynamic SQL
//!
//! Per-model queries interpolate the target/parent *table* and the parent *fk*
//! column directly into the SQL string. Those identifiers come ONLY from the
//! static `CONTENT_TYPES` registry (`&'static str`), never from user input,
//! and are reached only after a successful registry lookup — so the
//! interpolation is closed-world and injection-free. All UUIDs stay bound via
//! `= ANY($1::uuid[])`.

use std::collections::HashMap;

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::ObjectRef;
use crate::contexts::inventory::domain::content_type::{self, ContentTypeDef};
use crate::errors::AppError;

/// Resolves generic-FK pairs to labelled [`ObjectRef`]s in batch.
#[derive(Clone)]
pub struct SqlxObjectRefResolver {
    pool: PgPool,
}

/// Resolution of a single target row: its label, qualified if the model has a
/// parent join. A missing row produces no entry (the caller maps it to
/// `label: None`).
struct ResolvedLabel {
    label: String,
}

impl SqlxObjectRefResolver {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Resolve a single `(content_type_id, object_id)` pair. Convenience wrapper
    /// over [`Self::resolve`] for get/create/update handlers.
    pub async fn resolve_one(
        &self,
        content_type_id: Uuid,
        object_id: Uuid,
    ) -> Result<ObjectRef, AppError> {
        let map = self.resolve(&[(content_type_id, object_id)]).await?;
        Ok(map
            .get(&(content_type_id, object_id))
            .cloned()
            .unwrap_or(ObjectRef {
                object_type: String::new(),
                object_id,
                label: None,
            }))
    }

    /// Resolve many pairs at once. Returns a map keyed by the input pair.
    ///
    /// - An unknown `content_type_id` (no `inventory_content_type` row) is
    ///   omitted from step 1 and therefore absent from the result; the caller's
    ///   `.get(...).unwrap_or(...)` falls back to a label-less ref.
    /// - A registered content type whose token is not in [`CONTENT_TYPES`]
    ///   yields an `ObjectRef` carrying the row's token with `label: None`
    ///   (never a 500).
    /// - A registered model whose target row is missing (dangling) yields
    ///   `label: None`.
    pub async fn resolve(
        &self,
        pairs: &[(Uuid, Uuid)],
    ) -> Result<HashMap<(Uuid, Uuid), ObjectRef>, AppError> {
        let mut result: HashMap<(Uuid, Uuid), ObjectRef> = HashMap::new();
        if pairs.is_empty() {
            return Ok(result);
        }

        // Step 1: map every distinct content_type_id → its (app_label, model)
        // token in one query.
        let type_ids: Vec<Uuid> = {
            let mut ids: Vec<Uuid> = pairs.iter().map(|(tid, _)| *tid).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };

        let type_rows = sqlx::query(
            "SELECT id, app_label, model FROM inventory_content_type WHERE id = ANY($1::uuid[])",
        )
        .bind(&type_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load content types: {e}")))?;

        // type_id → canonical token string.
        let mut type_token: HashMap<Uuid, String> = HashMap::with_capacity(type_rows.len());
        for row in &type_rows {
            let id: Uuid = row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read content_type id: {e}")))?;
            let app_label: String = row.try_get("app_label").map_err(|e| {
                AppError::Database(format!("Failed to read content_type app_label: {e}"))
            })?;
            let model: String = row.try_get("model").map_err(|e| {
                AppError::Database(format!("Failed to read content_type model: {e}"))
            })?;
            type_token.insert(id, content_type::token(&app_label, &model));
        }

        // Step 2: group object_ids by registered model (def), so each model is
        // queried at most once. Pairs whose type maps to a known token are
        // resolvable; everything else gets a fallback ref below.
        let mut by_def: HashMap<&'static str, (&'static ContentTypeDef, Vec<Uuid>)> =
            HashMap::new();
        for (type_id, object_id) in pairs {
            let Some(token) = type_token.get(type_id) else {
                // Unknown content_type id: no row at all. Leave absent so the
                // caller's fallback applies.
                continue;
            };
            match content_type::by_token(token) {
                Some(def) => {
                    by_def
                        .entry(def.table)
                        .or_insert_with(|| (def, Vec::new()))
                        .1
                        .push(*object_id);
                }
                None => {
                    // Registered in DB but not in the registry: surface the
                    // token, no label, never 500.
                    result.insert(
                        (*type_id, *object_id),
                        ObjectRef {
                            object_type: token.clone(),
                            object_id: *object_id,
                            label: None,
                        },
                    );
                }
            }
        }

        // Step 3: one query per distinct model; build labels.
        // model table → (object_id → label) for rows that exist.
        let mut labels_by_table: HashMap<&'static str, HashMap<Uuid, ResolvedLabel>> =
            HashMap::new();
        for (table, (def, mut object_ids)) in by_def {
            object_ids.sort_unstable();
            object_ids.dedup();
            let labels = self.fetch_labels(def, &object_ids).await?;
            labels_by_table.insert(table, labels);
        }

        // Step 4: assemble the result for every resolvable pair.
        for (type_id, object_id) in pairs {
            let key = (*type_id, *object_id);
            if result.contains_key(&key) {
                continue; // already handled (registered-but-unregistered token).
            }
            let Some(token) = type_token.get(type_id) else {
                continue; // unknown content_type id → caller fallback.
            };
            let Some(def) = content_type::by_token(token) else {
                continue; // handled above.
            };
            let label = labels_by_table
                .get(def.table)
                .and_then(|m| m.get(object_id))
                .map(|resolved| resolved.label.clone());
            result.insert(
                key,
                ObjectRef {
                    object_type: token.clone(),
                    object_id: *object_id,
                    label,
                },
            );
        }

        Ok(result)
    }

    /// Run ONE query for a single model, returning the labels of the rows that
    /// exist. The table/parent identifiers come only from the static registry
    /// `def` (never user input).
    async fn fetch_labels(
        &self,
        def: &ContentTypeDef,
        object_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, ResolvedLabel>, AppError> {
        if object_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let query = match def.parent {
            Some(parent) => format!(
                "SELECT t.id, t.{label}::text AS name, p.name AS parent_name \
                 FROM {table} t \
                 LEFT JOIN {parent_table} p ON t.{fk} = p.id \
                 WHERE t.id = ANY($1::uuid[])",
                table = def.table,
                label = def.label_column(),
                parent_table = parent.table,
                fk = parent.fk,
            ),
            None => format!(
                "SELECT t.id, t.{label}::text AS name FROM {table} t WHERE t.id = ANY($1::uuid[])",
                table = def.table,
                label = def.label_column(),
            ),
        };

        let rows = sqlx::query(&query)
            .bind(object_ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to resolve labels for {}: {e}", def.table))
            })?;

        let mut labels = HashMap::with_capacity(rows.len());
        for row in &rows {
            let id: Uuid = row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read {} id: {e}", def.table)))?;
            let name: String = row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read {} name: {e}", def.table))
            })?;
            let label = if def.parent.is_some() {
                let parent_name: Option<String> = row.try_get("parent_name").ok().flatten();
                match parent_name {
                    Some(parent) => format!("{name} @ {parent}"),
                    None => name,
                }
            } else {
                name
            };
            labels.insert(id, ResolvedLabel { label });
        }
        Ok(labels)
    }
}
