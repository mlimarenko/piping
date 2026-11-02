//! Minimal persisted native inventory subjects for field-secret lifecycle tests.

use sqlx::PgPool;
use uuid::Uuid;

pub(crate) async fn seed_inventory_entity(
    pool: &PgPool,
    token: &str,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    let name = format!("field-lifecycle-{id}");
    match token {
        "dcim.site" => {
            sqlx::query("INSERT INTO dcim_site (id, name, slug, custom_field_data, status, facility, description, physical_address, shipping_address, comments) VALUES ($1, $2, $2, '{}', 'active', '', '', '', '', '')")
                .bind(id).bind(&name).execute(pool).await?;
        }
        "virtualization.virtualmachine" => {
            sqlx::query("INSERT INTO virtualization_virtualmachine (id, name, custom_field_data, status, comments, description, interface_count, virtual_disk_count, serial, start_on_boot) VALUES ($1, $2, '{}', 'active', '', '', 0, 0, '', 'default')")
                .bind(id).bind(&name).execute(pool).await?;
        }
        "dcim.device" => {
            let site = Uuid::now_v7();
            let manufacturer = Uuid::now_v7();
            let device_type = Uuid::now_v7();
            let role = Uuid::now_v7();
            sqlx::query("INSERT INTO dcim_site (id, name, slug, custom_field_data, status, facility, description, physical_address, shipping_address, comments) VALUES ($1, $2, $2, '{}', 'active', '', '', '', '', '')")
                .bind(site).bind(&name).execute(pool).await?;
            sqlx::query("INSERT INTO dcim_manufacturer (id, name, slug, custom_field_data, description, comments) VALUES ($1, $2, $2, '{}', '', '')")
                .bind(manufacturer).bind(&name).execute(pool).await?;
            sqlx::query("INSERT INTO dcim_devicerole (id, name, slug, custom_field_data, color, vm_role, description, comments, level, lft, rght, tree_id) VALUES ($1, $2, $2, '{}', '000000', false, '', '', 0, 1, 2, (SELECT COALESCE(max(tree_id), 0) + 1 FROM dcim_devicerole))")
                .bind(role).bind(&name).execute(pool).await?;
            sqlx::query("INSERT INTO dcim_devicetype (id, model, slug, custom_field_data, manufacturer_id, part_number, u_height, is_full_depth, front_image, rear_image, comments, description, console_port_template_count, console_server_port_template_count, power_port_template_count, power_outlet_template_count, interface_template_count, front_port_template_count, rear_port_template_count, device_bay_template_count, module_bay_template_count, inventory_item_template_count, exclude_from_utilization, device_count) VALUES ($1, $2, $2, '{}', $3, '', 1, false, '', '', '', '', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, 0)")
                .bind(device_type).bind(&name).bind(manufacturer).execute(pool).await?;
            sqlx::query("INSERT INTO dcim_device (id, name, custom_field_data, serial, status, comments, description, role_id, device_type_id, site_id, console_port_count, console_server_port_count, power_port_count, power_outlet_count, interface_count, front_port_count, rear_port_count, device_bay_count, module_bay_count, inventory_item_count) VALUES ($1, $2, '{}', '', 'active', '', '', $3, $4, $5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)")
                .bind(id).bind(&name).bind(role).bind(device_type).bind(site).execute(pool).await?;
        }
        _ => {
            return Err(sqlx::Error::Protocol(
                "unsupported native field fixture token".into(),
            ));
        }
    }
    Ok(())
}
