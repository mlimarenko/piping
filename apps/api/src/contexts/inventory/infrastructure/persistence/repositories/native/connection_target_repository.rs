// Connection target UNION read-model over `dcim_device` and
// `virtualization_virtualmachine`.
//
// This repository owns the connection-target query surface used by the
// connections workspace tree. It is a read projection, not a write-side table:
// devices and virtual machines remain the canonical inventory entities.
//
// All SQL lives here; REST handlers delegate through this read-model
// (compliance_no_handler_sql).

use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::access_control::domain::VisibilityPredicate;
use crate::contexts::device_management::domain::DeviceFamily;
use crate::contexts::inventory::infrastructure::persistence::repositories::visibility_sql::{
    VisibilityColumns, push_visibility_and,
};
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, list_sort_column, push_uuid_in,
};
use crate::errors::AppError;

/// Facet columns of the `connection_targets` UNION alias, for the row-visibility
/// predicate. Both UNION branches emit these identically.
const CONNECTION_TARGET_VISIBILITY_COLUMNS: VisibilityColumns = VisibilityColumns {
    id: "connection_targets.id",
    role_id: "connection_targets.role_id",
    site_id: "connection_targets.site_id",
    region_id: "(SELECT region_site.region_id FROM dcim_site region_site WHERE region_site.id = connection_targets.site_id)",
    tenant_id: "connection_targets.tenant_id",
    platform_id: "connection_targets.platform_id",
    cluster_id: "connection_targets.cluster_id",
    status: "connection_targets.status",
};

/// Columns are table-qualified to the `connection_targets` UNION alias. The
/// sort SQL values are compile-time literals and the ONLY thing interpolated
/// into `ORDER BY`; user input never reaches the SQL string.
pub const CONNECTION_TARGET_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "name",
        "connection_targets.name",
        "connections.targets.name",
    ),
    ListColumn::enum_filter(
        "status",
        "connection_targets.status",
        "connection_targets.status",
        "connections.targets.status",
    ),
    ListColumn::enum_filter_with_param(
        "targetKind",
        "connection_targets.target_kind",
        "target_kind",
        "connection_targets.target_kind",
        "connections.targets.targetKind",
    ),
    ListColumn::fk_filter(
        "region",
        "connection_targets.region_name",
        "region_id",
        "connection_targets.region_id",
        "Regions",
        "connections.targets.region",
    ),
    ListColumn::fk_filter(
        "site",
        "connection_targets.site_name",
        "site_id",
        "connection_targets.site_id",
        "Sites",
        "connections.targets.site",
    ),
    ListColumn::fk_filter(
        "tenant",
        "connection_targets.tenant_name",
        "tenant_id",
        "connection_targets.tenant_id",
        "Tenants",
        "connections.targets.tenant",
    ),
    ListColumn::fk_filter(
        "role",
        "connection_targets.role_name",
        "role_id",
        "connection_targets.role_id",
        "Device Roles",
        "connections.targets.role",
    ),
    ListColumn::fk_filter(
        "platform",
        "connection_targets.platform_name",
        "platform_id",
        "connection_targets.platform_id",
        "Platforms",
        "connections.targets.platform",
    ),
    ListColumn::fk_filter(
        "deviceType",
        "connection_targets.device_type_name",
        "device_type_id",
        "connection_targets.device_type_id",
        "Device Types",
        "connections.targets.deviceType",
    ),
    ListColumn::sortable(
        "primaryIp",
        "connection_targets.address",
        "connections.targets.primaryIp",
    ),
];

