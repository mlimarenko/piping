//! Service repository — full CRUD over `ipam_service`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! The parent link is a Generic Foreign Key (GFK):
//! `parent_object_type_id` + `parent_object_id` (both uuid NOT NULL).

use crate::contexts::inventory::domain::ipam::service::validate_port_mappings;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::native::dataflow::{DataflowEndpoint, EndpointType};
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;
use crate::errors::classify_sqlx_error_owned;

pub const SERVICE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "port_mappings",
        "port_mappings",
        "inventory.resources.fields.portMappings",
    ),
    ListColumn::fk_filter(
        "parentObject",
        "parent_object_id",
        "parent_object_id",
        "parent_object_id",
        "objects",
        "inventory.resources.fields.parentObject",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
    ListColumn::fk_filter(
        "owner",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.resources.fields.owner",
    ),
];

/// Representation of an `ipam_service` row exposed by the REST API.
///
/// `port_mappings` is the single ordered endpoint representation.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub id: Uuid,
    pub name: String,
    pub port_mappings: Vec<String>,
    pub ipaddresses: Vec<Uuid>,
    pub parent_object_type_id: Uuid,
    pub parent_object_id: Uuid,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `ipam_service` row.
#[derive(Debug, Clone)]
pub struct NewService {
    pub id: Uuid,
    pub name: String,
    pub port_mappings: Vec<String>,
    pub parent_object_type_id: Uuid,
    pub parent_object_id: Uuid,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub ipaddresses: Vec<Uuid>,
}

/// Fields that may be updated on an existing `ipam_service` row.
#[derive(Debug, Clone)]
pub struct UpdateService {
    /// Effective `(content_type_id, object_id)` authorized by the caller.
    pub authorized_parent_object: (Uuid, Uuid),
    /// Effective authorized binding identities, compared as a set under the row lock.
    pub authorized_ipaddresses: Vec<Uuid>,
    pub name: Option<String>,
    pub port_mappings: Option<Vec<String>>,
    pub parent_object_type_id: Option<Uuid>,
    pub parent_object_id: Option<Uuid>,
    pub description: Option<String>,
    pub ipaddresses: Option<Vec<Uuid>>,
}

/// Service list filter. Empty vectors mean "no filter"; non-empty vectors emit
/// parameterized `IN` predicates declared by [`SERVICE_COLUMNS`].
#[derive(Debug, Clone, Default)]
pub struct ServiceFilter {
    pub search: Option<String>,
    pub parent_object_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ServiceRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ServiceRecord>, AppError>;
    async fn list(
        &self,
        filter: ServiceFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ServiceRecord>, i64), AppError>;
    async fn create(&self, new_service: &NewService) -> Result<ServiceRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateService,
    ) -> Result<Option<ServiceRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewService],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ServiceRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxServiceRepository {
    pool: PgPool,
}

impl SqlxServiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ServiceRecord, AppError> {
        Ok(ServiceRecord {
            ipaddresses: row
                .try_get::<Option<Vec<Uuid>>, _>("ipaddresses")
                .map_err(classify_sqlx_error_owned)?
                .unwrap_or_default(),
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read service id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read service name: {}", e)))?,
            port_mappings: row
                .try_get::<Vec<String>, _>("port_mappings")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read service port_mappings: {}", e))
                })?,
            parent_object_type_id: row.try_get("parent_object_type_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read service parent_object_type_id: {}",
                    e
                ))
            })?,
            parent_object_id: row.try_get("parent_object_id").map_err(|e| {
                AppError::Database(format!("Failed to read service parent_object_id: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read service owner_id: {}", e))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn filter_values<'a>(filter: &'a ServiceFilter, param: &str) -> ServiceFilterValues<'a> {
        match param {
            "parent_object_id" => ServiceFilterValues::Uuids(&filter.parent_object_id),
            "owner_id" => ServiceFilterValues::Uuids(&filter.owner_id),
            _ => ServiceFilterValues::None,
        }
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &ServiceFilter) {
        let has_filter = filter.search.is_some()
            || !filter.parent_object_id.is_empty()
            || !filter.owner_id.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "port_mappings", "description"],
            filter.search.as_deref(),
        );

        for column in SERVICE_COLUMNS {
            let Some(column_filter) = column.filter else {
                continue;
            };
            match (
                column_filter.kind,
                Self::filter_values(filter, column_filter.param),
            ) {
                (FilterKind::Uuid, ServiceFilterValues::Uuids(values)) => {
                    push_uuid_in(builder, &mut where_state, column_filter.sql, values);
                }
                _ => {}
            }
        }
    }

    async fn count_filtered(&self, filter: &ServiceFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM ipam_service");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count services: {}", e)))
    }
}

enum ServiceFilterValues<'a> {
    Uuids(&'a [Uuid]),
    None,
}

