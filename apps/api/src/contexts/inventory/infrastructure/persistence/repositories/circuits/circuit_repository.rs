//! Circuit repository — full CRUD over `circuits_circuit`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use chrono::NaiveDate;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::{ListColumn, SortSpec, push_order_by, push_string_in, push_uuid_in};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::list_query::push_search_ilike;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const CIRCUIT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("cid", "c.cid", "inventory.circuits.columns.cid"),
    ListColumn::enum_filter(
        "status",
        "c.status",
        "c.status",
        "inventory.circuits.columns.status",
    ),
    ListColumn::fk_filter(
        "provider",
        "p.name",
        "provider_id",
        "c.provider_id",
        "providers",
        "inventory.circuits.columns.provider",
    ),
    ListColumn::fk_filter(
        "circuit_type",
        "ct.name",
        "type_id",
        "c.type_id",
        "circuit-types",
        "inventory.circuits.columns.circuitType",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "c.tenant_id",
        "tenants",
        "inventory.circuits.columns.tenant",
    ),
    ListColumn::sortable(
        "install_date",
        "c.install_date",
        "inventory.circuits.columns.installDate",
    ),
    ListColumn::sortable(
        "termination_date",
        "c.termination_date",
        "inventory.circuits.columns.terminationDate",
    ),
    ListColumn::sortable(
        "commit_rate",
        "c.commit_rate",
        "inventory.circuits.columns.commitRate",
    ),
    ListColumn::sortable(
        "distance",
        "c.distance",
        "inventory.circuits.columns.distance",
    ),
    ListColumn::sortable(
        "distance_unit",
        "c.distance_unit",
        "inventory.circuits.columns.distanceUnit",
    ),
    ListColumn::sortable(
        "description",
        "c.description",
        "inventory.circuits.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "c.comments",
        "inventory.circuits.columns.comments",
    ),
];

/// Representation of a `circuits_circuit` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct CircuitRecord {
    pub id: Uuid,
    pub cid: String,
    pub status: String,
    pub provider_id: Uuid,
    pub type_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub provider_account_id: Option<Uuid>,
    pub termination_a_id: Option<Uuid>,
    pub termination_z_id: Option<Uuid>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: String,
    pub comments: String,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
    pub custom_field_data: Option<serde_json::Value>,
}

/// Hydrated list row for `circuits_circuit` — includes provider/type/tenant
/// names resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct CircuitListItem {
    pub record: CircuitRecord,
    pub provider_name: Option<String>,
    pub type_name: Option<String>,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `circuits_circuit` row.
#[derive(Debug, Clone)]
pub struct NewCircuit {
    pub id: Uuid,
    pub cid: String,
    pub status: Option<String>,
    pub provider_id: Uuid,
    pub type_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub provider_account_id: Option<Uuid>,
    pub termination_a_id: Option<Uuid>,
    pub termination_z_id: Option<Uuid>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
}

/// Fields that may be updated on an existing `circuits_circuit` row.
#[derive(Debug, Clone)]
pub struct UpdateCircuit {
    pub cid: Option<String>,
    pub status: Option<String>,
    pub provider_id: Option<Uuid>,
    pub type_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub provider_account_id: Option<Uuid>,
    pub termination_a_id: Option<Uuid>,
    pub termination_z_id: Option<Uuid>,
    pub install_date: Option<NaiveDate>,
    pub termination_date: Option<NaiveDate>,
    pub commit_rate: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub distance: Option<f64>,
    pub distance_unit: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkCircuitResult {
    pub created: Vec<CircuitRecord>,
    pub errors: Vec<String>,
}

/// Filters, search and sort of one `list_hydrated` call.
#[derive(Debug, Default)]
pub struct CircuitFilter {
    pub provider_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub type_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait CircuitRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitRecord>, AppError>;
    async fn list_hydrated(
        &self,
        filter: CircuitFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<CircuitListItem>, i64), AppError>;
    async fn create(&self, new_circuit: &NewCircuit) -> Result<CircuitRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCircuit,
    ) -> Result<Option<CircuitRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewCircuit]) -> Result<BulkCircuitResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxCircuitRepository {
    pool: PgPool,
}

