//! Inventory discovery repository.
//!
//! Returns the unified host view (devices + virtual machines) used by monitoring
//! and service-discovery endpoints. Keeps the cross-table query inside the
//! inventory context and off the handler layer.

use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use super::super::visibility_sql::{VisibilityColumns, push_visibility_and};
use crate::contexts::access_control::domain::VisibilityPredicate;
use crate::contexts::inventory::domain::{
    DiscoveryHostFilters, DiscoveryHostRow, DiscoveryInterfaceAddress,
};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

/// Facet columns of the discovery `hosts` CTE for the row-visibility predicate.
/// `visibility_site_id` is the row's RAW site (a VM's own
/// `site_id`, no cluster fallback) so the filter decides exactly like the
/// per-target enforcement sites and the device/VM list read-models; the
/// display-oriented `h.site_id` keeps its cluster fallback.
const DISCOVERY_VISIBILITY_COLUMNS: VisibilityColumns = VisibilityColumns {
    id: "h.host_id",
    role_id: "h.role_id",
    site_id: "h.visibility_site_id",
    region_id: "(SELECT region_site.region_id FROM dcim_site region_site WHERE region_site.id = h.visibility_site_id)",
    tenant_id: "h.tenant_id",
    platform_id: "h.platform_id",
    cluster_id: "h.cluster_id",
    status: "h.status",
};

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait DiscoveryRepository: Send + Sync {
    /// List the unified host view. `visibility` is the caller's compiled
    /// row-visibility predicate; `None` means an unfiltered,
    /// trusted-infrastructure read (vmagent HTTP SD, the list-labels poller,
    /// per-target flows already gated by `Read × CONNECTION_TARGET`).
    async fn list_discovery_hosts(
        &self,
        filters: &DiscoveryHostFilters,
        visibility: Option<&VisibilityPredicate>,
    ) -> Result<Vec<DiscoveryHostRow>, AppError>;

    /// Filter caller-supplied host ids down to the ones `visibility` admits —
    /// the row-level gate for id-keyed batch reads (host labels). Ids that do
    /// not resolve to a device/VM are dropped (fail-closed).
    async fn list_visible_host_ids(
        &self,
        host_ids: &[Uuid],
        visibility: &VisibilityPredicate,
    ) -> Result<Vec<Uuid>, AppError>;
}

#[derive(Clone)]
pub struct SqlxDiscoveryRepository {
    pool: PgPool,
}

