//! Transaction intent and lock order shared by native hardware and graph replay.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::errors::{AppError, classify_sqlx_error_owned};

/// Rebuild device component counts after materialization or explicit graph replay.
pub(crate) async fn recount_device_components(
    transaction: &mut Transaction<'_, Postgres>,
    devices: &[Uuid],
) -> Result<(), AppError> {
    if devices.is_empty() {
        return Ok(());
    }
    sqlx::query(
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
    )
    .bind(devices)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Acquire before device, type, component or secret-resource row locks. The
/// statement triggers use the same key for direct SQL and FK cascade consumers.
pub(crate) async fn lock_hardware_graph(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    super::super::extras::tag_repository::lock_tag_catalog_for_usage(transaction).await?;
    sqlx::query("SELECT dcim_acquire_hardware_graph()")
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

/// An archive or snapshot owns explicit component identities. It must not also
/// synthesize unrelated template components. Only materialization is suppressed;
/// graph, Tag, foreign-key and live installation validators remain active.
pub(crate) async fn prepare_explicit_hardware_replay(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    lock_hardware_graph(transaction).await?;
    sqlx::query("SELECT set_config('piping.hardware_materialization','explicit_graph',true)")
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}