// `distance` is NUMERIC in the schema but `f64` in the domain, and sqlx does
// not decode NUMERIC into f64 — the cast is what makes the column readable at
// all. Without it the read failed, `.ok()` turned the failure into `None`, and
// `update()` (which merges unspecified fields from the row it just read) wrote
// that `None` back: an operator-entered circuit length was destroyed by the
// next unrelated edit.
const CIRCUIT_SELECT_COLUMNS: &str = r#"
    id, cid, status, provider_id, type_id, tenant_id, provider_account_id,
    termination_a_id, termination_z_id, install_date, termination_date,
    commit_rate, description, comments, distance::float8 AS distance,
    distance_unit, custom_field_data
"#;

impl SqlxCircuitRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn push_hydrated_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        provider_id: &[Uuid],
        tenant_id: &[Uuid],
        type_id: &[Uuid],
        status: &[String],
        search: Option<&str>,
    ) {
        let mut where_state = WhereState::new();
        push_uuid_in(builder, &mut where_state, "c.provider_id", provider_id);
        push_uuid_in(builder, &mut where_state, "c.tenant_id", tenant_id);
        push_uuid_in(builder, &mut where_state, "c.type_id", type_id);
        push_string_in(builder, &mut where_state, "c.status", status);
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "c.cid",
                "c.status",
                "c.install_date",
                "c.termination_date",
                "c.commit_rate",
                "c.description",
                "c.comments",
                "c.distance",
                "c.distance_unit",
                "p.name",
                "ct.name",
                "t.name",
            ],
            search,
        );
    }

    async fn count_hydrated(
        &self,
        provider_id: &[Uuid],
        tenant_id: &[Uuid],
        type_id: &[Uuid],
        status: &[String],
        search: Option<&str>,
    ) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT COUNT(*) FROM circuits_circuit c
            LEFT JOIN circuits_provider p ON p.id = c.provider_id
            LEFT JOIN circuits_circuittype ct ON ct.id = c.type_id
            LEFT JOIN tenancy_tenant t ON t.id = c.tenant_id
            "#,
        );
        Self::push_hydrated_filters(
            &mut builder,
            provider_id,
            tenant_id,
            type_id,
            status,
            search,
        );
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count circuits: {}", e)))
    }

    fn parse_row(row: &PgRow) -> Result<CircuitRecord, AppError> {
        Ok(CircuitRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read circuit id: {}", e)))?,
            cid: row
                .try_get("cid")
                .map_err(|e| AppError::Database(format!("Failed to read circuit cid: {}", e)))?,
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read circuit status: {}", e)))?,
            provider_id: row.try_get("provider_id").map_err(|e| {
                AppError::Database(format!("Failed to read circuit provider_id: {}", e))
            })?,
            type_id: row.try_get("type_id").map_err(|e| {
                AppError::Database(format!("Failed to read circuit type_id: {}", e))
            })?,
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read circuit tenant_id: {}", e))
            })?,
            provider_account_id: row
                .try_get::<Option<Uuid>, _>("provider_account_id")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read circuit provider_account_id: {}", e))
                })?,
            termination_a_id: row
                .try_get::<Option<Uuid>, _>("termination_a_id")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read circuit termination_a_id: {}", e))
                })?,
            termination_z_id: row
                .try_get::<Option<Uuid>, _>("termination_z_id")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read circuit termination_z_id: {}", e))
                })?,
            install_date: row
                .try_get::<Option<NaiveDate>, _>("install_date")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read circuit install_date: {}", e))
                })?,
            termination_date: row
                .try_get::<Option<NaiveDate>, _>("termination_date")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read circuit termination_date: {}", e))
                })?,
            commit_rate: row.try_get::<Option<i32>, _>("commit_rate").map_err(|e| {
                AppError::Database(format!("Failed to read circuit commit_rate: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read circuit description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read circuit comments: {}", e))
            })?,
            distance: row
                .try_get::<Option<f64>, _>("distance")
                .map_err(|e| AppError::Database(format!("Failed to read circuit distance: {e}")))?,
            distance_unit: row
                .try_get::<Option<String>, _>("distance_unit")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read circuit distance_unit: {e}"))
                })?,
            custom_field_data: col(row, "custom_field_data")?,
        })
    }
}

