//! VRF repository — full CRUD over `ipam_vrf`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! Import/export route targets are NetBox M2M relations stored in the
//! `ipam_vrf_import_targets` / `ipam_vrf_export_targets` join tables. They are
//! read via correlated `array(...)` subqueries and written by replacing the
//! join rows (delete + insert) inside the same transaction as the scalar write,
//! so a VRF and its route-target links are always consistent.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VRF_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "v.name", "inventory.resources.fields.name"),
    ListColumn::sortable("rd", "v.rd", "inventory.resources.fields.rd"),
    ListColumn::enum_filter(
        "enforce_unique",
        "v.enforce_unique",
        "v.enforce_unique",
        "inventory.resources.fields.enforceUnique",
    ),
    ListColumn::sortable(
        "description",
        "v.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "v.comments",
        "inventory.resources.fields.comments",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "v.tenant_id",
        "tenants",
        "inventory.resources.fields.tenant",
    ),
    ListColumn::sortable("tenantName", "t.name", "inventory.resources.fields.tenant"),
];

/// Representation of an `ipam_vrf` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct VrfRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub rd: Option<String>,
    pub enforce_unique: bool,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub import_targets: Vec<Uuid>,
    pub export_targets: Vec<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `ipam_vrf` — includes the tenant name resolved via
/// LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct VrfListItem {
    pub record: VrfRecord,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `ipam_vrf` row.
