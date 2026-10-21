//! Inventory adapter over the shared FK-blocked-delete guard.
//!
//! The engine lives in [`crate::contexts::shared::delete_guard`]; this module
//! contributes what only the inventory context knows — parent joins from the
//! `CONTENT_TYPES` registry for "child @ parent" labels, and the stable UI
//! entity keys the frontend uses to deep-link blocker groups.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::content_type::CONTENT_TYPES;
use crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{native_field_subjects, NativeFieldSubject};
use crate::contexts::shared::delete_guard::{BlockerParentJoin, BlockerTableMetadata};
use crate::errors::AppError;

/// Delete one inventory row by id with blocked-delete reporting.
///
/// # Errors
///
/// [`AppError::DeleteBlocked`] when inbound references hold the row;
/// otherwise the classified database error.
pub async fn delete_inventory_record(
    pool: &PgPool,
    target_table: &'static str,
    id: Uuid,
) -> Result<u64, AppError> {
    let mut transaction = pool.begin().await?;
    let deleted =
        delete_inventory_record_in_transaction(&mut transaction, target_table, id).await?;
    transaction.commit().await?;
    Ok(deleted)
}

fn inventory_blocker_metadata(table: &str) -> BlockerTableMetadata {
    BlockerTableMetadata {
        parent_join: parent_join_for_table(table),
        entity_key: entity_key_for_table(table),
    }
}

/// Transaction variant for inventory-owned relations removed with their parent.
pub async fn delete_inventory_record_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_table: &'static str,
    id: Uuid,
) -> Result<u64, AppError> {
    use crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::{detach_consumer, lock_secrets};
    crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::lock_field_catalog_for_usage(transaction).await?;
    let subjects = native_field_subjects(transaction).await?;
    if subjects
        .iter()
        .any(|definition| definition.table_name == target_table)
    {
        let statement = format!("SELECT id FROM {target_table} WHERE id = $1 FOR UPDATE");
        if sqlx::query(&statement)
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .is_none()
        {
            return Ok(0);
        }
        let resources =
            cascade_inventory_resources(transaction, target_table, id, &subjects).await?;
        let ids: Vec<Uuid> = resources.iter().map(|(_, id)| *id).collect();
        let secrets = sqlx::query!("SELECT secret_id FROM secret_usage_binding WHERE consumer_id = ANY($1) AND owner_scope_id = consumer_id AND ((consumer_type = 'inventory_custom_field' AND owner_scope_type = 'inventory_custom_field') OR (consumer_type = 'connection_scope:connection_target' AND owner_scope_type = 'connection_target')) ORDER BY secret_id", &ids)
            .fetch_all(&mut **transaction).await?;
        lock_secrets(
            transaction,
            &secrets.iter().map(|row| row.secret_id).collect::<Vec<_>>(),
        )
        .await?;
        for (table, resource_id) in resources {
            detach_consumer(
                transaction,
                "inventory_custom_field",
                resource_id,
                "inventory_custom_field",
                resource_id,
            )
            .await?;
            if matches!(
                table.as_str(),
                "dcim_device" | "virtualization_virtualmachine"
            ) {
                detach_consumer(
                    transaction,
                    "connection_scope:connection_target",
                    resource_id,
                    "connection_target",
                    resource_id,
                )
                .await?;
            }
        }
    }
    crate::contexts::shared::delete_guard::delete_record_guarded_in_transaction(
        transaction,
        target_table,
        id,
        inventory_blocker_metadata,
    )
    .await
}

async fn cascade_inventory_resources(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_table: &'static str,
    id: Uuid,
    subjects: &[NativeFieldSubject],
) -> Result<Vec<(String, Uuid)>, AppError> {
    // CONTENT_TYPES.parent is label metadata, not deletion authority. Follow
    // only actual native CASCADE constraints, and only writable inventory rows.
    let mut resources = vec![(target_table.to_string(), id)];
    let mut visited = std::collections::BTreeSet::from([(target_table.to_string(), id)]);
    let mut cursor = 0;
    while cursor < resources.len() {
        let (table, parent_id) = resources[cursor].clone();
        cursor += 1;
        let children = sqlx::query!("SELECT child.relname::text AS \"table_name!\", a.attname::text AS \"column_name!\" FROM pg_constraint c JOIN pg_class child ON child.oid = c.conrelid JOIN pg_namespace n ON n.oid = child.relnamespace JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1] JOIN pg_attribute parent_key ON parent_key.attrelid = c.confrelid AND parent_key.attnum = c.confkey[1] WHERE c.confrelid = to_regclass($1) AND c.contype = 'f' AND c.confdeltype = 'c' AND array_length(c.conkey, 1) = 1 AND parent_key.attname = 'id' AND parent_key.atttypid = 'uuid'::regtype AND n.nspname = 'public' ORDER BY child.relname, a.attname", table)
            .fetch_all(&mut **transaction).await?;
        for child in children {
            let Some(native) = subjects
                .iter()
                .find(|definition| definition.table_name == child.table_name)
            else {
                continue;
            };
            let column = child.column_name.replace('"', "\"\"");
            let statement = format!(
                "SELECT id FROM public.\"{}\" WHERE \"{column}\" = $1 ORDER BY id FOR UPDATE",
                native.table_name.replace('"', "\"\"")
            );
            let rows = sqlx::query(&statement)
                .bind(parent_id)
                .fetch_all(&mut **transaction)
                .await?;
            for row in rows {
                let id: Uuid = row.try_get("id")?;
                if visited.insert((native.table_name.clone(), id)) {
                    resources.push((native.table_name.clone(), id));
                }
            }
        }
    }
    Ok(resources)
}

