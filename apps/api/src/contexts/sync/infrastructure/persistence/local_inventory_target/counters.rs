use std::collections::BTreeSet;

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{FieldKind, InventoryEntity, PlannedRow};
use crate::errors::AppError;

/// Resolve persisted owners before and after a component-only replay so an
/// omitted Device and both sides of a component move receive correct counts.
pub(super) async fn device_owners(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
) -> Result<Vec<Uuid>, AppError> {
    let mut devices: BTreeSet<_> = planned
        .iter()
        .filter(|row| row.entity == InventoryEntity::Device)
        .map(|row| row.local_id)
        .collect();
    for entity in InventoryEntity::all() {
        let Some(table) = entity.table() else {
            continue;
        };
        let owns_device = entity.field_iter().any(|field| {
            field.column() == "device_id"
                && matches!(
                    field.kind(),
                    FieldKind::ForeignKey {
                        target: Some(InventoryEntity::Device)
                    }
                )
        });
        if !owns_device {
            continue;
        }
        let ids: Vec<_> = planned
            .iter()
            .filter(|row| row.entity == entity)
            .map(|row| row.local_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let mut query = sqlx::QueryBuilder::<Postgres>::new("SELECT DISTINCT device_id FROM ");
        query
            .push(table)
            .push(" WHERE id = ANY(")
            .push_bind(ids)
            .push(") AND device_id IS NOT NULL");
        let owners: Vec<Uuid> = query
            .build_query_scalar()
            .fetch_all(&mut **transaction)
            .await?;
        devices.extend(owners);
    }
    Ok(devices.into_iter().collect())
}

/// Rebuild inventory-derived columns from the committed graph, not remote caches.
pub(super) async fn recompute(
    transaction: &mut Transaction<'_, Postgres>,
    devices: &[Uuid],
    virtual_machines: &[Uuid],
    prefixes: &[Uuid],
) -> Result<(), AppError> {
    crate::contexts::inventory::infrastructure::persistence::repositories::dcim::hardware_graph::recount_device_components(transaction, devices).await?;
    if !virtual_machines.is_empty() {
        sqlx::query!(
            r#"UPDATE virtualization_virtualmachine AS vm SET
                interface_count = (SELECT COUNT(*) FROM virtualization_vminterface WHERE virtual_machine_id = vm.id),
                virtual_disk_count = (SELECT COUNT(*) FROM virtualization_virtualdisk WHERE virtual_machine_id = vm.id)
               WHERE vm.id = ANY($1)"#,
            virtual_machines,
        ).execute(&mut **transaction).await?;
    }
    if !prefixes.is_empty() {
        sqlx::query!(
            r#"UPDATE ipam_prefix AS prefix SET
                _depth = (SELECT COUNT(*) FROM ipam_prefix AS ancestor
                    WHERE ancestor.vrf_id IS NOT DISTINCT FROM prefix.vrf_id
                      AND ancestor.prefix >> prefix.prefix)::smallint,
                _children = (SELECT COUNT(*) FROM ipam_prefix AS child
                    WHERE child.vrf_id IS NOT DISTINCT FROM prefix.vrf_id
                      AND prefix.prefix >> child.prefix
                      AND NOT EXISTS (SELECT 1 FROM ipam_prefix AS intermediate
                        WHERE intermediate.vrf_id IS NOT DISTINCT FROM prefix.vrf_id
                          AND prefix.prefix >> intermediate.prefix
                          AND intermediate.prefix >> child.prefix))
               WHERE prefix.id = ANY($1)"#,
            prefixes,
        )
        .execute(&mut **transaction)
        .await?;
        sqlx::query!(
            r#"UPDATE ipam_prefix AS prefix SET
                _site_id = CASE WHEN scope.app_label = 'dcim' AND scope.model = 'site' THEN prefix.scope_id END,
                _region_id = CASE WHEN scope.app_label = 'dcim' AND scope.model = 'region' THEN prefix.scope_id
                                  WHEN scope.app_label = 'dcim' AND scope.model = 'site' THEN site.region_id END,
                _site_group_id = CASE WHEN scope.app_label = 'dcim' AND scope.model = 'sitegroup' THEN prefix.scope_id
                                      WHEN scope.app_label = 'dcim' AND scope.model = 'site' THEN site.group_id END,
                _location_id = CASE WHEN scope.app_label = 'dcim' AND scope.model = 'location' THEN prefix.scope_id END
               FROM ipam_prefix AS owner
               LEFT JOIN inventory_content_type AS scope ON scope.id = owner.scope_type_id
               LEFT JOIN dcim_site AS site ON scope.app_label = 'dcim' AND scope.model = 'site' AND site.id = owner.scope_id
               WHERE owner.id = prefix.id AND prefix.id = ANY($1)"#,
            prefixes,
        ).execute(&mut **transaction).await?;
    }
    Ok(())
}
