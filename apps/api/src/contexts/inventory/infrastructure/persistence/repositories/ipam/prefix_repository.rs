//! Prefix repository — full CRUD.

use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::list_query::{
    ListColumn, SortSpec, push_id_filter, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const PREFIX_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("prefix", "p.prefix", "inventory.prefixes.columns.prefix"),
    ListColumn::enum_filter(
        "status",
        "p.status",
        "p.status",
        "inventory.prefixes.columns.status",
    ),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "p._site_id",
        "sites",
        "inventory.prefixes.columns.site",
    ),
    ListColumn::fk_filter(
        "vrf",
        "v.name",
        "vrf_id",
        "p.vrf_id",
        "vrfs",
        "inventory.prefixes.columns.vrf",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "p.tenant_id",
        "tenants",
        "inventory.prefixes.columns.tenant",
    ),
    ListColumn::enum_filter(
        "is_pool",
        "p.is_pool",
        "p.is_pool::text",
        "inventory.prefixes.columns.isPool",
    ),
    ListColumn::enum_filter(
        "mark_utilized",
        "p.mark_utilized",
        "p.mark_utilized::text",
        "inventory.prefixes.columns.markUtilized",
    ),
    ListColumn::sortable(
        "utilization",
        "utilization_percent",
        "inventory.prefixes.columns.utilization",
    ),
];

/// Minimal representation of an `ipam_prefix` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct PrefixRecord {
    pub id: Uuid,
    pub prefix: String,
    pub status: String,
    pub site_id: Option<Uuid>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub vlan_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub is_pool: bool,
    pub mark_utilized: bool,
    pub description: Option<String>,
}

/// Hydrated list row for `ipam_prefix` — includes site/vrf/tenant names resolved
/// via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct PrefixListItem {
    pub record: PrefixRecord,
    pub site_name: Option<String>,
    pub vrf_name: Option<String>,
    pub tenant_name: Option<String>,
    pub utilized_addresses: i64,
    pub usable_addresses: Option<i64>,
    pub utilization_percent: Option<f64>,
}

/// Read-model row for the native IPAM workspace tree. This is derived from
/// `ipam_prefix` and related labels; it is not a second source of truth.
#[derive(Debug, Clone)]
pub struct PrefixTreeNode {
    pub id: Uuid,
    pub node_id: String,
    pub prefix: String,
    pub parent_id: Option<Uuid>,
    pub parent_prefix: Option<String>,
    pub root_id: Option<Uuid>,
    pub root_prefix: Option<String>,
    pub depth: i32,
    pub child_count: i64,
    pub status: String,
    pub family: i32,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub vrf_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub mark_utilized: bool,
    pub utilized_addresses: i64,
    pub usable_addresses: Option<i64>,
    pub utilization_percent: Option<f64>,
    pub breadcrumbs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PrefixTreeFilter {
    pub search: Option<String>,
    pub root_id: Option<Uuid>,
    pub root_prefix: Option<String>,
    pub family: Option<i32>,
    pub site_id: Vec<Uuid>,
    pub vrf_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub status: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PrefixTreeReadModel {
    pub nodes: Vec<PrefixTreeNode>,
    pub total: i64,
    pub limit: u32,
    pub applied: PrefixTreeFilter,
}

#[derive(Debug, Clone)]
pub struct PrefixPlanningPreview {
    pub parent_id: Uuid,
    pub parent_prefix: String,
    pub requested_prefix_length: i32,
    pub status: String,
    pub candidate_prefix: Option<String>,
    pub reason: Option<String>,
    pub conflicts: Vec<String>,
    pub searched_candidates: u32,
}

/// Input for creating a new `ipam_prefix` row.
#[derive(Debug, Clone)]
pub struct NewPrefix {
    pub id: Uuid,
    pub prefix: String,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub vlan_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub is_pool: Option<bool>,
    pub mark_utilized: Option<bool>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `ipam_prefix` row.
#[derive(Debug, Clone)]
pub struct UpdatePrefix {
    pub prefix: Option<String>,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub vlan_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub is_pool: Option<bool>,
    pub mark_utilized: Option<bool>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkPrefixResult {
    pub created: Vec<PrefixRecord>,
    pub errors: Vec<String>,
}

/// Prefix list filter. Multi-value FK fields are `Vec<Uuid>` where an empty
/// vector means "no filter" and a non-empty vector emits `col IN (…)`. `status`
/// is a multi-value string filter. `sort` carries an already-allowlist-validated
/// [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct PrefixFilter {
    pub ids: Option<Vec<Uuid>>,
    pub search: Option<String>,
    pub site_id: Vec<Uuid>,
    pub vrf_id: Vec<Uuid>,
    pub role_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub is_pool: Vec<String>,
    pub mark_utilized: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait PrefixRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PrefixRecord>, AppError>;
    async fn list_hydrated(
        &self,
        filter: PrefixFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PrefixListItem>, i64), AppError>;
    async fn list_tree(
        &self,
        filter: PrefixTreeFilter,
        limit: Option<u32>,
    ) -> Result<PrefixTreeReadModel, AppError>;
    async fn preview_next_available_prefix(
        &self,
        parent_id: Uuid,
        prefix_length: i32,
    ) -> Result<PrefixPlanningPreview, AppError>;
    async fn create(&self, new_prefix: &NewPrefix) -> Result<PrefixRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePrefix,
    ) -> Result<Option<PrefixRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewPrefix]) -> Result<BulkPrefixResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxPrefixRepository {
    pool: PgPool,
}

impl SqlxPrefixRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Parse one prefix row. Both callers (get_by_id and the hydrated list)
    /// select every column this reads, so any failure is a programming error
    /// surfaced with the column name.
    fn parse_row(row: &PgRow) -> Result<PrefixRecord, AppError> {
        Ok(PrefixRecord {
            id: col(row, "id")?,
            prefix: col(row, "prefix")?,
            status: col(row, "status")?,
            site_id: col(row, "site_id")?,
            vrf_id: col(row, "vrf_id")?,
            tenant_id: col(row, "tenant_id")?,
            vlan_id: col(row, "vlan_id")?,
            role_id: col(row, "role_id")?,
            is_pool: col(row, "is_pool")?,
            mark_utilized: col(row, "mark_utilized")?,
            description: col(row, "description")?,
        })
    }