fn parent_join_for_table(table: &str) -> Option<BlockerParentJoin> {
    CONTENT_TYPES
        .iter()
        .find(|def| def.table == table)
        .and_then(|def| def.parent)
        .map(|parent| BlockerParentJoin {
            fk: parent.fk,
            table: parent.table,
        })
        .or(match table {
            "dcim_rackreservation" => Some(BlockerParentJoin {
                fk: "rack_id",
                table: "dcim_rack",
            }),
            "vpn_tunneltermination" => Some(BlockerParentJoin {
                fk: "tunnel_id",
                table: "vpn_tunnel",
            }),
            _ => None,
        })
}

fn entity_key_for_table(table: &str) -> Option<&'static str> {
    match table {
        "users_owner" => Some("Owners"),
        "users_ownergroup" => Some("Owner Groups"),
        "dcim_site" => Some("Sites"),
        "dcim_device" => Some("Devices"),
        "virtualization_virtualmachine" => Some("Virtual Machines"),
        "tenancy_tenant" => Some("Tenants"),
        "tenancy_tenantgroup" => Some("Tenant Groups"),
        "dcim_region" => Some("Regions"),
        "dcim_devicerole" => Some("Device Roles"),
        "tenancy_contact" => Some("Contacts"),
        "tenancy_contactrole" => Some("Contact Roles"),
        "tenancy_contactgroup" => Some("Contact Groups"),
        "tenancy_contactassignment" => Some("Contact Assignments"),
        "dcim_manufacturer" => Some("Manufacturers"),
        "dcim_sitegroup" => Some("Site Groups"),
        "dcim_platform" => Some("Platforms"),
        "dcim_devicetype" => Some("Device Types"),
        "dcim_rack" => Some("Racks"),
        "dcim_interface" => Some("Interfaces"),
        "dcim_cable" => Some("Cables"),
        "virtualization_clustertype" => Some("Cluster Types"),
        "virtualization_clustergroup" => Some("Cluster Groups"),
        "virtualization_cluster" => Some("Clusters"),
        "ipam_vlan" => Some("VLANs"),
        "ipam_prefix" => Some("Prefixes"),
        "ipam_ipaddress" => Some("IP Addresses"),
        "virtualization_vminterface" => Some("VM Interfaces"),
        "dcim_rackrole" => Some("Rack Roles"),
        "dcim_racktype" => Some("Rack Types"),
        "dcim_location" => Some("Locations"),
        "ipam_rir" => Some("RIRs"),
        "ipam_vrf" => Some("VRFs"),
        "ipam_routetarget" => Some("Route Targets"),
        "ipam_aggregate" => Some("Aggregates"),
        "ipam_asn" => Some("ASNs"),
        "ipam_iprange" => Some("IP Ranges"),
        "ipam_service" => Some("Services"),
        "inventory_dataflow" => Some("Dataflows"),
        "dcim_module" => Some("Modules"),
        "dcim_moduletype" => Some("Module Types"),
        "dcim_virtualchassis" => Some("Virtual Chassis"),
        "virtualization_virtualdisk" => Some("Virtual Disks"),
        "virtualization_virtualmachinetype" => Some("Virtual Machine Types"),
        "circuits_provider" => Some("Providers"),
        "circuits_circuittype" => Some("Circuit Types"),
        "circuits_providernetwork" => Some("Provider Networks"),
        "circuits_circuit" => Some("Circuits"),
        "ipam_asnrange" => Some("ASN Ranges"),
        "dcim_inventoryitemrole" => Some("Inventory Item Roles"),
        "vpn_tunnelgroup" => Some("Tunnel Groups"),
        "vpn_tunnel" => Some("Tunnels"),
        "vpn_tunneltermination" => Some("Tunnel Terminations"),
        "vpn_ikepolicy" => Some("IKE Policies"),
        "vpn_ikeproposal" => Some("IKE Proposals"),
        "vpn_ipsecpolicy" => Some("IPsec Policies"),
        "vpn_ipsecproposal" => Some("IPsec Proposals"),
        "vpn_ipsecprofile" => Some("IPsec Profiles"),
        "circuits_circuitgroup" => Some("Circuit Groups"),
        "circuits_provideraccount" => Some("Provider Accounts"),
        "circuits_circuittermination" => Some("Circuit Terminations"),
        "circuits_virtualcircuittype" => Some("Virtual Circuit Types"),
        "circuits_virtualcircuit" => Some("Virtual Circuits"),
        "dcim_powerpanel" => Some("Power Panels"),
        "dcim_powerfeed" => Some("Power Feeds"),
        "dcim_rackreservation" => Some("Rack Reservations"),
        "dcim_macaddress" => Some("MAC Addresses"),
        "dcim_virtualdevicecontext" => Some("Virtual Device Contexts"),
        "ipam_servicetemplate" => Some("Service Templates"),
        "dcim_devicebay" => Some("Device Bays"),
        "dcim_modulebay" => Some("Module Bays"),
        "dcim_consoleport" => Some("Console Ports"),
        "dcim_consoleserverport" => Some("Console Server Ports"),
        "dcim_powerport" => Some("Power Ports"),
        "dcim_poweroutlet" => Some("Power Outlets"),
        "dcim_frontport" => Some("Front Ports"),
        "dcim_rearport" => Some("Rear Ports"),
        "dcim_inventoryitem" => Some("Inventory Items"),
        _ => None,
    }
}