#[derive(Debug, Clone)]
pub struct ConnectionTarget {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub address_bound: bool,
    pub target_kind: ConnectionTargetKind,
    pub status: Option<String>,
    pub device_id: Option<Uuid>,
    pub device_name: Option<String>,
    pub vm_id: Option<Uuid>,
    pub vm_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub region_id: Option<Uuid>,
    pub region_name: Option<String>,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub role_custom_fields: Option<serde_json::Value>,
    pub platform_id: Option<Uuid>,
    pub platform_name: Option<String>,
    /// The platform's `napalm_driver` slug, verbatim (drives family derivation).
    pub platform_driver: Option<String>,
    /// Device family parsed from [`Self::platform_driver`], as its stable wire
    /// slug (e.g. `dvrip_sofia`). `None` when the driver slug is unset or is not
    /// a recognized cctv-device family.
    pub family: Option<String>,
    /// True when [`Self::family`] resolved — i.e. the target is a managed cctv
    /// device (recorder/camera) whose device-management plane is available.
    pub is_cctv: bool,
    pub device_type_id: Option<Uuid>,
    pub device_type_name: Option<String>,
    pub cluster_id: Option<Uuid>,
    pub cluster_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum ConnectionTargetKind {
    Device,
    VirtualMachine,
}

impl ConnectionTargetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionTargetKind::Device => "device",
            ConnectionTargetKind::VirtualMachine => "virtual_machine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionTargetKindFilter {
    Device,
    VirtualMachine,
}

impl ConnectionTargetKindFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "device" => Some(Self::Device),
            "virtual_machine" => Some(Self::VirtualMachine),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::VirtualMachine => "virtual_machine",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionTargetFilters {
    pub id: Option<Uuid>,
    pub search: Option<String>,
    pub name: Option<String>,
    /// Multi-value, case-insensitive `status` IN filter (matches any).
    pub status: Vec<String>,
    pub address: Option<String>,
    /// Multi-value `site_id` IN filter (matches any).
    pub site_id: Vec<Uuid>,
    /// Multi-value `region_id` IN filter (matches any).
    pub region_id: Vec<Uuid>,
    /// Multi-value `tenant_id` IN filter (matches any).
    pub tenant_id: Vec<Uuid>,
    /// Multi-value `role_id` IN filter (matches any).
    pub role_id: Vec<Uuid>,
    /// Multi-value `platform_id` IN filter (matches any).
    pub platform_id: Vec<Uuid>,
    /// Multi-value `device_type_id` IN filter (matches any).
    pub device_type_id: Vec<Uuid>,
    /// Multi-value `target_kind` IN filter (matches any).
    pub target_kind: Vec<ConnectionTargetKindFilter>,
    pub sort: Option<SortSpec>,
    /// Row-level visibility predicate compiled from the caller's
    /// `Read × CONNECTION_TARGET` policies. `None` = no filtering
    /// (caller has an unconstrained read grant); `Some(Never)` short-circuits to
    /// an empty result. Applied to BOTH the rows query and the COUNT so paging
    /// stays consistent.
    pub visibility: Option<VisibilityPredicate>,
}

#[async_trait::async_trait]
pub trait ConnectionTargetRepository: Send + Sync {
    /// List connection targets, optionally filtered by a case-insensitive substring `search`
    /// applied to the target's text columns (`name`, `address`, `device_name`,
    /// `vm_name`). An empty/whitespace-only `search` is treated as no filter by
    /// callers; `None` means no filter. The returned `total` reflects the
    /// applied filter.
    async fn list(
        &self,
        limit: i64,
        offset: i64,
        filters: ConnectionTargetFilters,
    ) -> Result<(Vec<ConnectionTarget>, i64), AppError>;
}

#[derive(Clone)]
pub struct SqlxConnectionTargetRepository {
    pool: PgPool,
}

impl SqlxConnectionTargetRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn list_with_filters(
        &self,
        limit: i64,
        offset: i64,
        filters: &ConnectionTargetFilters,
    ) -> Result<(Vec<sqlx::postgres::PgRow>, i64), AppError> {
        let base_sql = LIST_SQL;

        let mut rows_query = QueryBuilder::<Postgres>::new(base_sql);
        push_connection_target_filter_predicates(&mut rows_query, filters);
        push_connection_target_order_by(&mut rows_query, filters.sort.as_ref());
        rows_query.push(" LIMIT ");
        rows_query.push_bind(limit);
        rows_query.push(" OFFSET ");
        rows_query.push_bind(offset);
        let rows = rows_query.build().fetch_all(&self.pool).await?;

        // When the first page already returned the whole filtered set (fewer rows
        // than the requested limit at offset 0), the total is exactly what we
        // fetched — so skip the COUNT(*) entirely. That count wraps the same
        // expensive UNION read-model (per-row LATERAL primary-IP lookup), so
        // avoiding it removes roughly half the DB work on the common
        // single-fetch workspace load. Any other case still runs the exact count.
        let row_count = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let total = if offset == 0 && row_count < limit {
            row_count
        } else {
            let mut count_query = QueryBuilder::<Postgres>::new("SELECT COUNT(*) AS total FROM (");
            count_query.push(base_sql);
            push_connection_target_filter_predicates(&mut count_query, filters);
            count_query.push(") AS filtered_connection_targets");
            count_query
                .build()
                .fetch_one(&self.pool)
                .await?
                .try_get("total")?
        };

        Ok((rows, total))
    }
}