    fn parse_tree_row(row: &PgRow, root_id: Option<Uuid>) -> Result<PrefixTreeNode, AppError> {
        let id: Uuid = col(row, "id")?;
        let prefix: String = col(row, "prefix")?;
        let parent_prefix: Option<String> = col(row, "parent_prefix")?;
        // The SQL COALESCEs breadcrumbs to an empty array, so a bare Vec read
        // is the honest type — a decode error must not become an empty trail.
        let mut breadcrumbs: Vec<String> = col(row, "breadcrumbs")?;
        breadcrumbs.push(prefix.clone());
        Ok(PrefixTreeNode {
            id,
            node_id: format!("prefix:{id}"),
            prefix,
            parent_id: col(row, "parent_id")?,
            parent_prefix,
            root_id,
            root_prefix: col(row, "root_prefix")?,
            depth: col(row, "depth")?,
            child_count: col(row, "child_count")?,
            status: col(row, "status")?,
            family: col(row, "family")?,
            site_id: col(row, "site_id")?,
            site_name: col(row, "site_name")?,
            vrf_id: col(row, "vrf_id")?,
            vrf_name: col(row, "vrf_name")?,
            tenant_id: col(row, "tenant_id")?,
            tenant_name: col(row, "tenant_name")?,
            mark_utilized: col(row, "mark_utilized")?,
            utilized_addresses: col(row, "utilized_addresses")?,
            usable_addresses: col(row, "usable_addresses")?,
            utilization_percent: col(row, "utilization_percent")?,
            breadcrumbs,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv4Prefix {
    network: u32,
    length: u8,
}

impl Ipv4Prefix {
    fn parse(value: &str) -> Option<Self> {
        let (addr_raw, length_raw) = value.trim().split_once('/')?;
        let IpAddr::V4(addr) = IpAddr::from_str(addr_raw).ok()? else {
            return None;
        };
        let length = length_raw.parse::<u8>().ok()?;
        if length > 32 {
            return None;
        }
        let mask = ipv4_mask(length);
        Some(Self {
            network: u32::from(addr) & mask,
            length,
        })
    }

    fn contains(self, other: Self) -> bool {
        self.length <= other.length && (other.network & ipv4_mask(self.length)) == self.network
    }

    fn overlaps(self, other: Self) -> bool {
        self.contains(other) || other.contains(self)
    }

    fn child_at(self, length: u8, ordinal: u64) -> Option<Self> {
        if length <= self.length || length > 32 {
            return None;
        }
        let block_size = 1u64 << (32 - length);
        let offset = ordinal.checked_mul(block_size)?;
        let network = u64::from(self.network).checked_add(offset)?;
        if network > u64::from(u32::MAX) {
            return None;
        }
        let child = Self {
            network: network as u32,
            length,
        };
        self.contains(child).then_some(child)
    }
}

impl std::fmt::Display for Ipv4Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", Ipv4Addr::from(self.network), self.length)
    }
}

fn ipv4_mask(length: u8) -> u32 {
    if length == 0 {
        0
    } else {
        u32::MAX << (32 - length)
    }
}

fn preview_next_ipv4_child(
    parent: Ipv4Prefix,
    existing: &[(Uuid, Ipv4Prefix)],
    parent_id: Uuid,
    prefix_length: i32,
) -> PrefixPlanningPreview {
    let requested = prefix_length;
    let Ok(prefix_length) = u8::try_from(prefix_length) else {
        return planning_error(parent_id, parent, requested, "invalid_prefix_length");
    };
    if prefix_length <= parent.length || prefix_length > 32 {
        return planning_error(parent_id, parent, requested, "invalid_prefix_length");
    }

    let candidate_count = 1u64 << (prefix_length - parent.length);
    let max_candidates = candidate_count.min(65_536);
    for ordinal in 0..max_candidates {
        let Some(candidate) = parent.child_at(prefix_length, ordinal) else {
            continue;
        };
        let conflicts: Vec<String> = existing
            .iter()
            .filter(|(id, prefix)| *id != parent_id && prefix.overlaps(candidate))
            .map(|(_, prefix)| prefix.to_string())
            .collect();
        if conflicts.is_empty() {
            return PrefixPlanningPreview {
                parent_id,
                parent_prefix: parent.to_string(),
                requested_prefix_length: requested,
                status: "available".to_string(),
                candidate_prefix: Some(candidate.to_string()),
                reason: None,
                conflicts,
                searched_candidates: (ordinal + 1) as u32,
            };
        }
    }

    PrefixPlanningPreview {
        parent_id,
        parent_prefix: parent.to_string(),
        requested_prefix_length: requested,
        status: if max_candidates == candidate_count {
            "exhausted"
        } else {
            "window_exhausted"
        }
        .to_string(),
        candidate_prefix: None,
        reason: Some(
            if max_candidates == candidate_count {
                "no_available_prefix"
            } else {
                "candidate_space_exceeds_preview_window"
            }
            .to_string(),
        ),
        conflicts: Vec::new(),
        searched_candidates: max_candidates as u32,
    }
}

fn planning_error(
    parent_id: Uuid,
    parent: Ipv4Prefix,
    requested_prefix_length: i32,
    reason: &str,
) -> PrefixPlanningPreview {
    PrefixPlanningPreview {
        parent_id,
        parent_prefix: parent.to_string(),
        requested_prefix_length,
        status: "invalid".to_string(),
        candidate_prefix: None,
        reason: Some(reason.to_string()),
        conflicts: Vec::new(),
        searched_candidates: 0,
    }
}

impl PrefixRepository for SqlxPrefixRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PrefixRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, prefix::text AS prefix, status, _site_id AS site_id, vrf_id, tenant_id,
                   vlan_id, role_id, is_pool, mark_utilized, description
            FROM ipam_prefix
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load prefix: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_hydrated(
        &self,
        filter: PrefixFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PrefixListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT p.id, p.prefix::text AS prefix, p.status, p._site_id AS site_id, p.vrf_id, p.tenant_id,
                   p.vlan_id, p.role_id, p.is_pool, p.mark_utilized, p.description,
                   s.name AS site_name,
                   v.name AS vrf_name,
                   t.name AS tenant_name,
                   utilization.utilized_addresses,
                   capacity.usable_addresses,
                   CASE
                       WHEN capacity.usable_addresses IS NULL OR capacity.usable_addresses = 0 THEN NULL
                       ELSE LEAST(ROUND(
                           (
                               CASE
                                   WHEN p.mark_utilized THEN capacity.usable_addresses::numeric
                                   ELSE utilization.utilized_addresses::numeric
                               END
                               / capacity.usable_addresses::numeric
                           ) * 100,
                           2
                       ), 100)::float8
                   END AS utilization_percent,
                   COUNT(*) OVER() AS total_count
            FROM ipam_prefix p
            LEFT JOIN dcim_site s ON s.id = p._site_id
            LEFT JOIN ipam_vrf v ON v.id = p.vrf_id
            LEFT JOIN tenancy_tenant t ON t.id = p.tenant_id
            LEFT JOIN LATERAL (
                SELECT COUNT(DISTINCT host(ip.address))::bigint AS utilized_addresses
                FROM ipam_ipaddress ip
                WHERE ip.vrf_id IS NOT DISTINCT FROM p.vrf_id
                  AND family(ip.address) = family(p.prefix)
                  AND host(ip.address)::inet <<= p.prefix
            ) utilization ON TRUE
            LEFT JOIN LATERAL (
                SELECT CASE
                    WHEN family(p.prefix) <> 4 THEN NULL::bigint
                    WHEN p.is_pool OR masklen(p.prefix) >= 31
                        THEN POWER(2::numeric, 32 - masklen(p.prefix))::bigint
                    ELSE GREATEST(POWER(2::numeric, 32 - masklen(p.prefix))::bigint - 2, 0)
                END AS usable_addresses
            ) capacity ON TRUE
            "#,
        );

        let has_filter = filter.ids.is_some()
            || filter.search.is_some()
            || !filter.site_id.is_empty()
            || !filter.vrf_id.is_empty()
            || !filter.role_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.is_pool.is_empty()
            || !filter.mark_utilized.is_empty()
            || !filter.status.is_empty();
        if has_filter {
            builder.push(" WHERE ");
            let mut where_state = WhereState::after_where();
            push_id_filter(
                &mut builder,
                &mut where_state,
                "p.id",
                filter.ids.as_deref(),
            );
            push_search_ilike(
                &mut builder,
                &mut where_state,
                &[
                    "p.prefix",
                    "p.status",
                    "p.description",
                    "s.name",
                    "v.name",
                    "t.name",
                ],
                filter.search.as_deref(),
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "p._site_id",
                &filter.site_id,
            );
            push_uuid_in(&mut builder, &mut where_state, "p.vrf_id", &filter.vrf_id);
            push_uuid_in(&mut builder, &mut where_state, "p.role_id", &filter.role_id);
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "p.tenant_id",
                &filter.tenant_id,
            );
            push_string_in(&mut builder, &mut where_state, "p.status", &filter.status);
            push_string_in(
                &mut builder,
                &mut where_state,
                "p.is_pool::text",
                &filter.is_pool,
            );
            push_string_in(
                &mut builder,
                &mut where_state,
                "p.mark_utilized::text",
                &filter.mark_utilized,
            );
        }