#[derive(Debug, Clone)]
pub struct NewVrf {
    pub id: Uuid,
    pub name: String,
    pub rd: Option<String>,
    pub enforce_unique: bool,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub import_targets: Vec<Uuid>,
    pub export_targets: Vec<Uuid>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_vrf` row.
#[derive(Debug, Clone)]
pub struct UpdateVrf {
    pub name: Option<String>,
    pub rd: Option<String>,
    pub enforce_unique: Option<bool>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub import_targets: Option<Vec<Uuid>>,
    pub export_targets: Option<Vec<Uuid>>,
}

/// Selection, paging and ordering for the VRF collection.
pub struct VrfListParams<'a> {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub enforce_unique: &'a [String],
    pub tenant_ids: &'a [Uuid],
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VrfRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VrfRecord>, AppError>;
    async fn list(&self, params: VrfListParams<'_>) -> Result<(Vec<VrfListItem>, i64), AppError>;
    async fn create(&self, new_vrf: &NewVrf) -> Result<VrfRecord, AppError>;
    async fn update(&self, id: Uuid, changes: &UpdateVrf) -> Result<Option<VrfRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewVrf],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<VrfRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVrfRepository {
    pool: PgPool,
}

impl SqlxVrfRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VrfRecord, AppError> {
        Ok(VrfRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read vrf id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read vrf external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read vrf name: {}", e)))?,
            rd: col::<Option<String>>(row, "rd")?,
            enforce_unique: row.try_get("enforce_unique").map_err(|e| {
                AppError::Database(format!("Failed to read vrf enforce_unique: {}", e))
            })?,
            description: col::<Option<String>>(row, "description")?,
            comments: col::<Option<String>>(row, "comments")?,
            tenant_id: col::<Option<Uuid>>(row, "tenant_id")?,
            import_targets: col::<Vec<Uuid>>(row, "import_targets")?,
            export_targets: col::<Vec<Uuid>>(row, "export_targets")?,
            owner_id: row
                .try_get::<Option<Uuid>, _>("owner_id")
                .map_err(|e| AppError::Database(format!("Failed to read vrf owner_id: {}", e)))?,
            tags: col::<Vec<String>>(row, "tags")?,
            custom_fields: col::<Option<serde_json::Value>>(row, "custom_field_data")?,
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read vrf piping_synced_at: {}", e))
                })?,
        })
    }

    /// Replace the import/export route-target join rows for a VRF inside an
    /// existing transaction (delete all, then insert the supplied set).
    async fn replace_targets(
        tx: &mut Transaction<'_, Postgres>,
        vrf_id: Uuid,
        import_targets: &[Uuid],
        export_targets: &[Uuid],
    ) -> Result<(), AppError> {
        sqlx::query("DELETE FROM ipam_vrf_import_targets WHERE vrf_id = $1")
            .bind(vrf_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to clear vrf import_targets: {}", e))
            })?;
        sqlx::query("DELETE FROM ipam_vrf_export_targets WHERE vrf_id = $1")
            .bind(vrf_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to clear vrf export_targets: {}", e))
            })?;

        for routetarget_id in import_targets {
            sqlx::query(
                r#"
                INSERT INTO ipam_vrf_import_targets (id, vrf_id, routetarget_id)
                VALUES (uuidv7(), $1, $2)
                "#,
            )
            .bind(vrf_id)
            .bind(routetarget_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to insert vrf import_target: {}", e))
            })?;
        }
        for routetarget_id in export_targets {
            sqlx::query(
                r#"
                INSERT INTO ipam_vrf_export_targets (id, vrf_id, routetarget_id)
                VALUES (uuidv7(), $1, $2)
                "#,
            )
            .bind(vrf_id)
            .bind(routetarget_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to insert vrf export_target: {}", e))
            })?;
        }
        Ok(())
    }

    /// Insert the scalar `ipam_vrf` row inside an existing transaction.
    async fn insert_scalar(
        tx: &mut Transaction<'_, Postgres>,
        item: &NewVrf,
    ) -> Result<(), AppError> {
        let description = item.description.clone().unwrap_or_default();
        let comments = item.comments.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_vrf (
                id, custom_field_data, name, rd, enforce_unique, description,
                tenant_id, comments, owner_id, tags
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, '{}')
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.rd.as_deref())
        .bind(item.enforce_unique)
        .bind(&description)
        .bind(item.tenant_id)
        .bind(&comments)
        .bind(item.owner_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vrf: {}", e)))?;
        Ok(())
    }
}

impl VrfRepository for SqlxVrfRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VrfRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, rd, enforce_unique, description, comments,
                   tenant_id, owner_id, tags, custom_field_data, piping_synced_at,
                   ARRAY(SELECT routetarget_id FROM ipam_vrf_import_targets
                         WHERE vrf_id = ipam_vrf.id) AS import_targets,
                   ARRAY(SELECT routetarget_id FROM ipam_vrf_export_targets
                         WHERE vrf_id = ipam_vrf.id) AS export_targets
            FROM ipam_vrf
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vrf: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(&self, params: VrfListParams<'_>) -> Result<(Vec<VrfListItem>, i64), AppError> {
        let limit = clamp_limit(params.limit) as i64;
        let offset = clamp_offset(params.offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT v.id, v.external_id, v.name, v.rd, v.enforce_unique, v.description, v.comments,
                   v.tenant_id, v.owner_id, v.tags, v.custom_field_data, v.piping_synced_at,
                   ARRAY(SELECT routetarget_id FROM ipam_vrf_import_targets
                         WHERE vrf_id = v.id) AS import_targets,
                   ARRAY(SELECT routetarget_id FROM ipam_vrf_export_targets
                         WHERE vrf_id = v.id) AS export_targets,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_vrf v
            LEFT JOIN tenancy_tenant t ON t.id = v.tenant_id
            "#,
        );
        push_vrf_filters(
            &mut builder,
            params.enforce_unique,
            params.tenant_ids,
            params.search.as_deref(),
        );
        push_order_by(&mut builder, VRF_COLUMNS, params.sort.as_ref(), "v.name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list vrfs: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vrf total_count: {}", e))
            })?,
            None => {
                count_vrfs(
                    &self.pool,
                    params.enforce_unique,
                    params.tenant_ids,
                    params.search.as_deref(),
                )
                .await?
            }
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(VrfListItem {
                    record: Self::parse_row(row)?,
                    tenant_name: col::<Option<String>>(row, "tenant_name")?,
                })
            })
            .collect::<Result<Vec<VrfListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_vrf: &NewVrf) -> Result<VrfRecord, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("Failed to begin transaction: {}", e)))?;

        Self::insert_scalar(&mut tx, new_vrf).await?;
        Self::replace_targets(
            &mut tx,
            new_vrf.id,
            &new_vrf.import_targets,
            &new_vrf.export_targets,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit vrf insert: {}", e)))?;

        Ok(VrfRecord {
            id: new_vrf.id,
            external_id: None,
            name: new_vrf.name.clone(),
            rd: new_vrf.rd.clone(),
            enforce_unique: new_vrf.enforce_unique,
            description: Some(new_vrf.description.clone().unwrap_or_default()),
            comments: Some(new_vrf.comments.clone().unwrap_or_default()),
            tenant_id: new_vrf.tenant_id,
            import_targets: new_vrf.import_targets.clone(),
            export_targets: new_vrf.export_targets.clone(),
            owner_id: new_vrf.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(&self, id: Uuid, changes: &UpdateVrf) -> Result<Option<VrfRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let rd = if changes.rd.is_some() {
            changes.rd.clone()
        } else {
            existing.rd
        };
        let enforce_unique = changes.enforce_unique.unwrap_or(existing.enforce_unique);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };
        let comments = if changes.comments.is_some() {
            changes.comments.clone()
        } else {
            existing.comments
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let import_targets = changes
            .import_targets
            .clone()
            .unwrap_or(existing.import_targets);
        let export_targets = changes
            .export_targets
            .clone()
            .unwrap_or(existing.export_targets);

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("Failed to begin transaction: {}", e)))?;

        sqlx::query(
            r#"
            UPDATE ipam_vrf
            SET name = $2, rd = $3, enforce_unique = $4, description = $5,
                comments = $6, tenant_id = $7
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(rd.as_deref())
        .bind(enforce_unique)
        .bind(description.clone().unwrap_or_default())
        .bind(comments.clone().unwrap_or_default())
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vrf: {}", e)))?;

        Self::replace_targets(&mut tx, id, &import_targets, &export_targets).await?;

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit vrf update: {}", e)))?;

        Ok(Some(VrfRecord {
            id,
            external_id: existing.external_id,
            name,
            rd,
            enforce_unique,
            description,
            comments,
            tenant_id,
            import_targets,
            export_targets,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewVrf],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<VrfRecord>, AppError> {
        // Intentionally not `bulk_insert::execute`: replacing a VRF's relation
        // rows is committed per item, so one later commit failure must not roll
        // back earlier successful VRFs.
        let mut created = Vec::new();
        let mut errors = Vec::new();

        for item in items {
            let mut tx =
                self.pool.begin().await.map_err(|e| {
                    AppError::Database(format!("Failed to begin transaction: {}", e))
                })?;

            let result = async {
                Self::insert_scalar(&mut tx, item).await?;
                Self::replace_targets(&mut tx, item.id, &item.import_targets, &item.export_targets)
                    .await
            }
            .await;

            match result {
                Ok(()) => match tx.commit().await {
                    Ok(()) => created.push(VrfRecord {
                        id: item.id,
                        external_id: None,
                        name: item.name.clone(),
                        rd: item.rd.clone(),
                        enforce_unique: item.enforce_unique,
                        description: Some(item.description.clone().unwrap_or_default()),
                        comments: Some(item.comments.clone().unwrap_or_default()),
                        tenant_id: item.tenant_id,
                        import_targets: item.import_targets.clone(),
                        export_targets: item.export_targets.clone(),
                        owner_id: item.owner_id,
                        tags: Vec::new(),
                        custom_fields: None,
                        piping_synced_at: None,
                    }),
                    Err(e) => errors.push(format!("Failed to commit vrf '{}': {}", item.name, e)),
                },
                Err(e) => {
                    let _ = tx.rollback().await;
                    errors.push(format!("Failed to insert vrf '{}': {}", item.name, e));
                }
            }
        }

        Ok(super::super::bulk_insert::BulkCreateResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_vrf", id)
            .await
            .map(|_| ())
    }
}

fn push_vrf_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    enforce_unique: &[String],
    tenant_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_string_in(
        builder,
        &mut where_state,
        "v.enforce_unique::text",
        enforce_unique,
    );
    push_uuid_in(builder, &mut where_state, "v.tenant_id", tenant_ids);
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "v.name",
            "v.rd",
            "v.enforce_unique",
            "v.description",
            "v.comments",
            "t.name",
        ],
        search,
    );
}

async fn count_vrfs(
    pool: &PgPool,
    enforce_unique: &[String],
    tenant_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new(
        "SELECT COUNT(*) FROM ipam_vrf v LEFT JOIN tenancy_tenant t ON t.id = v.tenant_id",
    );
    push_vrf_filters(&mut builder, enforce_unique, tenant_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count vrfs: {}", e)))
}