impl CircuitRepository for SqlxCircuitRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitRecord>, AppError> {
        let sql = format!("SELECT {CIRCUIT_SELECT_COLUMNS} FROM circuits_circuit WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load circuit: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_hydrated(
        &self,
        filter: CircuitFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<CircuitListItem>, i64), AppError> {
        let CircuitFilter {
            provider_id,
            tenant_id,
            type_id,
            status,
            search,
            sort,
        } = filter;
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT c.id, c.cid, c.status, c.provider_id, c.type_id, c.tenant_id,
                   c.provider_account_id, c.termination_a_id, c.termination_z_id,
                   c.install_date, c.termination_date, c.commit_rate, c.description,
                   c.comments, c.distance::float8 AS distance, c.distance_unit,
                   c.custom_field_data,
                   p.name AS provider_name,
                   ct.name AS type_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM circuits_circuit c
            LEFT JOIN circuits_provider p ON p.id = c.provider_id
            LEFT JOIN circuits_circuittype ct ON ct.id = c.type_id
            LEFT JOIN tenancy_tenant t ON t.id = c.tenant_id
            "#,
        );
        Self::push_hydrated_filters(
            &mut builder,
            &provider_id,
            &tenant_id,
            &type_id,
            &status,
            search.as_deref(),
        );
        push_order_by(
            &mut builder,
            CIRCUIT_COLUMNS,
            sort.as_ref(),
            "c.cid ASC, c.id",
        );
        if sort.is_some() {
            builder.push(", c.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list circuits hydrated: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read circuit total_count: {}", e))
            })?,
            None => {
                self.count_hydrated(
                    &provider_id,
                    &tenant_id,
                    &type_id,
                    &status,
                    search.as_deref(),
                )
                .await?
            }
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(CircuitListItem {
                    record: Self::parse_row(row)?,
                    provider_name: col(row, "provider_name")?,
                    type_name: col(row, "type_name")?,
                    tenant_name: col(row, "tenant_name")?,
                })
            })
            .collect::<Result<Vec<CircuitListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_circuit: &NewCircuit) -> Result<CircuitRecord, AppError> {
        let status = new_circuit
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("active")
            .to_string();
        let description = new_circuit.description.clone().unwrap_or_default();
        let comments = new_circuit.comments.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO circuits_circuit (
                id, custom_field_data, cid, status, provider_id, type_id,
                tenant_id, provider_account_id, termination_a_id, termination_z_id,
                install_date, termination_date, commit_rate, description, comments,
                distance, distance_unit
            )
            VALUES (
                $1, '{}'::jsonb, $2, $3, $4, $5,
                $6, $7, $8, $9,
                $10, $11, $12, $13, $14,
                $15, $16
            )
            "#,
        )
        .bind(new_circuit.id)
        .bind(&new_circuit.cid)
        .bind(&status)
        .bind(new_circuit.provider_id)
        .bind(new_circuit.type_id)
        .bind(new_circuit.tenant_id)
        .bind(new_circuit.provider_account_id)
        .bind(new_circuit.termination_a_id)
        .bind(new_circuit.termination_z_id)
        .bind(new_circuit.install_date)
        .bind(new_circuit.termination_date)
        .bind(new_circuit.commit_rate)
        .bind(&description)
        .bind(&comments)
        .bind(new_circuit.distance)
        .bind(new_circuit.distance_unit.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create circuit: {}", e)))?;

        Ok(CircuitRecord {
            id: new_circuit.id,
            cid: new_circuit.cid.clone(),
            status,
            provider_id: new_circuit.provider_id,
            type_id: new_circuit.type_id,
            tenant_id: new_circuit.tenant_id,
            provider_account_id: new_circuit.provider_account_id,
            termination_a_id: new_circuit.termination_a_id,
            termination_z_id: new_circuit.termination_z_id,
            install_date: new_circuit.install_date,
            termination_date: new_circuit.termination_date,
            commit_rate: new_circuit.commit_rate,
            description,
            comments,
            distance: new_circuit.distance,
            distance_unit: new_circuit.distance_unit.clone(),
            custom_field_data: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCircuit,
    ) -> Result<Option<CircuitRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let cid = changes.cid.clone().unwrap_or(existing.cid);
        let status = changes.status.clone().unwrap_or(existing.status);
        let provider_id = changes.provider_id.unwrap_or(existing.provider_id);
        let type_id = changes.type_id.unwrap_or(existing.type_id);
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let provider_account_id = if changes.provider_account_id.is_some() {
            changes.provider_account_id
        } else {
            existing.provider_account_id
        };
        let termination_a_id = if changes.termination_a_id.is_some() {
            changes.termination_a_id
        } else {
            existing.termination_a_id
        };
        let termination_z_id = if changes.termination_z_id.is_some() {
            changes.termination_z_id
        } else {
            existing.termination_z_id
        };
        let install_date = if changes.install_date.is_some() {
            changes.install_date
        } else {
            existing.install_date
        };
        let termination_date = if changes.termination_date.is_some() {
            changes.termination_date
        } else {
            existing.termination_date
        };
        let commit_rate = if changes.commit_rate.is_some() {
            changes.commit_rate
        } else {
            existing.commit_rate
        };
        let description = changes.description.clone().unwrap_or(existing.description);
        let comments = changes.comments.clone().unwrap_or(existing.comments);
        let distance = if changes.distance.is_some() {
            changes.distance
        } else {
            existing.distance
        };
        let distance_unit = if changes.distance_unit.is_some() {
            changes.distance_unit.clone()
        } else {
            existing.distance_unit
        };

        sqlx::query(
            r#"
            UPDATE circuits_circuit
            SET cid = $2, status = $3, provider_id = $4, type_id = $5,
                tenant_id = $6, provider_account_id = $7, termination_a_id = $8,
                termination_z_id = $9, install_date = $10, termination_date = $11,
                commit_rate = $12, description = $13, comments = $14,
                distance = $15, distance_unit = $16
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&cid)
        .bind(&status)
        .bind(provider_id)
        .bind(type_id)
        .bind(tenant_id)
        .bind(provider_account_id)
        .bind(termination_a_id)
        .bind(termination_z_id)
        .bind(install_date)
        .bind(termination_date)
        .bind(commit_rate)
        .bind(&description)
        .bind(&comments)
        .bind(distance)
        .bind(distance_unit.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update circuit: {}", e)))?;

        Ok(Some(CircuitRecord {
            id,
            cid,
            status,
            provider_id,
            type_id,
            tenant_id,
            provider_account_id,
            termination_a_id,
            termination_z_id,
            install_date,
            termination_date,
            commit_rate,
            description,
            comments,
            distance,
            distance_unit,
            custom_field_data: existing.custom_field_data,
        }))
    }

    async fn bulk_create(&self, items: &[NewCircuit]) -> Result<BulkCircuitResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let status = item
                .status
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("active")
                .to_string();
            let description = item.description.clone().unwrap_or_default();
            let comments = item.comments.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO circuits_circuit (
                    id, custom_field_data, cid, status, provider_id, type_id,
                    tenant_id, provider_account_id, termination_a_id, termination_z_id,
                    install_date, termination_date, commit_rate, description, comments,
                    distance, distance_unit
                )
                VALUES (
                    $1, '{}'::jsonb, $2, $3, $4, $5,
                    $6, $7, $8, $9,
                    $10, $11, $12, $13, $14,
                    $15, $16
                )
                "#,
                )
                .bind(item.id)
                .bind(&item.cid)
                .bind(&status)
                .bind(item.provider_id)
                .bind(item.type_id)
                .bind(item.tenant_id)
                .bind(item.provider_account_id)
                .bind(item.termination_a_id)
                .bind(item.termination_z_id)
                .bind(item.install_date)
                .bind(item.termination_date)
                .bind(item.commit_rate)
                .bind(&description)
                .bind(&comments)
                .bind(item.distance)
                .bind(item.distance_unit.as_deref()),
            )
            .await?;

            match result {
                Ok(_) => created.push(CircuitRecord {
                    id: item.id,
                    cid: item.cid.clone(),
                    status,
                    provider_id: item.provider_id,
                    type_id: item.type_id,
                    tenant_id: item.tenant_id,
                    provider_account_id: item.provider_account_id,
                    termination_a_id: item.termination_a_id,
                    termination_z_id: item.termination_z_id,
                    install_date: item.install_date,
                    termination_date: item.termination_date,
                    commit_rate: item.commit_rate,
                    description,
                    comments,
                    distance: item.distance,
                    distance_unit: item.distance_unit.clone(),
                    custom_field_data: None,
                }),
                Err(e) => {
                    errors.push(format!("Failed to insert circuit '{}': {}", item.cid, e));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk circuit insert: {}", e))
        })?;

        Ok(BulkCircuitResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "circuits_circuit", id)
            .await
            .map(|_| ())
    }
}