/// Validate existing listener/egress bindings while their IP rows are locked.
async fn validate_service_ipaddresses(
    connection: &mut PgConnection,
    parent_type_id: Uuid,
    parent_id: Uuid,
    ids: &[Uuid],
) -> Result<(), AppError> {
    if ids.len() > 32 {
        return Err(AppError::Validation(
            "Service ipaddresses allows at most 32 bindings".into(),
        ));
    }
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT app_label || '.' || model FROM inventory_content_type WHERE id = $1",
    )
    .bind(parent_type_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if kind.as_deref() == Some("ipam.fhrpgroup") {
        let group: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM ipam_fhrpgroup WHERE id = $1 FOR SHARE")
                .bind(parent_id)
                .fetch_optional(&mut *connection)
                .await
                .map_err(classify_sqlx_error_owned)?;
        if group.is_none() {
            return Err(AppError::Validation(
                "Service FHRP parent does not exist".into(),
            ));
        }
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let bindings: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT assigned_object_type_id, assigned_object_id FROM ipam_ipaddress WHERE id = ANY($1) ORDER BY id FOR SHARE",
        ).bind(&ids).fetch_all(&mut *connection).await.map_err(classify_sqlx_error_owned)?;
        if bindings.len() != ids.len()
            || bindings.iter().any(|&(kind, id)| {
                !((kind.is_none() && id.is_none())
                    || (kind == Some(parent_type_id) && id == Some(parent_id)))
            })
        {
            return Err(AppError::Validation(
                "Service IP binding does not belong to its FHRP parent".into(),
            ));
        }
        return Ok(());
    }
    let parent_type = EndpointType::parse(kind.as_deref().unwrap_or(""))?;
    let table = match parent_type {
        EndpointType::Device => "dcim_device",
        EndpointType::VirtualMachine => "virtualization_virtualmachine",
        _ => {
            return Err(AppError::Validation(
                "Service parent must be a device or VM".into(),
            ));
        }
    };
    let parent: Option<Uuid> =
        sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE id = $1 FOR SHARE"))
            .bind(parent_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
    if parent.is_none() {
        return Err(AppError::Validation(
            "Service parent host does not exist".into(),
        ));
    }
    let expected = DataflowEndpoint {
        object_type: parent_type,
        object_id: parent_id,
    };
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let rows: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, assigned_object_id FROM ipam_ipaddress WHERE id = ANY($1) ORDER BY id FOR SHARE")
        .bind(&ids).fetch_all(&mut *connection).await.map_err(classify_sqlx_error_owned)?;
    if rows.len() != ids.len() {
        return Err(AppError::Validation(
            "Service IP binding does not exist".into(),
        ));
    }
    for (id, assigned) in rows {
        let owners =
            super::super::native::ipam_repository::address_host_owners(connection, id).await?;
        if !super::super::native::ipam_repository::address_assignment_valid(connection, id).await?
            || owners.iter().any(|owner| owner != &expected)
            || (assigned.is_some() && !owners.contains(&expected))
        {
            return Err(AppError::Validation(
                "Service IP binding does not belong to its parent host".into(),
            ));
        }
    }
    Ok(())
}