        push_order_by(
            &mut builder,
            PREFIX_COLUMNS,
            filter.sort.as_ref(),
            "p.prefix ASC, p.id",
        );
        if filter.sort.is_some() {
            builder.push(", p.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list prefixes hydrated: {}", e))
            })?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read prefix total_count: {}", e)))?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(|row| {
                Ok::<PrefixListItem, AppError>(PrefixListItem {
                    record: Self::parse_row(row)?,
                    site_name: col(row, "site_name")?,
                    vrf_name: col(row, "vrf_name")?,
                    tenant_name: col(row, "tenant_name")?,
                    utilized_addresses: row.try_get("utilized_addresses").map_err(|e| {
                        AppError::Database(format!("Failed to read prefix utilized_addresses: {e}"))
                    })?,
                    usable_addresses: row.try_get("usable_addresses").map_err(|e| {
                        AppError::Database(format!("Failed to read prefix usable_addresses: {e}"))
                    })?,
                    utilization_percent: row.try_get("utilization_percent").map_err(|e| {
                        AppError::Database(format!(
                            "Failed to read prefix utilization_percent: {e}"
                        ))
                    })?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn list_tree(
        &self,
        filter: PrefixTreeFilter,
        limit: Option<u32>,
    ) -> Result<PrefixTreeReadModel, AppError> {
        let limit = clamp_limit(limit).min(500);

        let root_prefix = if let Some(root_id) = filter.root_id {
            let row = sqlx::query("SELECT prefix::text AS prefix FROM ipam_prefix WHERE id = $1")
                .bind(root_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to load root prefix: {e}")))?;
            match row {
                Some(row) => {
                    Some(row.try_get::<String, _>("prefix").map_err(|e| {
                        AppError::Database(format!("Failed to read root prefix: {e}"))
                    })?)
                }
                None => return Err(AppError::NotFound("Root prefix not found".to_string())),
            }
        } else {
            filter.root_prefix.clone()
        };

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT p.id, p.prefix::text AS prefix, p.status,
                   p._site_id AS site_id, s.name AS site_name,
                   p.vrf_id, v.name AS vrf_name,
                   p.tenant_id, t.name AS tenant_name,
                   p.mark_utilized,
                   family(p.prefix) AS family,
                   parent.id AS parent_id,
                   parent.prefix::text AS parent_prefix,
                   "#,
        );
        if let Some(root_prefix) = &root_prefix {
            builder.push_bind(root_prefix);
            builder.push("::cidr::text AS root_prefix, ");
        } else {
            builder.push("NULL::text AS root_prefix, ");
        }
        builder.push(
            r#"
                   COALESCE(parent_depth.depth, 0)::int AS depth,
                   COALESCE(children.child_count, 0)::bigint AS child_count,
                   COALESCE(crumbs.breadcrumbs, ARRAY[]::text[]) AS breadcrumbs,
                   utilization.utilized_addresses,
                   capacity.usable_addresses,
                   CASE
                       WHEN capacity.usable_addresses IS NULL OR capacity.usable_addresses = 0 THEN NULL
                       ELSE LEAST(ROUND(
                           (
                               CASE
                                   WHEN p.mark_utilized THEN capacity.usable_addresses::numeric
                                   ELSE utilization.utilized_addresses::numeric
                               END
                               / capacity.usable_addresses::numeric
                           ) * 100,
                           2
                       ), 100)::float8
                   END AS utilization_percent,
                   COUNT(*) OVER() AS total_count
            FROM ipam_prefix p
            LEFT JOIN dcim_site s ON s.id = p._site_id
            LEFT JOIN ipam_vrf v ON v.id = p.vrf_id
            LEFT JOIN tenancy_tenant t ON t.id = p.tenant_id
            LEFT JOIN LATERAL (
                SELECT pp.id, pp.prefix
                FROM ipam_prefix pp
                WHERE pp.id <> p.id
                  AND pp.vrf_id IS NOT DISTINCT FROM p.vrf_id
                  AND family(pp.prefix) = family(p.prefix)
                  AND p.prefix << pp.prefix
                ORDER BY masklen(pp.prefix) DESC, pp.prefix
                LIMIT 1
            ) parent ON TRUE
            LEFT JOIN LATERAL (
                SELECT COUNT(*)::int AS depth
                FROM ipam_prefix ancestor
                WHERE ancestor.id <> p.id
                  AND ancestor.vrf_id IS NOT DISTINCT FROM p.vrf_id
                  AND family(ancestor.prefix) = family(p.prefix)
                  AND p.prefix << ancestor.prefix
            ) parent_depth ON TRUE
            LEFT JOIN LATERAL (
                SELECT COUNT(*)::bigint AS child_count
                FROM ipam_prefix child
                WHERE child.id <> p.id
                  AND child.vrf_id IS NOT DISTINCT FROM p.vrf_id
                  AND family(child.prefix) = family(p.prefix)
                  AND child.prefix << p.prefix
                  AND NOT EXISTS (
                    SELECT 1
                    FROM ipam_prefix between_prefix
                    WHERE between_prefix.id <> p.id
                      AND between_prefix.id <> child.id
                      AND between_prefix.vrf_id IS NOT DISTINCT FROM p.vrf_id
                      AND family(between_prefix.prefix) = family(p.prefix)
                      AND child.prefix << between_prefix.prefix
                      AND between_prefix.prefix << p.prefix
                  )
            ) children ON TRUE
            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(ancestor.prefix::text ORDER BY masklen(ancestor.prefix), ancestor.prefix) AS breadcrumbs
                FROM ipam_prefix ancestor
                WHERE ancestor.id <> p.id
                  AND ancestor.vrf_id IS NOT DISTINCT FROM p.vrf_id
                  AND family(ancestor.prefix) = family(p.prefix)
                  AND p.prefix << ancestor.prefix
            ) crumbs ON TRUE
            LEFT JOIN LATERAL (
                SELECT COUNT(DISTINCT host(ip.address))::bigint AS utilized_addresses
                FROM ipam_ipaddress ip
                WHERE ip.vrf_id IS NOT DISTINCT FROM p.vrf_id
                  AND family(ip.address) = family(p.prefix)
                  AND host(ip.address)::inet <<= p.prefix
            ) utilization ON TRUE
            LEFT JOIN LATERAL (
                SELECT CASE
                    WHEN family(p.prefix) <> 4 THEN NULL::bigint
                    WHEN p.is_pool OR masklen(p.prefix) >= 31
                        THEN POWER(2::numeric, 32 - masklen(p.prefix))::bigint
                    ELSE GREATEST(POWER(2::numeric, 32 - masklen(p.prefix))::bigint - 2, 0)
                END AS usable_addresses
            ) capacity ON TRUE
            "#,
        );

        let has_filter = filter.search.is_some()
            || filter.family.is_some()
            || root_prefix.is_some()
            || !filter.site_id.is_empty()
            || !filter.vrf_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.status.is_empty();
        if has_filter {
            builder.push(" WHERE ");
            let mut where_state = WhereState::after_where();
            push_search_ilike(
                &mut builder,
                &mut where_state,
                &[
                    "p.prefix",
                    "p.status",
                    "p.description",
                    "s.name",
                    "v.name",
                    "t.name",
                ],
                filter.search.as_deref(),
            );
            if let Some(root_prefix) = &root_prefix {
                where_state.and(&mut builder);
                builder.push("p.prefix <<= ");
                builder.push_bind(root_prefix);
                builder.push("::cidr");
            }
            if let Some(family) = filter.family {
                where_state.and(&mut builder);
                builder.push("family(p.prefix) = ");
                builder.push_bind(family);
            }
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "p._site_id",
                &filter.site_id,
            );
            push_uuid_in(&mut builder, &mut where_state, "p.vrf_id", &filter.vrf_id);
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "p.tenant_id",
                &filter.tenant_id,
            );
            push_string_in(&mut builder, &mut where_state, "p.status", &filter.status);
        }

        builder.push(" ORDER BY family(p.prefix), masklen(p.prefix), p.prefix, p.id LIMIT ");
        builder.push_bind(i64::from(limit));

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list prefix tree: {e}")))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read prefix tree total: {e}")))?
            .unwrap_or(0);
        let nodes = rows
            .iter()
            .map(|row| Self::parse_tree_row(row, filter.root_id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PrefixTreeReadModel {
            nodes,
            total,
            limit,
            applied: PrefixTreeFilter {
                root_prefix,
                ..filter
            },
        })
    }

    async fn preview_next_available_prefix(
        &self,
        parent_id: Uuid,
        prefix_length: i32,
    ) -> Result<PrefixPlanningPreview, AppError> {
        let parent = self
            .get_by_id(parent_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Parent prefix not found".to_string()))?;
        let Some(parent_ipv4) = Ipv4Prefix::parse(&parent.prefix) else {
            return Ok(PrefixPlanningPreview {
                parent_id,
                parent_prefix: parent.prefix,
                requested_prefix_length: prefix_length,
                status: "unsupported".to_string(),
                candidate_prefix: None,
                reason: Some("ipv6_prefix_planning_not_calculated".to_string()),
                conflicts: Vec::new(),
                searched_candidates: 0,
            });
        };

        let rows = sqlx::query(
            r#"
            SELECT id, prefix::text AS prefix
            FROM ipam_prefix
            WHERE vrf_id IS NOT DISTINCT FROM $1
              AND family(prefix) = 4
              AND prefix <<= $2::cidr
            ORDER BY masklen(prefix), prefix, id
            LIMIT 10000
            "#,
        )
        .bind(parent.vrf_id)
        .bind(&parent.prefix)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load planning prefixes: {e}")))?;

        // A decode error here must fail the request: silently dropping a row
        // from `existing` would let the planner propose an already-occupied
        // prefix. Non-IPv4 rows staying out (parse -> None) is intended.
        let mut existing = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: Uuid = col(row, "id")?;
            let prefix: String = col(row, "prefix")?;
            if let Some(parsed) = Ipv4Prefix::parse(&prefix) {
                existing.push((id, parsed));
            }
        }
        Ok(preview_next_ipv4_child(
            parent_ipv4,
            &existing,
            parent_id,
            prefix_length,
        ))
    }

    async fn create(&self, new_prefix: &NewPrefix) -> Result<PrefixRecord, AppError> {
        let status = new_prefix
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("active")
            .to_string();
        let is_pool = new_prefix.is_pool.unwrap_or(false);
        let mark_utilized = new_prefix.mark_utilized.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO ipam_prefix (
                id, prefix, status, _site_id, vrf_id, tenant_id, vlan_id,
                role_id, is_pool, mark_utilized, description,
                custom_field_data, comments, _children, _depth, tags
            )
            VALUES ($1, $2::cidr, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                    '{}'::jsonb, '', 0, 0, '{}')
            "#,
        )
        .bind(new_prefix.id)
        .bind(&new_prefix.prefix)
        .bind(&status)
        .bind(new_prefix.site_id)
        .bind(new_prefix.vrf_id)
        .bind(new_prefix.tenant_id)
        .bind(new_prefix.vlan_id)
        .bind(new_prefix.role_id)
        .bind(is_pool)
        .bind(mark_utilized)
        .bind(new_prefix.description.as_deref().unwrap_or(""))
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create prefix: {}", e)))?;

        Ok(PrefixRecord {
            id: new_prefix.id,
            prefix: new_prefix.prefix.clone(),
            status,
            site_id: new_prefix.site_id,
            vrf_id: new_prefix.vrf_id,
            tenant_id: new_prefix.tenant_id,
            vlan_id: new_prefix.vlan_id,
            role_id: new_prefix.role_id,
            is_pool,
            mark_utilized,
            description: new_prefix.description.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePrefix,
    ) -> Result<Option<PrefixRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let prefix = changes.prefix.clone().unwrap_or(existing.prefix);
        let status = changes.status.clone().unwrap_or(existing.status);
        let site_id = if changes.site_id.is_some() {
            changes.site_id
        } else {
            existing.site_id
        };
        let vrf_id = if changes.vrf_id.is_some() {
            changes.vrf_id
        } else {
            existing.vrf_id
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let vlan_id = if changes.vlan_id.is_some() {
            changes.vlan_id
        } else {
            existing.vlan_id
        };
        let role_id = if changes.role_id.is_some() {
            changes.role_id
        } else {
            existing.role_id
        };
        let is_pool = changes.is_pool.unwrap_or(existing.is_pool);
        let mark_utilized = changes.mark_utilized.unwrap_or(existing.mark_utilized);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE ipam_prefix
            SET prefix = $2::cidr, status = $3, _site_id = $4, vrf_id = $5, tenant_id = $6,
                vlan_id = $7, role_id = $8, is_pool = $9, mark_utilized = $10, description = $11
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&prefix)
        .bind(&status)
        .bind(site_id)
        .bind(vrf_id)
        .bind(tenant_id)
        .bind(vlan_id)
        .bind(role_id)
        .bind(is_pool)
        .bind(mark_utilized)
        .bind(description.as_deref().unwrap_or(""))
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update prefix: {}", e)))?;

        Ok(Some(PrefixRecord {
            id,
            prefix,
            status,
            site_id,
            vrf_id,
            tenant_id,
            vlan_id,
            role_id,
            is_pool,
            mark_utilized,
            description,
        }))
    }

    async fn bulk_create(&self, items: &[NewPrefix]) -> Result<BulkPrefixResult, AppError> {
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
            let is_pool = item.is_pool.unwrap_or(false);
            let mark_utilized = item.mark_utilized.unwrap_or(false);

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO ipam_prefix (
                    id, prefix, status, _site_id, vrf_id, tenant_id, vlan_id,
                    role_id, is_pool, mark_utilized, description,
                    custom_field_data, comments, _children, _depth, tags
                )
                VALUES ($1, $2::cidr, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                        '{}'::jsonb, '', 0, 0, '{}')
                "#,
                )
                .bind(item.id)
                .bind(&item.prefix)
                .bind(&status)
                .bind(item.site_id)
                .bind(item.vrf_id)
                .bind(item.tenant_id)
                .bind(item.vlan_id)
                .bind(item.role_id)
                .bind(is_pool)
                .bind(mark_utilized)
                .bind(item.description.as_deref().unwrap_or("")),
            )
            .await?;

            match result {
                Ok(_) => created.push(PrefixRecord {
                    id: item.id,
                    prefix: item.prefix.clone(),
                    status,
                    site_id: item.site_id,
                    vrf_id: item.vrf_id,
                    tenant_id: item.tenant_id,
                    vlan_id: item.vlan_id,
                    role_id: item.role_id,
                    is_pool,
                    mark_utilized,
                    description: item.description.clone(),
                }),
                Err(e) => errors.push(format!("Failed to insert prefix '{}': {}", item.prefix, e)),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk prefix insert: {}", e))
        })?;

        Ok(BulkPrefixResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_prefix", id)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT_ID: Uuid = Uuid::from_u128(0xaaaaaaaa_aaaa_7aaa_8aaa_aaaaaaaaaaaa);
    const CHILD_ID: Uuid = Uuid::from_u128(0xbbbbbbbb_bbbb_7bbb_8bbb_bbbbbbbbbbbb);

    #[test]
    fn preview_next_ipv4_child_returns_first_available_child() {
        let parent = Ipv4Prefix::parse("10.0.0.0/24").expect("valid parent");

        let preview = preview_next_ipv4_child(parent, &[(PARENT_ID, parent)], PARENT_ID, 26);

        assert_eq!(preview.status, "available");
        assert_eq!(preview.candidate_prefix.as_deref(), Some("10.0.0.0/26"));
        assert_eq!(preview.searched_candidates, 1);
        assert!(preview.conflicts.is_empty());
    }

    #[test]
    fn preview_next_ipv4_child_skips_overlapping_existing_prefixes() {
        let parent = Ipv4Prefix::parse("10.0.0.0/24").expect("valid parent");
        let first_child = Ipv4Prefix::parse("10.0.0.0/26").expect("valid child");

        let preview = preview_next_ipv4_child(
            parent,
            &[(PARENT_ID, parent), (CHILD_ID, first_child)],
            PARENT_ID,
            26,
        );

        assert_eq!(preview.status, "available");
        assert_eq!(preview.candidate_prefix.as_deref(), Some("10.0.0.64/26"));
        assert_eq!(preview.searched_candidates, 2);
    }

    #[test]
    fn preview_next_ipv4_child_reports_exhausted_parent() {
        let parent = Ipv4Prefix::parse("10.0.0.0/30").expect("valid parent");
        let child_a = Ipv4Prefix::parse("10.0.0.0/31").expect("valid child");
        let child_b = Ipv4Prefix::parse("10.0.0.2/31").expect("valid child");

        let preview = preview_next_ipv4_child(
            parent,
            &[
                (PARENT_ID, parent),
                (CHILD_ID, child_a),
                (Uuid::now_v7(), child_b),
            ],
            PARENT_ID,
            31,
        );

        assert_eq!(preview.status, "exhausted");
        assert_eq!(preview.reason.as_deref(), Some("no_available_prefix"));
        assert_eq!(preview.candidate_prefix, None);
        assert_eq!(preview.searched_candidates, 2);
    }

    #[test]
    fn preview_next_ipv4_child_rejects_invalid_prefix_length() {
        let parent = Ipv4Prefix::parse("10.0.0.0/24").expect("valid parent");

        let preview = preview_next_ipv4_child(parent, &[(PARENT_ID, parent)], PARENT_ID, 24);

        assert_eq!(preview.status, "invalid");
        assert_eq!(preview.reason.as_deref(), Some("invalid_prefix_length"));
        assert_eq!(preview.candidate_prefix, None);
    }

    #[test]
    fn ipv4_prefix_containment_and_overlap_are_network_based() {
        let parent = Ipv4Prefix::parse("10.0.0.0/24").expect("valid parent");
        let child = Ipv4Prefix::parse("10.0.0.128/25").expect("valid child");
        let sibling = Ipv4Prefix::parse("10.0.1.0/24").expect("valid sibling");

        assert!(parent.contains(child));
        assert!(parent.overlaps(child));
        assert!(!parent.overlaps(sibling));
    }
}
