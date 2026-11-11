//! Inventory adapter over the shared FK-blocked-delete guard.
//!
//! The engine lives in [`crate::contexts::shared::delete_guard`]; this module
//! contributes what only the inventory context knows — parent joins from the
//! `CONTENT_TYPES` registry for "child @ parent" labels, and the stable UI
//! entity keys the frontend uses to deep-link blocker groups.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::content_type::{
    CONTENT_TYPES, INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES,
};
use crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{native_field_subjects, NativeFieldSubject};
use crate::contexts::shared::delete_guard::{BlockerParentJoin, BlockerTableMetadata};
use crate::errors::{
    AppError, DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup,
    classify_sqlx_error_owned,
};

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

pub(crate) fn inventory_blocker_metadata(table: &str) -> BlockerTableMetadata {
    let owner_join = match table {
        "dcim_modulebay_module_bay_types" => Some(BlockerParentJoin {
            fk: "modulebay_id",
            table: "dcim_modulebay",
        }),
        "dcim_moduletype_module_bay_types" => Some(BlockerParentJoin {
            fk: "moduletype_id",
            table: "dcim_moduletype",
        }),
        "dcim_modulebaytemplate_module_bay_types" => Some(BlockerParentJoin {
            fk: "modulebaytemplate_id",
            table: "dcim_modulebaytemplate",
        }),
        _ => None,
    };
    BlockerTableMetadata {
        parent_join: if owner_join.is_some() {
            None
        } else {
            parent_join_for_table(table)
        },
        entity_key: entity_key_for_table(owner_join.map_or(table, |owner| owner.table)),
        owner_join,
    }
}

