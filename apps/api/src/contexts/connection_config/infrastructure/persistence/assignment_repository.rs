use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Error as SqlxError;
use sqlx::Row;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::connection_config::domain::{
    ConnConfAssignment, ConnConfScope, NewConnConfAssignment, UpdateConnConfAssignment,
};
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const CONNCONF_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("id", "id", "connectionConfig.assignments.columns.id"),
    ListColumn::fk_filter(
        "record_id",
        "record_id",
        "record_id",
        "record_id",
        "connection_config_records",
        "connectionConfig.assignments.columns.record",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "scope_type",
        "scope_type",
        "connectionConfig.assignments.columns.scopeType",
    ),
    ListColumn::fk_filter(
        "scope_id",
        "scope_id",
        "scope_id",
        "scope_id",
        "connection_config_scopes",
        "connectionConfig.assignments.columns.scope",
    ),
    ListColumn::sortable(
        "weight",
        "weight",
        "connectionConfig.assignments.columns.weight",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "connectionConfig.assignments.columns.enabled",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "connectionConfig.assignments.columns.createdAt",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct ConnConfAssignmentFilter {
    pub record_id: Vec<Uuid>,
    pub scope_type: Vec<ConnConfScope>,
    pub scope_id: Vec<Uuid>,
    pub enabled: Vec<String>,
    pub sort: Option<SortSpec>,
}

/// Scope-ancestry FK ids for a device or VM, used to gather applicable
/// assignments. All fields are `None` when the host lacks the association.
/// `region_id`/`site_group_id` are derived transitively from the host's site.
#[derive(Debug, Clone, Default)]
pub struct DeviceScopeIds {
    pub region_id: Option<Uuid>,
    pub site_group_id: Option<Uuid>,
    pub site_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub rack_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub cluster_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub device_type_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    /// The device id itself (for the `device` scope) when target is a device.
    pub device_id: Option<Uuid>,
    /// The VM id itself (for the `vm` scope) when target is a VM.
    pub vm_id: Option<Uuid>,
}

impl DeviceScopeIds {
    /// Translate this scope ancestry into the ordered `(scope_type, scope_id)`
    /// pairs the connection-config resolver matches on, plus the implicit
    /// `global` scope. Pure mapping, no SQL.
    ///
    /// This is the single source of truth for the ancestry → scope-pair mapping;
    /// every caller (the REST effective-config route, the connect path, and the
    /// device-management resolver) delegates here so the ordering stays identical.
    #[must_use]
    pub fn to_scope_pairs(&self) -> Vec<(ConnConfScope, Option<Uuid>)> {
        let mut out = vec![(ConnConfScope::Global, None)];
        let mut push = |scope: ConnConfScope, id: Option<Uuid>| {
            if let Some(id) = id {
                out.push((scope, Some(id)));
            }
        };
        push(ConnConfScope::Region, self.region_id);
        push(ConnConfScope::SiteGroup, self.site_group_id);
        push(ConnConfScope::Site, self.site_id);
        push(ConnConfScope::Location, self.location_id);
        push(ConnConfScope::Rack, self.rack_id);
        push(ConnConfScope::Tenant, self.tenant_id);
        push(ConnConfScope::Cluster, self.cluster_id);
        push(ConnConfScope::Role, self.role_id);
        push(ConnConfScope::DeviceType, self.device_type_id);
        push(ConnConfScope::Platform, self.platform_id);
        push(ConnConfScope::Device, self.device_id);
        push(ConnConfScope::Vm, self.vm_id);
        out
    }
}

fn decode_device_scope_ids(
    mut read: impl FnMut(&'static str) -> Result<Option<Uuid>, AppError>,
) -> Result<DeviceScopeIds, AppError> {
    Ok(DeviceScopeIds {
        region_id: read("region_id")?,
        site_group_id: read("site_group_id")?,
        site_id: read("site_id")?,
        location_id: read("location_id")?,
        rack_id: read("rack_id")?,
        tenant_id: read("tenant_id")?,
        cluster_id: read("cluster_id")?,
        role_id: read("role_id")?,
        device_type_id: read("device_type_id")?,
        platform_id: read("platform_id")?,
        device_id: read("device_id")?,
        vm_id: read("vm_id")?,
    })
}

/// CRUD + scope-resolution persistence for `connconf_assignment`.
#[async_trait]
pub trait ConnConfAssignmentRepository: Send + Sync {
    async fn list(
        &self,
        filter: ConnConfAssignmentFilter,
    ) -> Result<Vec<ConnConfAssignment>, AppError>;
    async fn list_all(&self) -> Result<Vec<ConnConfAssignment>, AppError>;
    async fn list_for_record(&self, record_id: Uuid) -> Result<Vec<ConnConfAssignment>, AppError>;
    async fn find(&self, id: Uuid) -> Result<Option<ConnConfAssignment>, AppError>;
    async fn create(&self, input: &NewConnConfAssignment) -> Result<ConnConfAssignment, AppError>;
    async fn update(
        &self,
        id: Uuid,
        input: &UpdateConnConfAssignment,
    ) -> Result<Option<ConnConfAssignment>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<bool, AppError>;
    /// List enabled assignments matching any of the supplied `(scope_type, scope_id)`
    /// pairs, plus the global scope.
    async fn list_for_scopes(
        &self,
        scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<Vec<ConnConfAssignment>, AppError>;
    /// Resolve the inventory scope ancestry for a device or VM. Returns `None`
    /// when the target exists in neither table.
    async fn get_device_scopes(&self, target_id: Uuid) -> Result<Option<DeviceScopeIds>, AppError>;
}

#[derive(Clone)]
pub struct SqlxConnConfAssignmentRepository {
    pool: PgPool,
}

impl SqlxConnConfAssignmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Atomically create an assignment unless its record/scope key already exists.
    /// Returns `None` when a concurrent or prior request won the unique-key race.
    pub async fn create_if_absent(
        &self,
        input: &NewConnConfAssignment,
    ) -> Result<Option<ConnConfAssignment>, AppError> {
        let query = format!(
            "INSERT INTO connconf_assignment \
             (record_id, scope_type, scope_id, weight, enabled) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (record_id, scope_type, COALESCE(scope_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
             DO NOTHING \
             RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(input.record_id)
            .bind(input.scope_type.as_str())
            .bind(input.scope_id)
            .bind(input.weight.unwrap_or(0))
            .bind(input.enabled.unwrap_or(true))
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("create_if_absent connconf_assignment: {e}"))
            })?;
        row.map(map_row).transpose()
    }
}