fn push_connection_target_order_by(
    builder: &mut QueryBuilder<'_, Postgres>,
    sort: Option<&SortSpec>,
) {
    builder.push(" ORDER BY ");
    match sort.and_then(|spec| {
        list_sort_column(CONNECTION_TARGET_COLUMNS, &spec.key).map(|column| (column, spec))
    }) {
        Some((column, spec)) => {
            builder.push(column);
            builder.push(spec.sql_direction());
            builder.push(", connection_targets.id");
        }
        None => {
            builder.push("connection_targets.name ASC, connection_targets.id");
        }
    }
}

fn push_connection_target_filter_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    filters: &ConnectionTargetFilters,
) {
    if let Some(search) = filters.search.as_deref() {
        let pattern = format!("%{search}%");
        builder.push(" WHERE (connection_targets.name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR connection_targets.address ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR connection_targets.device_name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR connection_targets.vm_name ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    } else {
        builder.push(" WHERE TRUE");
    }

    if let Some(id) = filters.id {
        builder.push(" AND connection_targets.id = ");
        builder.push_bind(id);
    }
    if let Some(name) = filters.name.as_deref() {
        push_ilike_filter(builder, "connection_targets.name", name);
    }
    // Multi-value, case-insensitive `status` IN filter: fold both sides to lower
    // case to preserve the prior `LOWER(status) = LOWER($1)` semantics.
    if !filters.status.is_empty() {
        builder.push(" AND LOWER(connection_targets.status) IN (");
        let mut separated = builder.separated(", ");
        for status in &filters.status {
            separated.push_bind(status.to_lowercase());
        }
        builder.push(")");
    }
    if let Some(address) = filters.address.as_deref() {
        push_ilike_filter(builder, "connection_targets.address", address);
    }
    // `site_id`/`role_id` are plain UUID columns; bind every value (never
    // interpolate) and emit `col IN ($1, $2, …)`.
    let mut where_state = WhereState::after_predicate();
    push_uuid_in(
        builder,
        &mut where_state,
        "connection_targets.region_id",
        &filters.region_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "connection_targets.site_id",
        &filters.site_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "connection_targets.tenant_id",
        &filters.tenant_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "connection_targets.role_id",
        &filters.role_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "connection_targets.platform_id",
        &filters.platform_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "connection_targets.device_type_id",
        &filters.device_type_id,
    );
    // Multi-value `target_kind` IN filter (plain text column, matches any).
    if !filters.target_kind.is_empty() {
        builder.push(" AND connection_targets.target_kind IN (");
        let mut separated = builder.separated(", ");
        for target_kind in &filters.target_kind {
            separated.push_bind(target_kind.as_str());
        }
        builder.push(")");
    }
    // Row-level visibility predicate — appended to both the rows
    // query and the COUNT (this function is the single choke point for both), so
    // the skip-COUNT shortcut stays consistent.
    if let Some(predicate) = filters.visibility.as_ref() {
        push_visibility_and(builder, predicate, &CONNECTION_TARGET_VISIBILITY_COLUMNS);
    }
}

fn push_ilike_filter(builder: &mut QueryBuilder<'_, Postgres>, column: &'static str, value: &str) {
    builder.push(" AND ");
    builder.push(column);
    builder.push(" ILIKE ");
    builder.push_bind(format!("%{value}%"));
}

const LIST_SQL: &str = r#"
WITH dev AS (
    SELECT
        d.id                         AS id,
        d.name                       AS name,
        COALESCE(
            NULLIF(split_part(ip4.address::text, '/', 1), ''),
            NULLIF(split_part(ip6.address::text, '/', 1), ''),
            fallback_ip.address
        )                            AS address,
        CASE
            WHEN NULLIF(split_part(ip4.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.dcim_interface di
                    WHERE di.device_id = d.id
                      AND di.id = ip4.assigned_object_id
                ),
                FALSE
            )
            WHEN NULLIF(split_part(ip6.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.dcim_interface di
                    WHERE di.device_id = d.id
                      AND di.id = ip6.assigned_object_id
                ),
                FALSE
            )
            ELSE fallback_ip.address IS NOT NULL
        END                          AS address_bound,
        'device'::text               AS target_kind,
        d.status::text               AS status,
        d.id                         AS device_id,
        d.name                       AS device_name,
        NULL::uuid                   AS vm_id,
        NULL::text                   AS vm_name,
        d.tenant_id                  AS tenant_id,
        t.name                       AS tenant_name,
        d.site_id                    AS site_id,
        s.name                       AS site_name,
        s.region_id                  AS region_id,
        r.name                       AS region_name,
        d.role_id                    AS role_id,
        dr.name                      AS role_name,
        dr.custom_field_data         AS role_custom_fields,
        d.platform_id                AS platform_id,
        p.name                       AS platform_name,
        p.napalm_driver              AS platform_driver,
        d.device_type_id             AS device_type_id,
        dt.model                     AS device_type_name,
        d.cluster_id                 AS cluster_id,
        c.name                       AS cluster_name
    FROM public.dcim_device d
    LEFT JOIN public.tenancy_tenant       t  ON t.id  = d.tenant_id
    LEFT JOIN public.dcim_site            s  ON s.id  = d.site_id
    LEFT JOIN public.dcim_region          r  ON r.id  = s.region_id
    LEFT JOIN public.dcim_devicerole      dr ON dr.id = d.role_id
    LEFT JOIN public.dcim_platform        p  ON p.id  = d.platform_id
    LEFT JOIN public.dcim_devicetype      dt ON dt.id = d.device_type_id
    LEFT JOIN public.virtualization_cluster c ON c.id = d.cluster_id
    LEFT JOIN public.ipam_ipaddress       ip4 ON ip4.id = d.primary_ip4_id
    LEFT JOIN public.ipam_ipaddress       ip6 ON ip6.id = d.primary_ip6_id
    LEFT JOIN LATERAL (
        SELECT host(ip.address) AS address
        FROM public.dcim_interface di
        JOIN public.ipam_ipaddress ip
          ON ip.assigned_object_id = di.id
        JOIN public.inventory_content_type ct
          ON ct.id = ip.assigned_object_type_id
         AND ct.app_label = 'dcim'
         AND ct.model = 'interface'
        WHERE di.device_id = d.id
        ORDER BY
            CASE WHEN family(ip.address) = 4 THEN 0 ELSE 1 END,
            COALESCE(di.name, ''),
            di.id,
            host(ip.address),
            masklen(ip.address),
            ip.id
        LIMIT 1
    ) fallback_ip ON TRUE
), vm AS (
    SELECT
        v.id                         AS id,
        v.name                       AS name,
        COALESCE(
            NULLIF(split_part(ip4.address::text, '/', 1), ''),
            NULLIF(split_part(ip6.address::text, '/', 1), ''),
            fallback_ip.address
        )                            AS address,
        CASE
            WHEN NULLIF(split_part(ip4.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.virtualization_vminterface vi
                    WHERE vi.virtual_machine_id = v.id
                      AND vi.id = ip4.assigned_object_id
                ),
                FALSE
            )
            WHEN NULLIF(split_part(ip6.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.virtualization_vminterface vi
                    WHERE vi.virtual_machine_id = v.id
                      AND vi.id = ip6.assigned_object_id
                ),
                FALSE
            )
            ELSE fallback_ip.address IS NOT NULL
        END                          AS address_bound,
        'virtual_machine'::text      AS target_kind,
        v.status::text               AS status,
        NULL::uuid                   AS device_id,
        NULL::text                   AS device_name,
        v.id                         AS vm_id,
        v.name                       AS vm_name,
        v.tenant_id                  AS tenant_id,
        t.name                       AS tenant_name,
        v.site_id                    AS site_id,
        s.name                       AS site_name,
        s.region_id                  AS region_id,
        r.name                       AS region_name,
        v.role_id                    AS role_id,
        dr.name                      AS role_name,
        dr.custom_field_data         AS role_custom_fields,
        v.platform_id                AS platform_id,
        p.name                       AS platform_name,
        p.napalm_driver              AS platform_driver,
        NULL::uuid                   AS device_type_id,
        NULL::text                   AS device_type_name,
        v.cluster_id                 AS cluster_id,
        c.name                       AS cluster_name
    FROM public.virtualization_virtualmachine v
    LEFT JOIN public.tenancy_tenant       t  ON t.id  = v.tenant_id
    LEFT JOIN public.dcim_site            s  ON s.id  = v.site_id
    LEFT JOIN public.dcim_region          r  ON r.id  = s.region_id
    LEFT JOIN public.dcim_devicerole      dr ON dr.id = v.role_id
    LEFT JOIN public.dcim_platform        p  ON p.id  = v.platform_id
    LEFT JOIN public.virtualization_cluster c ON c.id = v.cluster_id
    LEFT JOIN public.ipam_ipaddress       ip4 ON ip4.id = v.primary_ip4_id
    LEFT JOIN public.ipam_ipaddress       ip6 ON ip6.id = v.primary_ip6_id
    LEFT JOIN LATERAL (
        SELECT host(ip.address) AS address
        FROM public.virtualization_vminterface vi
        JOIN public.ipam_ipaddress ip
          ON ip.assigned_object_id = vi.id
        JOIN public.inventory_content_type ct
          ON ct.id = ip.assigned_object_type_id
         AND ct.app_label = 'virtualization'
         AND ct.model = 'vminterface'
        WHERE vi.virtual_machine_id = v.id
        ORDER BY
            CASE WHEN family(ip.address) = 4 THEN 0 ELSE 1 END,
            COALESCE(vi.name, ''),
            vi.id,
            host(ip.address),
            masklen(ip.address),
            ip.id
        LIMIT 1
    ) fallback_ip ON TRUE
)
SELECT * FROM (SELECT * FROM dev UNION ALL SELECT * FROM vm) AS connection_targets
"#;

/// Derive a connection target's wire family slug and whether it is a cctv
/// (video) device from the operator-set `napalm_driver` slug.
///
/// A recognized family yields its stable slug; only a *video* family sets
/// `is_cctv`. `is_cctv` gates the camera management plane and the `cctv` launch
/// protocol, so a non-video family that shares the same taxonomy (the Linux/SSH
/// host) carries a family slug but is never reported as a camera.
fn classify_target_family(platform_driver: Option<&str>) -> (Option<String>, bool) {
    let parsed = platform_driver.and_then(DeviceFamily::parse);
    let family = parsed.map(|f| f.as_str().to_string());
    let is_cctv = parsed.is_some_and(|family| family.is_video());
    (family, is_cctv)
}

#[async_trait::async_trait]
impl ConnectionTargetRepository for SqlxConnectionTargetRepository {
    async fn list(
        &self,
        limit: i64,
        offset: i64,
        filters: ConnectionTargetFilters,
    ) -> Result<(Vec<ConnectionTarget>, i64), AppError> {
        let (rows, total) = self.list_with_filters(limit, offset, &filters).await?;
        let mut items = Vec::with_capacity(rows.len());
        for r in rows {
            let kind_str: String = r.try_get("target_kind")?;
            let target_kind = if kind_str == "device" {
                ConnectionTargetKind::Device
            } else {
                ConnectionTargetKind::VirtualMachine
            };
            // Structural, domain-neutral family derivation: parse the operator-set
            // `napalm_driver` slug through the typed `DeviceFamily` taxonomy — never
            // a hardcoded SQL IN-list. A recognized *video* family means this is a
            // managed cctv device; a recognized non-video family (Linux) has a
            // family slug but is not cctv.
            let platform_driver = r.try_get::<Option<String>, _>("platform_driver")?;
            let (family, is_cctv) = classify_target_family(platform_driver.as_deref());
            items.push(ConnectionTarget {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                address: r.try_get::<Option<String>, _>("address")?,
                address_bound: r.try_get::<bool, _>("address_bound")?,
                target_kind,
                status: r.try_get::<Option<String>, _>("status")?,
                device_id: r.try_get::<Option<Uuid>, _>("device_id")?,
                device_name: r.try_get::<Option<String>, _>("device_name")?,
                vm_id: r.try_get::<Option<Uuid>, _>("vm_id")?,
                vm_name: r.try_get::<Option<String>, _>("vm_name")?,
                tenant_id: r.try_get::<Option<Uuid>, _>("tenant_id")?,
                tenant_name: r.try_get::<Option<String>, _>("tenant_name")?,
                site_id: r.try_get::<Option<Uuid>, _>("site_id")?,
                site_name: r.try_get::<Option<String>, _>("site_name")?,
                region_id: r.try_get::<Option<Uuid>, _>("region_id")?,
                region_name: r.try_get::<Option<String>, _>("region_name")?,
                role_id: r.try_get::<Option<Uuid>, _>("role_id")?,
                role_name: r.try_get::<Option<String>, _>("role_name")?,
                role_custom_fields: r
                    .try_get::<Option<serde_json::Value>, _>("role_custom_fields")?,
                platform_id: r.try_get::<Option<Uuid>, _>("platform_id")?,
                platform_name: r.try_get::<Option<String>, _>("platform_name")?,
                platform_driver,
                family,
                is_cctv,
                device_type_id: r.try_get::<Option<Uuid>, _>("device_type_id")?,
                device_type_name: r.try_get::<Option<String>, _>("device_type_name")?,
                cluster_id: r.try_get::<Option<Uuid>, _>("cluster_id")?,
                cluster_name: r.try_get::<Option<String>, _>("cluster_name")?,
            });
        }
        Ok((items, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sql_contains_all(sql: &str, markers: &[&str]) {
        for marker in markers {
            assert!(
                sql.contains(marker),
                "expected SQL predicate `{marker}` in: {sql}"
            );
        }
    }

    fn assert_sql_absent_all(sql: &str, markers: &[&str]) {
        for marker in markers {
            assert!(
                !sql.contains(marker),
                "SQL must not interpolate `{marker}`: {sql}"
            );
        }
    }

    #[test]
    fn advanced_filters_push_all_expected_predicates_without_interpolating_values() {
        let filters = ConnectionTargetFilters {
            id: Some(Uuid::from_u128(3)),
            search: Some("prod".to_string()),
            name: Some("edge".to_string()),
            status: vec!["active".to_string()],
            address: Some("10.0".to_string()),
            region_id: vec![Uuid::from_u128(4)],
            site_id: vec![Uuid::from_u128(1)],
            tenant_id: vec![Uuid::from_u128(5)],
            role_id: vec![Uuid::from_u128(2)],
            platform_id: vec![Uuid::from_u128(6)],
            device_type_id: vec![Uuid::from_u128(7)],
            target_kind: vec![ConnectionTargetKindFilter::VirtualMachine],
            sort: None,
            visibility: None,
        };
        let mut builder = QueryBuilder::<Postgres>::new("");

        push_connection_target_filter_predicates(&mut builder, &filters);
        let sql = builder.sql();

        assert_sql_contains_all(
            sql,
            &[
                "WHERE (connection_targets.name ILIKE",
                "OR connection_targets.address ILIKE",
                "OR connection_targets.device_name ILIKE",
                "OR connection_targets.vm_name ILIKE",
                "AND connection_targets.name ILIKE",
                "AND connection_targets.id =",
                "AND LOWER(connection_targets.status) IN (",
                "AND connection_targets.address ILIKE",
                "AND connection_targets.region_id IN (",
                "AND connection_targets.site_id IN (",
                "AND connection_targets.tenant_id IN (",
                "AND connection_targets.role_id IN (",
                "AND connection_targets.platform_id IN (",
                "AND connection_targets.device_type_id IN (",
                "AND connection_targets.target_kind IN (",
            ],
        );
        assert_sql_absent_all(sql, &["prod", "edge", "active", "10.0", "virtual_machine"]);
    }

    #[test]
    fn target_kind_only_filter_uses_where_true_base_predicate() {
        let filters = ConnectionTargetFilters {
            target_kind: vec![ConnectionTargetKindFilter::Device],
            ..ConnectionTargetFilters::default()
        };
        let mut builder = QueryBuilder::<Postgres>::new("");

        push_connection_target_filter_predicates(&mut builder, &filters);
        let sql = builder.sql();

        assert!(sql.starts_with(" WHERE TRUE"));
        assert!(sql.contains("AND connection_targets.target_kind IN ("));
        assert!(!sql.contains("device"));
    }

    #[test]
    fn visibility_predicate_is_appended_and_binds_its_value() {
        use crate::contexts::access_control::domain::{Facet, VisibilityPredicate};
        let filters = ConnectionTargetFilters {
            visibility: Some(VisibilityPredicate::Eq {
                facet: Facet::SiteId,
                value: "00000000-0000-7000-8000-0000000000a1".to_string(),
            }),
            ..ConnectionTargetFilters::default()
        };
        let mut builder = QueryBuilder::<Postgres>::new("");
        push_connection_target_filter_predicates(&mut builder, &filters);
        let sql = builder.sql();

        assert!(sql.starts_with(" WHERE TRUE"));
        assert!(sql.contains("AND (connection_targets.site_id::text = "));
        // The uuid value is bound ($n placeholder), never interpolated.
        assert!(!sql.contains("0000000000a1"));
    }

    #[test]
    fn list_sql_keeps_union_query_for_dynamic_filters() {
        let base_sql = LIST_SQL;

        assert!(base_sql.contains(
            "SELECT * FROM (SELECT * FROM dev UNION ALL SELECT * FROM vm) AS connection_targets"
        ));
        assert!(!base_sql.contains("LIMIT $1 OFFSET $2"));
    }

    #[test]
    fn order_by_uses_allowlisted_sort_column_and_direction() {
        let mut builder = QueryBuilder::<Postgres>::new("");
        let sort = SortSpec {
            key: "name".to_string(),
            descending: true,
        };

        push_connection_target_order_by(&mut builder, Some(&sort));
        let sql = builder.sql();

        assert_eq!(
            sql,
            " ORDER BY connection_targets.name DESC, connection_targets.id"
        );
    }

    #[test]
    fn sort_allowlist_accepts_only_visible_table_keys() {
        assert_eq!(
            list_sort_column(CONNECTION_TARGET_COLUMNS, "targetKind"),
            Some("connection_targets.target_kind")
        );
        assert_eq!(
            list_sort_column(CONNECTION_TARGET_COLUMNS, "deviceType"),
            Some("connection_targets.device_type_name")
        );
        assert_eq!(
            list_sort_column(CONNECTION_TARGET_COLUMNS, "primaryIp"),
            Some("connection_targets.address")
        );
        assert!(list_sort_column(CONNECTION_TARGET_COLUMNS, "target_kind").is_none());
        assert!(list_sort_column(CONNECTION_TARGET_COLUMNS, "device_type").is_none());
        assert!(list_sort_column(CONNECTION_TARGET_COLUMNS, "address").is_none());
    }

    #[test]
    fn order_by_defaults_to_stable_name_sort() {
        let mut builder = QueryBuilder::<Postgres>::new("");

        push_connection_target_order_by(&mut builder, None);
        let sql = builder.sql();

        assert_eq!(
            sql,
            " ORDER BY connection_targets.name ASC, connection_targets.id"
        );
    }

    #[test]
    fn a_video_family_is_cctv_but_a_linux_family_is_not() {
        assert_eq!(
            classify_target_family(Some("hikvision")),
            (Some("hikvision".to_string()), true)
        );
        // A Linux host carries the family slug for driver selection but must NOT
        // be reported as a cctv device.
        assert_eq!(
            classify_target_family(Some("linux")),
            (Some("linux".to_string()), false)
        );
        assert_eq!(classify_target_family(Some("acme")), (None, false));
        assert_eq!(classify_target_family(None), (None, false));
    }
}