async fn replace_service_ipaddresses(
    connection: &mut PgConnection,
    service_id: Uuid,
    ids: &[Uuid],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM ipam_service_ipaddresses WHERE service_id = $1")
        .bind(service_id)
        .execute(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    sqlx::query("INSERT INTO ipam_service_ipaddresses (service_id, ipaddress_id) SELECT $1, id FROM (SELECT DISTINCT unnest($2::uuid[]) AS id) bindings")
        .bind(service_id).bind(ids).execute(connection).await.map_err(classify_sqlx_error_owned)?;
    Ok(())
}

impl ServiceRepository for SqlxServiceRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ServiceRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, port_mappings, parent_object_type_id, parent_object_id,
                   description, owner_id, custom_field_data,
                    (SELECT array_agg(ipaddress_id ORDER BY ipaddress_id) FROM ipam_service_ipaddresses WHERE service_id = ipam_service.id) AS ipaddresses
            FROM ipam_service
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load service: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: ServiceFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ServiceRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, port_mappings, parent_object_type_id, parent_object_id,
                   description, owner_id, custom_field_data,
                    (SELECT array_agg(ipaddress_id ORDER BY ipaddress_id) FROM ipam_service_ipaddresses WHERE service_id = ipam_service.id) AS ipaddresses,
                    COUNT(*) OVER() AS total_count
            FROM ipam_service
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(&mut builder, SERVICE_COLUMNS, filter.sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list services: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read service total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<ServiceRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_service: &NewService) -> Result<ServiceRecord, AppError> {
        let port_mappings = validate_port_mappings(&new_service.port_mappings)?;
        let description = new_service.description.clone().unwrap_or_default();
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        validate_service_ipaddresses(
            &mut tx,
            new_service.parent_object_type_id,
            new_service.parent_object_id,
            &new_service.ipaddresses,
        )
        .await?;

        sqlx::query(
            r#"
            INSERT INTO ipam_service (
                id, custom_field_data, name, port_mappings,
                parent_object_type_id, parent_object_id,
                description, comments, owner_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3::text[], $4, $5, $6, '', $7)
            "#,
        )
        .bind(new_service.id)
        .bind(&new_service.name)
        .bind(&port_mappings)
        .bind(new_service.parent_object_type_id)
        .bind(new_service.parent_object_id)
        .bind(&description)
        .bind(new_service.owner_id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        replace_service_ipaddresses(&mut tx, new_service.id, &new_service.ipaddresses).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        self.get_by_id(new_service.id)
            .await?
            .ok_or_else(|| AppError::Internal("created service could not be reloaded".to_string()))
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateService,
    ) -> Result<Option<ServiceRecord>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let mappings = changes
            .port_mappings
            .as_deref()
            .map(validate_port_mappings)
            .transpose()?;
        let row = sqlx::query(
            "SELECT s.*, NULL::uuid[] AS ipaddresses FROM ipam_service s WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut existing = Self::parse_row(&row)?;
        // A separate statement after acquiring the lock sees bindings committed
        // by a writer we may have waited for, not the locking SELECT's snapshot.
        existing.ipaddresses = sqlx::query_scalar("SELECT ipaddress_id FROM ipam_service_ipaddresses WHERE service_id = $1 ORDER BY ipaddress_id")
            .bind(id).fetch_all(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        let ipaddresses = changes
            .ipaddresses
            .as_deref()
            .unwrap_or(&existing.ipaddresses);

        let name = changes.name.clone().unwrap_or(existing.name);
        let parent_object_type_id = changes
            .parent_object_type_id
            .unwrap_or(existing.parent_object_type_id);
        let parent_object_id = changes
            .parent_object_id
            .unwrap_or(existing.parent_object_id);
        let mut effective_ips = ipaddresses.to_vec();
        effective_ips.sort_unstable();
        effective_ips.dedup();
        let mut authorized_ips = changes.authorized_ipaddresses.clone();
        authorized_ips.sort_unstable();
        authorized_ips.dedup();
        if (parent_object_type_id, parent_object_id) != changes.authorized_parent_object
            || effective_ips != authorized_ips
        {
            return Err(AppError::Conflict(
                "Service associations changed since authorization; retry the request".into(),
            ));
        }
        validate_service_ipaddresses(
            &mut tx,
            parent_object_type_id,
            parent_object_id,
            ipaddresses,
        )
        .await?;
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE ipam_service
            SET name = $2,
                parent_object_type_id = $3, parent_object_id = $4,
                description = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(parent_object_type_id)
        .bind(parent_object_id)
        .bind(description.clone().unwrap_or_default())
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if let Some(port_mappings) = mappings {
            sqlx::query("UPDATE ipam_service SET port_mappings = $2 WHERE id = $1")
                .bind(id)
                .bind(port_mappings)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
        }
        replace_service_ipaddresses(&mut tx, id, ipaddresses).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        self.get_by_id(id).await
    }

    async fn bulk_create(
        &self,
        items: &[NewService],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ServiceRecord>, AppError> {
        let result = super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk service insert",
            |connection, item| {
                Box::pin(async move {
                    let port_mappings = validate_port_mappings(&item.port_mappings)
                        .map_err(|error| error.to_string())?;
                    let description = item.description.clone().unwrap_or_default();
                    validate_service_ipaddresses(
                        connection,
                        item.parent_object_type_id,
                        item.parent_object_id,
                        &item.ipaddresses,
                    )
                    .await
                    .map_err(|error| error.to_string())?;

                    sqlx::query(
                        r#"
                INSERT INTO ipam_service (
                    id, custom_field_data, name, port_mappings,
                    parent_object_type_id, parent_object_id,
                    description, comments, owner_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3::text[], $4, $5, $6, '', $7)
                "#,
                    )
                    .bind(item.id)
                    .bind(&item.name)
                    .bind(&port_mappings)
                    .bind(item.parent_object_type_id)
                    .bind(item.parent_object_id)
                    .bind(&description)
                    .bind(item.owner_id)
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| {
                        format!("Failed to insert service '{}': {error}", item.name)
                    })?;

                    replace_service_ipaddresses(connection, item.id, &item.ipaddresses)
                        .await
                        .map_err(|error| error.to_string())?;
                    let mut ipaddresses = item.ipaddresses.clone();
                    ipaddresses.sort_unstable();
                    ipaddresses.dedup();
                    Ok(ServiceRecord {
                        ipaddresses,
                        id: item.id,
                        name: item.name.clone(),
                        port_mappings,
                        parent_object_type_id: item.parent_object_type_id,
                        parent_object_id: item.parent_object_id,
                        description: Some(description),
                        owner_id: item.owner_id,
                        custom_fields: None,
                    })
                })
            },
        )
        .await?;

        Ok(result)
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::lock_field_catalog_for_usage(&mut tx).await?;
        sqlx::query("SELECT id FROM ipam_service WHERE id = $1 FOR UPDATE")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        replace_service_ipaddresses(&mut tx, id, &[]).await?;
        super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "ipam_service",
            id,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)
    }
}