const SELECT_COLUMNS: &str = "id, record_id, scope_type, scope_id, weight, enabled, created_at";

fn push_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    filter: &ConnConfAssignmentFilter,
    state: &mut WhereState,
) {
    push_uuid_in(builder, state, "record_id", &filter.record_id);
    if !filter.scope_type.is_empty() {
        state.and(builder);
        builder.push("scope_type IN (");
        let mut separated = builder.separated(", ");
        for scope_type in &filter.scope_type {
            separated.push_bind(scope_type.as_str());
        }
        builder.push(")");
    }
    push_uuid_in(builder, state, "scope_id", &filter.scope_id);
    push_string_in(builder, state, "enabled::text", &filter.enabled);
}

fn map_row(row: sqlx::postgres::PgRow) -> Result<ConnConfAssignment, AppError> {
    let scope_raw: String = row
        .try_get("scope_type")
        .map_err(|e| AppError::Database(format!("connconf_assignment.scope_type: {e}")))?;
    let scope_type = ConnConfScope::parse(&scope_raw).ok_or_else(|| {
        AppError::Database(format!(
            "connconf_assignment.scope_type has unknown value '{scope_raw}'"
        ))
    })?;
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("connconf_assignment.id: {e}")))?;
    let record_id: Uuid = row
        .try_get("record_id")
        .map_err(|e| AppError::Database(format!("connconf_assignment.record_id: {e}")))?;
    let scope_id: Option<Uuid> = row
        .try_get("scope_id")
        .map_err(|e| AppError::Database(format!("connconf_assignment.scope_id: {e}")))?;
    let weight: i32 = row
        .try_get("weight")
        .map_err(|e| AppError::Database(format!("connconf_assignment.weight: {e}")))?;
    let enabled: bool = row
        .try_get("enabled")
        .map_err(|e| AppError::Database(format!("connconf_assignment.enabled: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("connconf_assignment.created_at: {e}")))?;
    Ok(ConnConfAssignment {
        id,
        record_id,
        scope_type,
        scope_id,
        weight,
        enabled,
        created_at,
    })
}

/// Classify a write failure on `connconf_assignment`. Retargeting an
/// assignment's scope can collide with the `uq_connconf_assignment`
/// `(record_id, scope_type, scope_id)` unique index — surface that as a 409
/// Conflict instead of a generic 500. Everything else stays a database error.
fn map_assignment_write_error(error: SqlxError) -> AppError {
    if let SqlxError::Database(db_error) = &error
        && db_error.is_unique_violation()
    {
        return AppError::Conflict(
            "an assignment for this record and scope already exists".to_string(),
        );
    }
    AppError::Database(format!("update connconf_assignment: {error}"))
}

#[async_trait]
impl ConnConfAssignmentRepository for SqlxConnConfAssignmentRepository {
    async fn list(
        &self,
        filter: ConnConfAssignmentFilter,
    ) -> Result<Vec<ConnConfAssignment>, AppError> {
        let mut builder = QueryBuilder::new("SELECT ");
        builder.push(SELECT_COLUMNS);
        builder.push(" FROM connconf_assignment");
        let mut where_state = WhereState::new();
        push_filters(&mut builder, &filter, &mut where_state);
        push_order_by(
            &mut builder,
            CONNCONF_ASSIGNMENT_COLUMNS,
            filter.sort.as_ref(),
            "record_id, id",
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("list connconf_assignment: {e}")))?;
        rows.into_iter().map(map_row).collect()
    }

    async fn list_all(&self) -> Result<Vec<ConnConfAssignment>, AppError> {
        self.list(ConnConfAssignmentFilter::default()).await
    }

    async fn list_for_record(&self, record_id: Uuid) -> Result<Vec<ConnConfAssignment>, AppError> {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM connconf_assignment \
             WHERE record_id = $1 ORDER BY scope_type, weight DESC"
        );
        let rows = sqlx::query(&query)
            .bind(record_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("list_for_record connconf_assignment: {e}")))?;
        rows.into_iter().map(map_row).collect()
    }

    async fn find(&self, id: Uuid) -> Result<Option<ConnConfAssignment>, AppError> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM connconf_assignment WHERE id = $1");
        let row = sqlx::query(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("find connconf_assignment: {e}")))?;
        row.map(map_row).transpose()
    }

    async fn create(&self, input: &NewConnConfAssignment) -> Result<ConnConfAssignment, AppError> {
        let query = format!(
            "INSERT INTO connconf_assignment \
             (record_id, scope_type, scope_id, weight, enabled) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(input.record_id)
            .bind(input.scope_type.as_str())
            .bind(input.scope_id)
            .bind(input.weight.unwrap_or(0))
            .bind(input.enabled.unwrap_or(true))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("create connconf_assignment: {e}")))?;
        map_row(row)
    }

    async fn update(
        &self,
        id: Uuid,
        input: &UpdateConnConfAssignment,
    ) -> Result<Option<ConnConfAssignment>, AppError> {
        // `$4` carries the new scope_type as text (NULL = leave scope alone). When
        // it is provided, both scope columns are rewritten ($4 + $5); the
        // `global ⟺ scope_id IS NULL` invariant is validated upstream and locked
        // by the table CHECK. The COALESCE guard keeps weight/enabled partial.
        let new_scope_type = input.scope_type.map(|s| s.as_str().to_string());
        let query = format!(
            "UPDATE connconf_assignment SET \
               weight = COALESCE($2, weight), \
               enabled = COALESCE($3, enabled), \
               scope_type = COALESCE($4, scope_type), \
               scope_id = CASE WHEN $4 IS NULL THEN scope_id ELSE $5 END \
             WHERE id = $1 \
             RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(id)
            .bind(input.weight)
            .bind(input.enabled)
            .bind(new_scope_type)
            .bind(input.scope_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_assignment_write_error)?;
        row.map(map_row).transpose()
    }

    async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        let rows = sqlx::query("DELETE FROM connconf_assignment WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("delete connconf_assignment: {e}")))?;
        Ok(rows.rows_affected() > 0)
    }

    async fn list_for_scopes(
        &self,
        scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<Vec<ConnConfAssignment>, AppError> {
        // Build a VALUES-driven match set so we resolve all layers in one
        // round-trip. `global` rows carry NULL scope_id; the COALESCE-to-nil
        // pattern matches the unique-index convention from the migration.
        let nil = "00000000-0000-0000-0000-000000000000";
        let mut params: Vec<(String, Uuid)> = Vec::with_capacity(scopes.len());
        for (scope_type, scope_id) in scopes {
            let id = scope_id.unwrap_or_else(Uuid::nil);
            params.push((scope_type.as_str().to_string(), id));
        }

        // Bind each (scope_type, scope_id) pair positionally.
        let mut clauses: Vec<String> = Vec::with_capacity(params.len());
        for (i, _) in params.iter().enumerate() {
            let t = i * 2 + 1;
            let v = i * 2 + 2;
            clauses.push(format!(
                "(scope_type = ${t} AND COALESCE(scope_id, '{nil}') = ${v})"
            ));
        }
        if clauses.is_empty() {
            return Ok(Vec::new());
        }
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM connconf_assignment \
             WHERE enabled AND ({})",
            clauses.join(" OR ")
        );
        let mut q = sqlx::query(&query);
        for (scope_type, scope_id) in &params {
            q = q.bind(scope_type).bind(*scope_id);
        }
        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("list_for_scopes connconf_assignment: {e}")))?;
        rows.into_iter().map(map_row).collect()
    }

    async fn get_device_scopes(&self, target_id: Uuid) -> Result<Option<DeviceScopeIds>, AppError> {
        // Device path: pull direct FKs and derive region/site_group from the
        // device's site. VM path: VMs carry site/tenant/role/cluster only.
        let row = sqlx::query(
            r#"
            SELECT
                s.region_id        AS region_id,
                s.group_id         AS site_group_id,
                d.site_id          AS site_id,
                d.location_id      AS location_id,
                d.rack_id          AS rack_id,
                d.tenant_id        AS tenant_id,
                d.cluster_id       AS cluster_id,
                d.role_id          AS role_id,
                d.device_type_id   AS device_type_id,
                d.platform_id      AS platform_id,
                d.id               AS device_id,
                NULL::uuid         AS vm_id
            FROM dcim_device d
            LEFT JOIN dcim_site s ON s.id = d.site_id
            WHERE d.id = $1
            UNION ALL
            SELECT
                s.region_id        AS region_id,
                s.group_id         AS site_group_id,
                v.site_id          AS site_id,
                NULL::uuid         AS location_id,
                NULL::uuid         AS rack_id,
                v.tenant_id        AS tenant_id,
                v.cluster_id       AS cluster_id,
                v.role_id          AS role_id,
                NULL::uuid         AS device_type_id,
                v.platform_id      AS platform_id,
                NULL::uuid         AS device_id,
                v.id               AS vm_id
            FROM virtualization_virtualmachine v
            LEFT JOIN dcim_site s ON s.id = v.site_id
            WHERE v.id = $1
            LIMIT 1
            "#,
        )
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("get_device_scopes: {e}")))?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(decode_device_scope_ids(|name| col(&row, name))?))
    }
}

#[cfg(test)]
mod tests {
    use super::decode_device_scope_ids;
    use crate::errors::AppError;

    #[test]
    fn absent_scope_ids_remain_none_but_failed_scope_reads_are_errors() {
        let empty = decode_device_scope_ids(|_| Ok(None)).expect("nullable scopes may be absent");
        assert!(empty.region_id.is_none());
        assert!(empty.tenant_id.is_none());

        let failed = decode_device_scope_ids(|name| {
            if name == "tenant_id" {
                Err(AppError::Database("tenant scope decode failed".to_string()))
            } else {
                Ok(None)
            }
        });
        assert!(
            failed.is_err(),
            "a failed scope read must not erase ancestry and change assignment resolution"
        );
    }
}
