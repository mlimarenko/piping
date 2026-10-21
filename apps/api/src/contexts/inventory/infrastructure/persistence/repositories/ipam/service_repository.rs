//! Service repository — full CRUD over `ipam_service`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! The parent link is a Generic Foreign Key (GFK):
//! `parent_object_type_id` + `parent_object_id` (both uuid NOT NULL).

use std::collections::HashMap;

use sqlx::postgres::PgRow;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::native::dataflow::{DataflowEndpoint, EndpointType};
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;
use crate::errors::classify_sqlx_error_owned;

pub const SERVICE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::enum_filter(
        "protocol",
        "protocol",
        "protocol",
        "inventory.resources.fields.protocol",
    ),
    ListColumn::sortable("ports", "ports", "inventory.resources.fields.ports"),
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
/// `protocol`/`ports` are the declared native columns and are nullable: an
/// imported source row may omit them. `port_mappings` is the canonical
/// projection of the `ipam_service_port_mapping` relation, whose entries are
/// `<protocol>/<port>` endpoint strings; it is empty when the service declares
/// none and never a fabricated placeholder for the native columns.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub id: Uuid,
    pub name: String,
    pub protocol: Option<String>,
    pub ports: Option<Vec<i32>>,
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
    pub protocol: String,
    pub ports: Vec<i32>,
    pub parent_object_type_id: Uuid,
    pub parent_object_id: Uuid,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub ipaddresses: Vec<Uuid>,
}

/// Fields that may be updated on an existing `ipam_service` row.
#[derive(Debug, Clone)]
pub struct UpdateService {
    pub name: Option<String>,
    pub protocol: Option<String>,
    pub ports: Option<Vec<i32>>,
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
    pub protocol: Vec<String>,
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
            protocol: row.try_get::<Option<String>, _>("protocol").map_err(|e| {
                AppError::Database(format!("Failed to read service protocol: {}", e))
            })?,
            ports: row
                .try_get::<Option<Vec<i32>>, _>("ports")
                .map_err(|e| AppError::Database(format!("Failed to read service ports: {}", e)))?,
            port_mappings: row
                .try_get::<Option<Vec<String>>, _>("port_mappings")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read service port_mappings: {}", e))
                })?
                .unwrap_or_default(),
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
            "protocol" => ServiceFilterValues::Strings(&filter.protocol),
            "parent_object_id" => ServiceFilterValues::Uuids(&filter.parent_object_id),
            "owner_id" => ServiceFilterValues::Uuids(&filter.owner_id),
            _ => ServiceFilterValues::None,
        }
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &ServiceFilter) {
        let has_filter = filter.search.is_some()
            || !filter.protocol.is_empty()
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
            &["name", "protocol", "ports", "description"],
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
                (FilterKind::String, ServiceFilterValues::Strings(values)) => {
                    push_string_in(builder, &mut where_state, column_filter.sql, values);
                }
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

    /// Project the canonical `ipam_service_port_mapping` relation onto a batch
    /// of records with one aggregate query (never one query per row).
    async fn attach_port_mappings(&self, records: &mut [ServiceRecord]) -> Result<(), AppError> {
        if records.is_empty() {
            return Ok(());
        }
        let ids: Vec<Uuid> = records.iter().map(|record| record.id).collect();
        let rows = sqlx::query(
            r#"
            SELECT service_id,
                   array_agg(protocol || '/' || port::text ORDER BY ordinal) AS port_mappings
            FROM ipam_service_port_mapping
            WHERE service_id = ANY($1)
            GROUP BY service_id
            "#,
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load service port mappings: {}", e)))?;

        let mut by_service: HashMap<Uuid, Vec<String>> = HashMap::with_capacity(rows.len());
        for row in &rows {
            let service_id: Uuid = row.try_get("service_id").map_err(|e| {
                AppError::Database(format!("Failed to read port mapping service_id: {}", e))
            })?;
            let mappings: Vec<String> = row
                .try_get("port_mappings")
                .map_err(|e| AppError::Database(format!("Failed to read port mappings: {}", e)))?;
            by_service.insert(service_id, mappings);
        }
        for record in records.iter_mut() {
            record.port_mappings = by_service.remove(&record.id).unwrap_or_default();
        }
        Ok(())
    }
}

enum ServiceFilterValues<'a> {
    Strings(&'a [String]),
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
            SELECT id, name, protocol, ports, parent_object_type_id, parent_object_id,
                   description, owner_id, custom_field_data,
                   (SELECT array_agg(pm.protocol || '/' || pm.port::text ORDER BY pm.ordinal)
                      FROM ipam_service_port_mapping pm
                     WHERE pm.service_id = ipam_service.id) AS port_mappings,
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
            SELECT id, name, protocol, ports, parent_object_type_id, parent_object_id,
                   description, owner_id, custom_field_data,
                   (SELECT array_agg(pm.protocol || '/' || pm.port::text ORDER BY pm.ordinal)
                      FROM ipam_service_port_mapping pm
                     WHERE pm.service_id = ipam_service.id) AS port_mappings,
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
                id, custom_field_data, name, protocol, ports,
                parent_object_type_id, parent_object_id,
                description, comments, owner_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4::int[], $5, $6, $7, '', $8)
            "#,
        )
        .bind(new_service.id)
        .bind(&new_service.name)
        .bind(&new_service.protocol)
        .bind(&new_service.ports)
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
        let row = sqlx::query("SELECT s.*, (SELECT array_agg(pm.protocol || '/' || pm.port::text ORDER BY pm.ordinal) FROM ipam_service_port_mapping pm WHERE pm.service_id = s.id) AS port_mappings, (SELECT array_agg(ipaddress_id ORDER BY ipaddress_id) FROM ipam_service_ipaddresses WHERE service_id = s.id) AS ipaddresses FROM ipam_service s WHERE id = $1 FOR UPDATE")
            .bind(id).fetch_optional(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let existing = Self::parse_row(&row)?;
        let ipaddresses = changes
            .ipaddresses
            .as_deref()
            .unwrap_or(&existing.ipaddresses);

        let name = changes.name.clone().unwrap_or(existing.name);
        let protocol = changes.protocol.clone().or(existing.protocol);
        let ports = changes.ports.clone().or(existing.ports);
        let parent_object_type_id = changes
            .parent_object_type_id
            .unwrap_or(existing.parent_object_type_id);
        let parent_object_id = changes
            .parent_object_id
            .unwrap_or(existing.parent_object_id);
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
            SET name = $2, protocol = $3, ports = $4::int[],
                parent_object_type_id = $5, parent_object_id = $6,
                description = $7
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&protocol)
        .bind(&ports)
        .bind(parent_object_type_id)
        .bind(parent_object_id)
        .bind(description.clone().unwrap_or_default())
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        replace_service_ipaddresses(&mut tx, id, ipaddresses).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        self.get_by_id(id).await
    }

    async fn bulk_create(
        &self,
        items: &[NewService],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ServiceRecord>, AppError> {
        let mut result = super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk service insert",
            |connection, item| {
                Box::pin(async move {
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
                    id, custom_field_data, name, protocol, ports,
                    parent_object_type_id, parent_object_id,
                    description, comments, owner_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4::int[], $5, $6, $7, '', $8)
                "#,
                    )
                    .bind(item.id)
                    .bind(&item.name)
                    .bind(&item.protocol)
                    .bind(&item.ports)
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
                        protocol: Some(item.protocol.clone()),
                        ports: Some(item.ports.clone()),
                        port_mappings: Vec::new(),
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

        self.attach_port_mappings(&mut result.created).await?;
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