/// Transaction variant for inventory-owned relations removed with their parent.
pub async fn delete_inventory_record_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_table: &'static str,
    id: Uuid,
) -> Result<u64, AppError> {
    use crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::{detach_consumer, lock_secrets};
    if target_table == "inventory_tag" {
        super::extras::tag_repository::lock_tag_catalog_for_definition(transaction).await?;
        reject_tag_references(transaction, id).await?;
        return crate::contexts::shared::delete_guard::delete_record_guarded_in_transaction(
            transaction,
            target_table,
            id,
            inventory_blocker_metadata,
        )
        .await;
    }
    if target_table == "inventory_application" {
        super::dcim::hardware_graph::lock_hardware_graph(transaction).await?;
        // Contacts are a typed GenericRelation, not a physical FK, so the shared
        // FK scan cannot see them. Block the delete instead of orphaning the
        // assignment; the contract restricts referenced catalog deletion.
        let rows = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT a.id, COALESCE(NULLIF(c.name, ''), a.id::text) \
             FROM tenancy_contactassignment a \
             JOIN inventory_content_type ct ON ct.id = a.object_type_id \
             LEFT JOIN tenancy_contact c ON c.id = a.contact_id \
             WHERE ct.app_label = 'inventory' AND ct.model = 'application' \
               AND a.object_id = $1 ORDER BY a.id",
        )
        .bind(id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if !rows.is_empty() {
            let count = i64::try_from(rows.len())
                .map_err(|_| AppError::Database("Contact assignment count overflow".into()))?;
            let examples = rows
                .iter()
                .take(3)
                .map(|(assignment_id, label)| DeleteBlockedExample {
                    id: Some(*assignment_id),
                    label: label.clone(),
                })
                .collect();
            return Err(AppError::DeleteBlocked(DeleteBlockedDetails {
                total_count: count,
                blockers: vec![DeleteBlockedGroup {
                    table: "tenancy_contactassignment".into(),
                    entity_key: Some("Contact Assignments".into()),
                    count,
                    examples,
                }],
            }));
        }
    }
    if matches!(
        target_table,
        "dcim_device"
            | "dcim_interface"
            | "virtualization_vminterface"
            | "virtualization_virtualmachine"
            | "ipam_vlan"
            | "ipam_routetarget"
            | "circuits_virtualcircuit"
            | "circuits_virtualcircuittermination"
            | "vpn_l2vpn"
            | "vpn_l2vpntermination"
            | "tenancy_contactassignment"
            | "ipam_vlantranslationpolicy"
            | "ipam_vlantranslationrule"
            | "wireless_wirelesslink"
            | "dcim_module"
            | "dcim_modulebay"
            | "dcim_devicetype"
            | "dcim_moduletype"
            | "dcim_moduletypeprofile"
            | "dcim_modulebaytemplate"
            // Component template rows are read by the AFTER INSERT
            // instantiation trigger under the hardware graph lock. Deleting a
            // template must serialize with a concurrent device/module create
            // for the same type instead of racing its template read.
            | "dcim_consoleporttemplate"
            | "dcim_consoleserverporttemplate"
            | "dcim_powerporttemplate"
            | "dcim_poweroutlettemplate"
            | "dcim_interfacetemplate"
            | "dcim_frontporttemplate"
            | "dcim_rearporttemplate"
            | "dcim_devicebaytemplate"
            | "dcim_inventoryitemtemplate"
            | "dcim_coolingsource"
            | "dcim_coolingfeed"
            | "dcim_coolingintake"
            | "dcim_coolingoutflow"
            | "dcim_coolingintaketemplate"
            | "dcim_coolingoutflowtemplate"
            | "dcim_site"
            | "dcim_location"
            | "dcim_rack"
    ) {
        // The guard locks the target and its secret-bearing cascade resources
        // before issuing DELETE. Acquire the hardware graph before those locks,
        // rather than waiting for the statement/cascade trigger to do it later.
        // Profile deletion's deferred FK check also locks referencing types;
        // serialize it with their create/update before field-catalog/row locks.
        super::dcim::hardware_graph::lock_hardware_graph(transaction).await?;
    }
    if matches!(
        target_table,
        "circuits_circuitgroupassignment"
            | "circuits_circuitgroup"
            | "circuits_circuit"
            | "circuits_virtualcircuit"
    ) {
        super::circuits::circuit_group_assignment_repository::lock_graph(transaction).await?;
        super::circuits::circuit_group_assignment_repository::reject_member_deletion(
            transaction,
            target_table,
            id,
        )
        .await?;
    }
    crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::lock_field_catalog_for_usage(transaction).await?;
    if target_table == "ipam_fhrpgroup" {
        // Association writers take a shared lock on this same target row.
        // Hold the exclusive lock while inspecting GFKs and deleting, since
        // PostgreSQL cannot enforce these polymorphic references as FKs.
        if sqlx::query("SELECT id FROM ipam_fhrpgroup WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .is_none()
        {
            return Ok(0);
        }
        reject_fhrp_group_references(transaction, id).await?;
    }
    let subjects = native_field_subjects(transaction).await?;
    // The group owns native membership children even while its own custom-field
    // scope is not published. Their secret consumers must join parent deletion.
    if matches!(
        target_table,
        "circuits_circuitgroup" | "circuits_virtualcircuit" | "vpn_l2vpn"
    ) || subjects
        .iter()
        .any(|definition| definition.table_name == target_table)
    {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id FROM ");
        query.push(target_table).push(" WHERE id = $1 FOR UPDATE");
        if query
            .build()
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .is_none()
        {
            return Ok(0);
        }
        let resources =
            cascade_inventory_resources(transaction, target_table, id, &subjects, true).await?;
        let ids: Vec<Uuid> = resources.iter().map(|(_, id)| *id).collect();
        let secrets = sqlx::query!("SELECT secret_id FROM secret_usage_binding WHERE consumer_id = ANY($1) AND owner_scope_id = consumer_id AND ((consumer_type = 'inventory_custom_field' AND owner_scope_type = 'inventory_custom_field') OR (consumer_type = 'connection_scope:connection_target' AND owner_scope_type = 'connection_target')) ORDER BY secret_id", &ids)
            .fetch_all(&mut **transaction).await?;
        let mut secret_ids = secrets.iter().map(|row| row.secret_id).collect::<Vec<_>>();
        if target_table == "ipam_fhrpgroup" {
            let native: Vec<Uuid> = sqlx::query_scalar(
                "SELECT secret_id FROM secret_usage_binding WHERE consumer_type = 'inventory_fhrp_group' AND consumer_id = $1 AND owner_scope_type = 'inventory_fhrp_group' AND owner_scope_id = $1",
            ).bind(id).fetch_all(&mut **transaction).await?;
            secret_ids.extend(native);
        }
        lock_secrets(transaction, &secret_ids).await?;
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
    if target_table == "ipam_fhrpgroup" {
        detach_consumer(
            transaction,
            "inventory_fhrp_group",
            id,
            "inventory_fhrp_group",
            id,
        )
        .await?;
    }
    reject_component_template_references(transaction, target_table, id).await?;
    crate::contexts::shared::delete_guard::delete_record_guarded_in_transaction(
        transaction,
        target_table,
        id,
        inventory_blocker_metadata,
    )
    .await
}

/// Polymorphic `InventoryItemTemplate.component` references are not real FKs.
/// Report them through the canonical structured blocker contract before the FK
/// guard, for every exact component-target model (seven template kinds plus the
/// specialized cooling templates). The target's own specialized delete owner
/// remains unchanged: it already calls this shared transaction entry point.
async fn reject_component_template_references(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_table: &'static str,
    id: Uuid,
) -> Result<(), AppError> {
    let Some(definition) = CONTENT_TYPES
        .iter()
        .find(|definition| definition.table == target_table)
    else {
        return Ok(());
    };
    let token = definition.token();
    if !INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES.contains(&token.as_str()) {
        return Ok(());
    }
    let content_type_id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label=$1 AND model=$2")
            .bind(definition.app_label)
            .bind(definition.model)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
    let Some(content_type_id) = content_type_id else {
        return Ok(());
    };
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT id,COALESCE(NULLIF(name,''),id::text) label,COUNT(*) OVER() total FROM dcim_inventoryitemtemplate WHERE component_type_id=$1 AND component_id=$2 ORDER BY name,id LIMIT 3",
    )
    .bind(content_type_id)
    .bind(id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let Some((_, _, count)) = rows.first() else {
        return Ok(());
    };
    Err(AppError::DeleteBlocked(DeleteBlockedDetails {
        total_count: *count,
        blockers: vec![DeleteBlockedGroup {
            table: "dcim_inventoryitemtemplate".to_string(),
            entity_key: entity_key_for_table("dcim_inventoryitemtemplate").map(str::to_owned),
            count: *count,
            examples: rows
                .into_iter()
                .map(|(id, label, _)| DeleteBlockedExample {
                    id: Some(id),
                    label,
                })
                .collect(),
        }],
    }))
}

/// Array and JSON references are not FKs. The exclusive catalogue lock makes
/// their assessment and deletion atomic with all database-owned usage writers.
async fn reject_tag_references(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<(), AppError> {
    use crate::errors::{DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup};
    let tables: Vec<String> = sqlx::query_scalar("SELECT table_name FROM inventory_tag_subjects()")
        .fetch_all(&mut **transaction)
        .await?;
    let mut blockers = Vec::new();
    for table in tables
        .into_iter()
        .chain(std::iter::once("playbook_target_set".into()))
    {
        // Identifiers are discovered from the bounded native schema function,
        // quoted even though no request input can enter this statement.
        let quoted = table.replace('"', "\"\"");
        let predicate = if table == "playbook_target_set" {
            "selector->'tag_ids' @> jsonb_build_array($1::text)"
        } else {
            "tags @> ARRAY[$1::text]"
        };
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT id,COALESCE(NULLIF(to_jsonb(owner)->>'name',''),NULLIF(to_jsonb(owner)->>'address',''),id::text) label,COUNT(*) OVER() total FROM public.\"",
        );
        query
            .push(quoted)
            .push("\" owner WHERE ")
            .push(predicate)
            .push(" ORDER BY id LIMIT 3");
        let rows: Vec<(Uuid, String, i64)> = query
            .build_query_as()
            .bind(id.to_string())
            .fetch_all(&mut **transaction)
            .await?;
        if let Some((_, _, count)) = rows.first() {
            blockers.push(DeleteBlockedGroup {
                entity_key: entity_key_for_table(&table).map(str::to_owned),
                table,
                count: *count,
                examples: rows
                    .into_iter()
                    .map(|(id, label, _)| DeleteBlockedExample {
                        id: Some(id),
                        label,
                    })
                    .collect(),
            });
        }
    }
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT c.id, c.name::text, count(*) OVER() FROM config_context_binding b JOIN config_context c ON c.id = b.context_id WHERE b.scope_type = 'tag' AND b.scope_id = $1 ORDER BY c.id LIMIT 3",
    ).bind(id).fetch_all(&mut **transaction).await?;
    if let Some((_, _, count)) = rows.first() {
        blockers.push(DeleteBlockedGroup {
            entity_key: entity_key_for_table("config_context").map(str::to_owned),
            table: "config_context_binding".into(),
            count: *count,
            examples: rows
                .into_iter()
                .map(|(id, label, _)| DeleteBlockedExample {
                    id: Some(id),
                    label,
                })
                .collect(),
        });
    }
    if blockers.is_empty() {
        return Ok(());
    }
    Err(AppError::DeleteBlocked(DeleteBlockedDetails {
        total_count: blockers.iter().map(|v| v.count).sum(),
        blockers,
    }))
}

/// GFK references have no PostgreSQL delete constraint. Report them through
/// the same structured blocker contract as the canonical FK guard.
async fn reject_fhrp_group_references(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<(), AppError> {
    use crate::errors::app_error::{
        DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup,
    };
    let mut blockers = Vec::new();
    for (table, type_column, object_column, label, entity_key) in [
        (
            "ipam_ipaddress",
            "assigned_object_type_id",
            "assigned_object_id",
            "address::text",
            "IP Addresses",
        ),
        (
            "ipam_service",
            "parent_object_type_id",
            "parent_object_id",
            "name",
            "Services",
        ),
    ] {
        // Every identifier is from the closed list above, never request data.
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id, ");
        query.push(label).push(" AS label, COUNT(*) OVER() AS total FROM ").push(table)
            .push(" WHERE ").push(object_column).push(" = $1 AND ").push(type_column)
            .push(" = (SELECT id FROM inventory_content_type WHERE app_label = 'ipam' AND model = 'fhrpgroup') ORDER BY id LIMIT 3");
        let rows: Vec<(Uuid, String, i64)> = query
            .build_query_as()
            .bind(id)
            .fetch_all(&mut **transaction)
            .await?;
        if let Some((_, _, count)) = rows.first() {
            blockers.push(DeleteBlockedGroup {
                table: table.into(),
                entity_key: Some(entity_key.into()),
                count: *count,
                examples: rows
                    .into_iter()
                    .map(|(id, label, _)| DeleteBlockedExample {
                        id: Some(id),
                        label,
                    })
                    .collect(),
            });
        }
    }
    if blockers.is_empty() {
        return Ok(());
    }
    Err(AppError::DeleteBlocked(DeleteBlockedDetails {
        total_count: blockers.iter().map(|group| group.count).sum(),
        blockers,
    }))
}

/// A resource identity is table-qualified: UUID equality across endpoint kinds
/// does not establish either ownership or a cascade edge.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InventoryResourceIdentity {
    pub table: String,
    pub id: Uuid,
}

/// Read the canonical native cascade closure without locking resource rows or
/// retiring consumers. The caller must already hold restore's graph locks so
/// classification and subsequent guarded deletion share one graph.
pub(crate) async fn inventory_delete_cascade(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_table: &'static str,
    id: Uuid,
) -> Result<Vec<InventoryResourceIdentity>, AppError> {
    crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::lock_field_catalog_for_usage(transaction).await?;
    let subjects = native_field_subjects(transaction).await?;
    Ok(
        cascade_inventory_resources(transaction, target_table, id, &subjects, false)
            .await?
            .into_iter()
            .map(|(table, id)| InventoryResourceIdentity { table, id })
            .collect(),
    )
}

async fn cascade_inventory_resources(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_table: &'static str,
    id: Uuid,
    subjects: &[NativeFieldSubject],
    lock_rows: bool,
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
            let lock = if lock_rows { " FOR UPDATE" } else { "" };
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id FROM public.\"");
            query
                .push(native.table_name.replace('"', "\"\""))
                .push("\" WHERE \"")
                .push(column)
                .push("\" = $1 ORDER BY id")
                .push(lock);
            let rows = query
                .build()
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
        // PostgreSQL cannot describe GenericRelation cascades in pg_constraint.
        // Mirror only the three typed edges owned by 0011's endpoint trigger;
        // a coincident UUID in another content type is never a child.
        let endpoint_type = match table.as_str() {
            "dcim_interface" => Some(("dcim", "interface")),
            "virtualization_vminterface" => Some(("virtualization", "vminterface")),
            "ipam_vlan" => Some(("ipam", "vlan")),
            _ => None,
        };
        if let Some((app_label, model)) = endpoint_type {
            let lock = if lock_rows { " FOR UPDATE OF t" } else { "" };
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
                "SELECT t.id FROM vpn_l2vpntermination t JOIN inventory_content_type ct ON ct.id=t.assigned_object_type_id WHERE ct.app_label=$1 AND ct.model=$2 AND t.assigned_object_id=$3 ORDER BY t.id",
            );
            query.push(lock);
            let children: Vec<Uuid> = query
                .build_query_scalar()
                .bind(app_label)
                .bind(model)
                .bind(parent_id)
                .fetch_all(&mut **transaction)
                .await?;
            for child_id in children {
                let child = ("vpn_l2vpntermination".to_string(), child_id);
                if visited.insert(child.clone()) {
                    resources.push(child);
                }
            }
        }
        if table == "vpn_l2vpn" {
            // ContactsMixin is another typed GenericRelation, not a physical
            // FK. Include its field consumers before the SQL parent cascade.
            let lock = if lock_rows { " FOR UPDATE OF a" } else { "" };
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
                "SELECT a.id FROM tenancy_contactassignment a JOIN inventory_content_type ct ON ct.id=a.object_type_id WHERE ct.app_label='vpn' AND ct.model='l2vpn' AND a.object_id=$1 ORDER BY a.id",
            );
            query.push(lock);
            let children: Vec<Uuid> = query
                .build_query_scalar()
                .bind(parent_id)
                .fetch_all(&mut **transaction)
                .await?;
            for child_id in children {
                let child = ("tenancy_contactassignment".to_string(), child_id);
                if visited.insert(child.clone()) {
                    resources.push(child);
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
        "wireless_wirelesslangroup" => Some("Wireless LAN Groups"),
        "wireless_wirelesslan" => Some("Wireless LANs"),
        "wireless_wirelesslink" => Some("Wireless Links"),
        "dcim_platform" => Some("Platforms"),
        "dcim_devicetype" => Some("Device Types"),
        "dcim_rack" => Some("Racks"),
        "dcim_interface" => Some("Interfaces"),
        "dcim_cable" => Some("Cables"),
        "virtualization_clustertype" => Some("Cluster Types"),
        "virtualization_clustergroup" => Some("Cluster Groups"),
        "virtualization_cluster" => Some("Clusters"),
        "ipam_vlan" => Some("VLANs"),
        "ipam_vlantranslationpolicy" => Some("VLAN Translation Policies"),
        "ipam_vlantranslationrule" => Some("VLAN Translation Rules"),
        "ipam_prefix" => Some("Prefixes"),
        "ipam_ipaddress" => Some("IP Addresses"),
        "virtualization_vminterface" => Some("VM Interfaces"),
        "dcim_rackrole" => Some("Rack Roles"),
        "dcim_rackgroup" => Some("Rack Groups"),
        "dcim_cablebundle" => Some("Cable Bundles"),
        "dcim_racktype" => Some("Rack Types"),
        "dcim_location" => Some("Locations"),
        "ipam_rir" => Some("RIRs"),
        "ipam_vrf" => Some("VRFs"),
        "ipam_routetarget" => Some("Route Targets"),
        "ipam_aggregate" => Some("Aggregates"),
        "ipam_asn" => Some("ASNs"),
        "ipam_iprange" => Some("IP Ranges"),
        "ipam_service" => Some("Services"),
        "ipam_fhrpgroup" => Some("FHRP Groups"),
        "ipam_fhrpgroupassignment" => Some("FHRP Group Assignments"),
        "inventory_dataflow" => Some("Dataflows"),
        "inventory_dataflow_selector" => Some("Dataflows"),
        "inventory_dataflow_selector_ip" => Some("Dataflows"),
        "inventory_application" => Some("Applications"),
        "inventory_application_role" => Some("Application Roles"),
        "inventory_dataflow_group" => Some("Dataflow Groups"),
        "inventory_address_group" => Some("Address Groups"),
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
        "circuits_circuitgroupassignment" => Some("Circuit Group Assignments"),
        "circuits_provideraccount" => Some("Provider Accounts"),
        "circuits_circuittermination" => Some("Circuit Terminations"),
        "circuits_virtualcircuittype" => Some("Virtual Circuit Types"),
        "circuits_virtualcircuit" => Some("Virtual Circuits"),
        "circuits_virtualcircuittermination" => Some("Virtual Circuit Terminations"),
        "vpn_l2vpn" => Some("L2VPNs"),
        "vpn_l2vpntermination" => Some("L2VPN Terminations"),
        "dcim_powerpanel" => Some("Power Panels"),
        "dcim_powerfeed" => Some("Power Feeds"),
        "dcim_rackreservation" => Some("Rack Reservations"),
        "dcim_macaddress" => Some("MAC Addresses"),
        "dcim_virtualdevicecontext" => Some("Virtual Device Contexts"),
        "ipam_servicetemplate" => Some("Service Templates"),
        "dcim_devicebay" => Some("Device Bays"),
        "dcim_modulebay" => Some("Module Bays"),
        "dcim_modulebaytype" => Some("Module Bay Types"),
        "dcim_modulebaytemplate" => Some("Module Bay Templates"),
        "dcim_consoleporttemplate" => Some("Console Port Templates"),
        "dcim_consoleserverporttemplate" => Some("Console Server Port Templates"),
        "dcim_powerporttemplate" => Some("Power Port Templates"),
        "dcim_poweroutlettemplate" => Some("Power Outlet Templates"),
        "dcim_interfacetemplate" => Some("Interface Templates"),
        "dcim_frontporttemplate" => Some("Front Port Templates"),
        "dcim_rearporttemplate" => Some("Rear Port Templates"),
        "dcim_devicebaytemplate" => Some("Device Bay Templates"),
        "dcim_inventoryitemtemplate" => Some("Inventory Item Templates"),
        "dcim_porttemplatemapping" => Some("Port Template Mappings"),
        "dcim_coolingsource" => Some("Cooling Sources"),
        "dcim_coolingfeed" => Some("Cooling Feeds"),
        "dcim_coolingintake" => Some("Cooling Intakes"),
        "dcim_coolingoutflow" => Some("Cooling Outflows"),
        "dcim_coolingintaketemplate" => Some("Cooling Intake Templates"),
        "dcim_coolingoutflowtemplate" => Some("Cooling Outflow Templates"),
        "dcim_modulebay_module_bay_types" => Some("Module Bay memberships"),
        "dcim_moduletype_module_bay_types" => Some("Module Type memberships"),
        "dcim_modulebaytemplate_module_bay_types" => Some("Module Bay Template memberships"),
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
