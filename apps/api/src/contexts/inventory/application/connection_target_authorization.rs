//! Batched inventory-facet resolution for connection-target authorization.
//!
//! The resolver deliberately mirrors the point-lookup path's precedence:
//! device rows win when an id exists in both target tables, then VM rows fill
//! the remaining ids. Site regions are resolved after both target queries so
//! the statement count is independent of the number of targets.

use std::collections::{HashMap, HashSet, hash_map::Entry};

use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::contexts::inventory::domain::{Device, VirtualMachine};
use crate::errors::AppError;

/// Inventory-owned policy facets for a device or virtual machine.
#[derive(Debug, Default, Clone, PartialEq, Eq, FromRow)]
pub struct ConnectionTargetAuthorizationFacets {
    pub role_id: Option<Uuid>,
    pub site_id: Option<Uuid>,
    pub region_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    pub cluster_id: Option<Uuid>,
    pub status: Option<String>,
}

/// Typed, transaction-local facets for projections that must not collapse a VM
/// and a device sharing a UUID. Uses the same canonical inventory facet fields.
pub async fn resolve_typed_connection_target_authorization_facets(
    connection: &mut sqlx::PgConnection,
    hosts: &std::collections::BTreeSet<
        crate::contexts::inventory::domain::native::dataflow::DataflowEndpoint,
    >,
) -> Result<
    std::collections::BTreeMap<
        crate::contexts::inventory::domain::native::dataflow::DataflowEndpoint,
        ConnectionTargetAuthorizationFacets,
    >,
    AppError,
> {
    use crate::contexts::inventory::domain::native::dataflow::EndpointType;
    let mut result = std::collections::BTreeMap::new();
    for host in hosts {
        let table = match host.object_type {
            EndpointType::Device => "dcim_device",
            EndpointType::VirtualMachine => "virtualization_virtualmachine",
            _ => {
                return Err(AppError::Internal(
                    "Authorization expected a typed host".into(),
                ));
            }
        };
        let row = sqlx::query_as::<_, ConnectionTargetAuthorizationFacets>(&format!("SELECT h.role_id, h.site_id, s.region_id, h.tenant_id, h.platform_id, h.cluster_id, h.status::text AS status FROM {table} h LEFT JOIN dcim_site s ON s.id = h.site_id WHERE h.id = $1"))
            .bind(host.object_id).fetch_optional(&mut *connection).await.map_err(crate::errors::classify_sqlx_error_owned)?;
        if let Some(facets) = row {
            result.insert(host.clone(), facets);
        }
    }
    Ok(result)
}

impl From<&Device> for ConnectionTargetAuthorizationFacets {
    fn from(device: &Device) -> Self {
        Self {
            role_id: device.role_id,
            site_id: device.site_id,
            region_id: None,
            tenant_id: device.tenant_id,
            platform_id: device.platform_id,
            cluster_id: device.cluster_id,
            status: device.status.clone(),
        }
    }
}

impl From<&VirtualMachine> for ConnectionTargetAuthorizationFacets {
    fn from(vm: &VirtualMachine) -> Self {
        Self {
            role_id: vm.role_id,
            site_id: vm.site_id,
            region_id: None,
            tenant_id: vm.tenant_id,
            platform_id: vm.platform_id,
            cluster_id: vm.cluster_id,
            status: vm.status.clone(),
        }
    }
}

#[derive(Debug, FromRow)]
struct TargetFacetRow {
    id: Uuid,
    role_id: Option<Uuid>,
    site_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    platform_id: Option<Uuid>,
    cluster_id: Option<Uuid>,
    status: Option<String>,
}

impl From<TargetFacetRow> for ConnectionTargetAuthorizationFacets {
    fn from(row: TargetFacetRow) -> Self {
        Self {
            role_id: row.role_id,
            site_id: row.site_id,
            region_id: None,
            tenant_id: row.tenant_id,
            platform_id: row.platform_id,
            cluster_id: row.cluster_id,
            status: row.status,
        }
    }
}

#[derive(Debug, FromRow)]
struct SiteRegionRow {
    id: Uuid,
    region_id: Option<Uuid>,
}

/// Resolve all known device/VM facets in three SQL statements regardless of
/// target count: devices, VMs, then the distinct referenced sites.
///
/// Unknown ids are absent from the returned map. Callers that historically
/// evaluated unknown targets with empty facets must continue to use
/// `unwrap_or_default()` rather than treating absence as authorization success.
pub async fn resolve_connection_target_authorization_facets(
    pool: &PgPool,
    target_ids: &[Uuid],
) -> Result<HashMap<Uuid, ConnectionTargetAuthorizationFacets>, AppError> {
    if target_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let unique_ids: Vec<Uuid> = target_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let device_rows = sqlx::query_as::<_, TargetFacetRow>(
        r#"
        SELECT id, role_id, site_id, tenant_id, platform_id, cluster_id,
               status::text AS status
        FROM dcim_device
        WHERE id = ANY($1)
        "#,
    )
    .bind(&unique_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to batch-resolve device authorization facets: {error}"
        ))
    })?;

    let vm_rows = sqlx::query_as::<_, TargetFacetRow>(
        r#"
        SELECT id, role_id, site_id, tenant_id, platform_id, cluster_id,
               status::text AS status
        FROM virtualization_virtualmachine
        WHERE id = ANY($1)
        "#,
    )
    .bind(&unique_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to batch-resolve VM authorization facets: {error}"
        ))
    })?;

    let mut facets: HashMap<Uuid, ConnectionTargetAuthorizationFacets> =
        HashMap::with_capacity(device_rows.len() + vm_rows.len());
    for row in device_rows {
        facets.insert(row.id, row.into());
    }
    for row in vm_rows {
        if let Entry::Vacant(entry) = facets.entry(row.id) {
            entry.insert(row.into());
        }
    }

    let site_ids: Vec<Uuid> = facets
        .values()
        .filter_map(|target| target.site_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if site_ids.is_empty() {
        return Ok(facets);
    }

    let site_rows = sqlx::query_as::<_, SiteRegionRow>(
        r#"
        SELECT id, region_id
        FROM dcim_site
        WHERE id = ANY($1)
        "#,
    )
    .bind(&site_ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to batch-resolve site authorization facets: {error}"
        ))
    })?;
    let regions: HashMap<Uuid, Option<Uuid>> = site_rows
        .into_iter()
        .map(|site| (site.id, site.region_id))
        .collect();

    for target in facets.values_mut() {
        target.region_id = target
            .site_id
            .and_then(|site_id| regions.get(&site_id).copied().flatten());
    }
    Ok(facets)
}