impl SqlxDiscoveryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl DiscoveryRepository for SqlxDiscoveryRepository {
    async fn list_discovery_hosts(
        &self,
        filters: &DiscoveryHostFilters,
        visibility: Option<&VisibilityPredicate>,
    ) -> Result<Vec<DiscoveryHostRow>, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            WITH hosts AS (
                SELECT
                    d.id AS host_id,
                    COALESCE(NULLIF(TRIM(d.name), ''), d.id::text) AS host_name,
                    COALESCE(
                        NULLIF(split_part(ip4.address::text, '/', 1), ''),
                        NULLIF(split_part(ip6.address::text, '/', 1), ''),
                        COALESCE(NULLIF(TRIM(d.name), ''), d.id::text)
                    ) AS host_address,
                    'device'::text AS kind,
                    d.site_id AS site_id,
                    d.site_id AS visibility_site_id,
                    d.tenant_id AS tenant_id,
                    d.role_id AS role_id,
                    d.rack_id AS rack_id,
                    d.cluster_id AS cluster_id,
                    d.status AS status,
                    d.platform_id AS platform_id,
                    d.device_type_id AS device_type_id,
                    NULLIF(TRIM(d.serial), '') AS serial,
                    NULLIF(TRIM(d.asset_tag), '') AS asset_tag,
                    NULLIF(TRIM(d.description), '') AS description,
                    NULLIF(TRIM(d.comments), '') AS comments,
                    NULLIF(split_part(ip4.address::text, '/', 1), '') AS primary_ip4,
                    NULLIF(split_part(ip6.address::text, '/', 1), '') AS primary_ip6,
                    d.local_context_data AS local_context_data,
                    d.custom_field_data AS custom_field_data
                FROM dcim_device d
                LEFT JOIN ipam_ipaddress ip4 ON ip4.id = d.primary_ip4_id
                LEFT JOIN ipam_ipaddress ip6 ON ip6.id = d.primary_ip6_id

                UNION ALL

                SELECT
                    vm.id AS host_id,
                    COALESCE(NULLIF(TRIM(vm.name), ''), vm.id::text) AS host_name,
                    COALESCE(
                        NULLIF(split_part(ip4.address::text, '/', 1), ''),
                        NULLIF(split_part(ip6.address::text, '/', 1), ''),
                        COALESCE(NULLIF(TRIM(vm.name), ''), vm.id::text)
                    ) AS host_address,
                    'virtual_machine'::text AS kind,
                    -- A VM's effective site is its own site_id, falling back to
                    -- the denormalized site of its cluster (NetBox scope→site).
                    COALESCE(vm.site_id, vc._site_id) AS site_id,
                    vm.site_id AS visibility_site_id,
                    vm.tenant_id AS tenant_id,
                    vm.role_id AS role_id,
                    NULL::uuid AS rack_id,
                    vm.cluster_id AS cluster_id,
                    vm.status AS status,
                    vm.platform_id AS platform_id,
                    NULL::uuid AS device_type_id,
                    NULLIF(TRIM(vm.serial), '') AS serial,
                    NULL::text AS asset_tag,
                    NULLIF(TRIM(vm.description), '') AS description,
                    NULLIF(TRIM(vm.comments), '') AS comments,
                    NULLIF(split_part(ip4.address::text, '/', 1), '') AS primary_ip4,
                    NULLIF(split_part(ip6.address::text, '/', 1), '') AS primary_ip6,
                    vm.local_context_data AS local_context_data,
                    vm.custom_field_data AS custom_field_data
                FROM virtualization_virtualmachine vm
                LEFT JOIN ipam_ipaddress ip4 ON ip4.id = vm.primary_ip4_id
                LEFT JOIN ipam_ipaddress ip6 ON ip6.id = vm.primary_ip6_id
                LEFT JOIN virtualization_cluster vc ON vc.id = vm.cluster_id
            )
            SELECT
                h.host_id,
                h.host_name,
                h.host_address,
                h.kind,
                h.site_id,
                s.name AS site_name,
                s.slug AS site_slug,
                h.tenant_id,
                t.name AS tenant_name,
                t.slug AS tenant_slug,
                h.role_id,
                r.name AS role_name,
                r.slug AS role_slug,
                NULLIF(lower(trim(h.status)), '') AS status,
                pl.name AS platform_name,
                dt.model AS model,
                mf.name AS manufacturer,
                h.serial,
                h.asset_tag,
                h.primary_ip4,
                h.primary_ip6,
                h.description,
                h.comments,
                NULLIF(TRIM(s.facility), '') AS site_facility,
                NULLIF(TRIM(s.physical_address), '') AS site_address,
                rg.name AS site_region,
                sg.name AS site_group,
                tg.name AS tenant_group,
                rk.name AS rack,
                cl.name AS cluster,
                COALESCE(
                    (
                        SELECT string_agg(iface.name || '=' || iface.ip, ',')
                        FROM (
                            SELECT i.name AS name, split_part(ia.address::text, '/', 1) AS ip
                            FROM dcim_interface i
                            JOIN ipam_ipaddress ia ON ia.assigned_object_id = i.id
                                AND ia.assigned_object_type_id = (
                                    SELECT id FROM inventory_content_type
                                    WHERE app_label = 'dcim' AND model = 'interface'
                                )
                            WHERE i.device_id = h.host_id
                              AND i.enabled = TRUE
                            ORDER BY i.name
                            LIMIT 32
                        ) AS iface
                    ),
                    (
                        SELECT string_agg(vif.name || '=' || vif.ip, ',')
                        FROM (
                            SELECT vi.name AS name, split_part(ia.address::text, '/', 1) AS ip
                            FROM virtualization_vminterface vi
                            JOIN ipam_ipaddress ia ON ia.assigned_object_id = vi.id
                                AND ia.assigned_object_type_id = (
                                    SELECT id FROM inventory_content_type
                                    WHERE app_label = 'virtualization' AND model = 'vminterface'
                                )
                            WHERE vi.virtual_machine_id = h.host_id
                              AND vi.enabled = TRUE
                            ORDER BY vi.name
                            LIMIT 32
                        ) AS vif
                    )
                ) AS interfaces,
                COALESCE(
                    (
                        SELECT jsonb_agg(
                            jsonb_build_object('name', iface.name, 'address', iface.ip)
                            ORDER BY iface.name, iface.ip
                        )
                        FROM (
                            SELECT i.name AS name, split_part(ia.address::text, '/', 1) AS ip
                            FROM dcim_interface i
                            JOIN ipam_ipaddress ia ON ia.assigned_object_id = i.id
                                AND ia.assigned_object_type_id = (
                                    SELECT id FROM inventory_content_type
                                    WHERE app_label = 'dcim' AND model = 'interface'
                                )
                            WHERE i.device_id = h.host_id
                              AND i.enabled = TRUE
                              AND NULLIF(split_part(ia.address::text, '/', 1), '') IS NOT NULL
                            ORDER BY i.name, ia.address
                            LIMIT 32
                        ) AS iface
                    ),
                    (
                        SELECT jsonb_agg(
                            jsonb_build_object('name', vif.name, 'address', vif.ip)
                            ORDER BY vif.name, vif.ip
                        )
                        FROM (
                            SELECT vi.name AS name, split_part(ia.address::text, '/', 1) AS ip
                            FROM virtualization_vminterface vi
                            JOIN ipam_ipaddress ia ON ia.assigned_object_id = vi.id
                                AND ia.assigned_object_type_id = (
                                    SELECT id FROM inventory_content_type
                                    WHERE app_label = 'virtualization' AND model = 'vminterface'
                                )
                            WHERE vi.virtual_machine_id = h.host_id
                              AND vi.enabled = TRUE
                              AND NULLIF(split_part(ia.address::text, '/', 1), '') IS NOT NULL
                            ORDER BY vi.name, ia.address
                            LIMIT 32
                        ) AS vif
                    ),
                    '[]'::jsonb
                ) AS interface_addresses,
                COALESCE(
                    (
                        SELECT jsonb_agg(
                            jsonb_build_object(
                                'name', tun.name,
                                'address', tun.ip,
                                'channel_address', tun.channel_address
                            )
                            ORDER BY tun.name, tun.ip
                        )
                        FROM (
                            -- Channel label comes from the tunnel termination role
                            -- (e.g. main/reserve), so the tunnel name itself is free
                            -- to be a human-readable site label.
                            SELECT
                                tt.role AS name,
                                split_part(ip.address::text, '/', 1) AS ip,
                                (
                                    SELECT split_part(chip.address::text, '/', 1)
                                    FROM ipam_ipaddress chip
                                    WHERE chip.assigned_object_id = i.id
                                      AND chip.assigned_object_type_id = (
                                          SELECT id FROM inventory_content_type
                                          WHERE app_label = 'dcim' AND model = 'interface'
                                      )
                                      AND NULLIF(split_part(chip.address::text, '/', 1), '') IS NOT NULL
                                    ORDER BY chip.address
                                    LIMIT 1
                                ) AS channel_address
                            FROM dcim_interface i
                            JOIN vpn_tunneltermination tt
                                ON tt.termination_id = i.id
                               AND tt.termination_type_id = (
                                    SELECT id FROM inventory_content_type
                                    WHERE app_label = 'dcim' AND model = 'interface'
                                )
                            JOIN ipam_ipaddress ip ON ip.id = tt.outside_ip_id
                            WHERE h.kind = 'device'
                              AND i.device_id = h.host_id
                              AND i.enabled = TRUE
                              AND NULLIF(split_part(ip.address::text, '/', 1), '') IS NOT NULL
                            ORDER BY tt.role, ip.address
                            LIMIT 64
                        ) AS tun
                    ),
                    '[]'::jsonb
                ) AS tunnel_addresses,
                (
                    -- NetBox contacts are polymorphic assignments
                    -- (tenancy_contactassignment) keyed by (object_id, object_type_id)
                    -- where object_type_id -> inventory_content_type. We must match the
                    -- RIGHT object kind, not just any object sharing a UUID, and we
                    -- surface inheritance from the host's site and tenant.
                    SELECT string_agg(
                        formatted,
                        ', '
                        ORDER BY level_rank, contact_label
                    )
                    FROM (
                        SELECT DISTINCT ON (contact_id, role_id, level_rank)
                            level_rank,
                            -- "Name (Role · LEVEL) PHONE <email>"
                            concat_ws(' ',
                                contact_label,
                                CASE WHEN role_label IS NOT NULL
                                     THEN '(' || role_label || ' · ' || level_label || ')'
                                     ELSE '(' || level_label || ')' END,
                                CASE WHEN phone_label IS NOT NULL
                                      AND phone_label <> COALESCE(contact_label, '')
                                     THEN phone_label
                                     ELSE NULL END,
                                CASE WHEN email_label IS NOT NULL
                                     THEN '<' || email_label || '>'
                                     ELSE NULL END
                            ) AS formatted,
                            contact_label
                        FROM (
                            SELECT
                                ca.contact_id AS contact_id,
                                ca.role_id AS role_id,
                                -- device first, then site, then tenant
                                CASE level
                                    WHEN 'device' THEN 0
                                    WHEN 'site' THEN 1
                                    ELSE 2
                                END AS level_rank,
                                CASE level
                                    WHEN 'device' THEN 'устройство'
                                    WHEN 'site' THEN 'площадка'
                                    ELSE 'тенант'
                                END AS level_label,
                                NULLIF(TRIM(c.name), '') AS contact_label,
                                NULLIF(TRIM(cr.name), '') AS role_label,
                                NULLIF(TRIM(c.phone), '') AS phone_label,
                                NULLIF(TRIM(c.email), '') AS email_label
                            FROM (
                                -- DIRECT: device-assigned contacts
                                SELECT ca.contact_id, ca.role_id, 'device'::text AS level
                                FROM tenancy_contactassignment ca
                                JOIN inventory_content_type ct ON ct.id = ca.object_type_id
                                WHERE ct.app_label = 'dcim'
                                  AND ct.model = 'device'
                                  AND h.kind = 'device'
                                  AND ca.object_id = h.host_id

                                UNION ALL

                                -- DIRECT: virtual-machine-assigned contacts
                                SELECT ca.contact_id, ca.role_id, 'device'::text AS level
                                FROM tenancy_contactassignment ca
                                JOIN inventory_content_type ct ON ct.id = ca.object_type_id
                                WHERE ct.app_label = 'virtualization'
                                  AND ct.model = 'virtualmachine'
                                  AND h.kind = 'virtual_machine'
                                  AND ca.object_id = h.host_id

                                UNION ALL

                                -- INHERITED: site-assigned contacts
                                SELECT ca.contact_id, ca.role_id, 'site'::text AS level
                                FROM tenancy_contactassignment ca
                                JOIN inventory_content_type ct ON ct.id = ca.object_type_id
                                WHERE ct.app_label = 'dcim'
                                  AND ct.model = 'site'
                                  AND ca.object_id = h.site_id

                                UNION ALL

                                -- INHERITED: tenant-assigned contacts
                                SELECT ca.contact_id, ca.role_id, 'tenant'::text AS level
                                FROM tenancy_contactassignment ca
                                JOIN inventory_content_type ct ON ct.id = ca.object_type_id
                                WHERE ct.app_label = 'tenancy'
                                  AND ct.model = 'tenant'
                                  AND ca.object_id = h.tenant_id
                            ) ca
                            JOIN tenancy_contact c ON c.id = ca.contact_id
                            JOIN tenancy_contactrole cr ON cr.id = ca.role_id
                            LIMIT 64
                        ) labelled
                        ORDER BY contact_id, role_id, level_rank
                    ) deduped
                ) AS contacts,
                h.local_context_data,
                h.custom_field_data
            FROM hosts h
            LEFT JOIN dcim_site s ON s.id = h.site_id
            LEFT JOIN tenancy_tenant t ON t.id = h.tenant_id
            LEFT JOIN dcim_devicerole r ON r.id = h.role_id
            LEFT JOIN dcim_platform pl ON pl.id = h.platform_id
            LEFT JOIN dcim_devicetype dt ON dt.id = h.device_type_id
            LEFT JOIN dcim_manufacturer mf ON mf.id = dt.manufacturer_id
            LEFT JOIN dcim_region rg ON rg.id = s.region_id
            LEFT JOIN dcim_sitegroup sg ON sg.id = s.group_id
            LEFT JOIN tenancy_tenantgroup tg ON tg.id = t.group_id
            LEFT JOIN dcim_rack rk ON rk.id = h.rack_id
            LEFT JOIN virtualization_cluster cl ON cl.id = h.cluster_id
            WHERE TRUE"#,
        );
        if let Some(kind) = filters.kind.as_deref() {
            builder.push(" AND h.kind = ");
            builder.push_bind(kind);
        }
        if let Some(site_id) = filters.site_id {
            builder.push(" AND h.site_id = ");
            builder.push_bind(site_id);
        }
        if let Some(tenant_id) = filters.tenant_id {
            builder.push(" AND h.tenant_id = ");
            builder.push_bind(tenant_id);
        }
        if let Some(role_id) = filters.role_id {
            builder.push(" AND h.role_id = ");
            builder.push_bind(role_id);
        }
        // An explicit status filter wins; otherwise the settings-derived
        // allow-list (when present) constrains the set.
        if let Some(status) = filters.status.as_deref() {
            builder.push(" AND lower(COALESCE(h.status, '')) = lower(");
            builder.push_bind(status);
            builder.push(")");
        } else if let Some(allowed) = &filters.allowed_statuses {
            builder.push(" AND lower(COALESCE(h.status, '')) = ANY(");
            builder.push_bind(allowed);
            builder.push(")");
        }
        if let Some(search) = filters.search.as_deref() {
            builder.push(" AND (h.host_name ILIKE '%' || ");
            builder.push_bind(search);
            builder.push(" || '%' OR h.host_address ILIKE '%' || ");
            builder.push_bind(search);
            builder.push(" || '%')");
        }
        if let Some(host_id) = filters.host_id {
            builder.push(" AND h.host_id = ");
            builder.push_bind(host_id);
        }
        if let Some(site_status) = filters.site_status.as_deref() {
            builder.push(" AND lower(COALESCE(s.status, '')) = lower(");
            builder.push_bind(site_status);
            builder.push(")");
        } else if let Some(allowed) = &filters.allowed_site_statuses {
            builder.push(" AND lower(COALESCE(s.status, '')) = ANY(");
            builder.push_bind(allowed);
            builder.push(")");
        }
        if let Some(predicate) = visibility {
            push_visibility_and(&mut builder, predicate, &DISCOVERY_VISIBILITY_COLUMNS);
        }
        builder.push(" ORDER BY h.host_name, h.host_id LIMIT ");
        builder.push_bind(i64::from(filters.fetch_limit));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(filters.fetch_offset));

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::DatabaseSource)?;

        // Both UNION arms are column-aligned (the VM arm fills the gaps with
        // typed NULLs), so this is one projection: every read goes through
        // `col` and a drift fails with the column name instead of decaying to
        // silent None.
        rows.into_iter()
            .map(|row| {
                Ok(DiscoveryHostRow {
                    host_id: col(&row, "host_id")?,
                    host_name: col(&row, "host_name")?,
                    host_address: col(&row, "host_address")?,
                    kind: col(&row, "kind")?,
                    site_id: col(&row, "site_id")?,
                    site_name: col(&row, "site_name")?,
                    site_slug: col(&row, "site_slug")?,
                    tenant_id: col(&row, "tenant_id")?,
                    tenant_name: col(&row, "tenant_name")?,
                    tenant_slug: col(&row, "tenant_slug")?,
                    role_id: col(&row, "role_id")?,
                    role_name: col(&row, "role_name")?,
                    role_slug: col(&row, "role_slug")?,
                    status: col(&row, "status")?,
                    platform_name: col(&row, "platform_name")?,
                    model: col(&row, "model")?,
                    manufacturer: col(&row, "manufacturer")?,
                    serial: col(&row, "serial")?,
                    asset_tag: col(&row, "asset_tag")?,
                    primary_ip4: col(&row, "primary_ip4")?,
                    primary_ip6: col(&row, "primary_ip6")?,
                    interfaces: col(&row, "interfaces")?,
                    interface_addresses: parse_interface_addresses(col(
                        &row,
                        "interface_addresses",
                    )?)?,
                    tunnel_addresses: parse_interface_addresses(col(&row, "tunnel_addresses")?)?,
                    contacts: col(&row, "contacts")?,
                    description: col(&row, "description")?,
                    comments: col(&row, "comments")?,
                    site_facility: col(&row, "site_facility")?,
                    site_address: col(&row, "site_address")?,
                    site_region: col(&row, "site_region")?,
                    site_group: col(&row, "site_group")?,
                    tenant_group: col(&row, "tenant_group")?,
                    rack: col(&row, "rack")?,
                    cluster: col(&row, "cluster")?,
                    local_context_data: col(&row, "local_context_data")?,
                    custom_field_data: col(&row, "custom_field_data")?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn list_visible_host_ids(
        &self,
        host_ids: &[Uuid],
        visibility: &VisibilityPredicate,
    ) -> Result<Vec<Uuid>, AppError> {
        if host_ids.is_empty() || visibility.is_never() {
            return Ok(Vec::new());
        }
        if visibility.is_always() {
            return Ok(host_ids.to_vec());
        }
        // Lean id+facets projection over the same device/VM union as the full
        // discovery view — one round-trip instead of per-id lookups. The column
        // names deliberately mirror `DISCOVERY_VISIBILITY_COLUMNS`.
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            WITH hosts AS (
                SELECT
                    d.id AS host_id,
                    d.role_id AS role_id,
                    d.site_id AS visibility_site_id,
                    d.tenant_id AS tenant_id,
                    d.platform_id AS platform_id,
                    d.cluster_id AS cluster_id,
                    d.status AS status
                FROM dcim_device d
                UNION ALL
                SELECT
                    vm.id AS host_id,
                    vm.role_id AS role_id,
                    vm.site_id AS visibility_site_id,
                    vm.tenant_id AS tenant_id,
                    vm.platform_id AS platform_id,
                    vm.cluster_id AS cluster_id,
                    vm.status AS status
                FROM virtualization_virtualmachine vm
            )
            SELECT h.host_id FROM hosts h WHERE h.host_id = ANY("#,
        );
        builder.push_bind(host_ids.to_vec());
        builder.push(")");
        push_visibility_and(&mut builder, visibility, &DISCOVERY_VISIBILITY_COLUMNS);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::DatabaseSource)?;
        rows.into_iter().map(|row| col(&row, "host_id")).collect()
    }
}

fn parse_interface_addresses(
    value: serde_json::Value,
) -> Result<Vec<DiscoveryInterfaceAddress>, AppError> {
    serde_json::from_value::<Vec<DiscoveryInterfaceAddress>>(value).map_err(|error| {
        AppError::Internal(format!(
            "Invalid discovery interface address aggregate: {error}"
        ))
    })
}
