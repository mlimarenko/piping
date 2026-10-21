use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::errors::AppError;

/// Rebuild inventory-derived columns from the committed graph, not remote caches.
pub(super) async fn recompute(
    transaction: &mut Transaction<'_, Postgres>,
    devices: &[Uuid],
    virtual_machines: &[Uuid],
    prefixes: &[Uuid],
) -> Result<(), AppError> {
    if !devices.is_empty() {
        sqlx::query!(
            r#"UPDATE dcim_device AS device SET
                console_port_count = (SELECT COUNT(*) FROM dcim_consoleport WHERE device_id = device.id),
                console_server_port_count = (SELECT COUNT(*) FROM dcim_consoleserverport WHERE device_id = device.id),
                power_port_count = (SELECT COUNT(*) FROM dcim_powerport WHERE device_id = device.id),
                power_outlet_count = (SELECT COUNT(*) FROM dcim_poweroutlet WHERE device_id = device.id),
                interface_count = (SELECT COUNT(*) FROM dcim_interface WHERE device_id = device.id),
                front_port_count = (SELECT COUNT(*) FROM dcim_frontport WHERE device_id = device.id),
                rear_port_count = (SELECT COUNT(*) FROM dcim_rearport WHERE device_id = device.id),
                device_bay_count = (SELECT COUNT(*) FROM dcim_devicebay WHERE device_id = device.id),
                module_bay_count = (SELECT COUNT(*) FROM dcim_modulebay WHERE device_id = device.id),
                inventory_item_count = (SELECT COUNT(*) FROM dcim_inventoryitem WHERE device_id = device.id)
               WHERE device.id = ANY($1)"#,
            devices,
        ).execute(&mut **transaction).await?;
    }
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
