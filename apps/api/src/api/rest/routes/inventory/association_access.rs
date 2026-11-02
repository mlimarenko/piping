//! Authorization for inventory association targets and their label projections.

use std::collections::HashMap;
use uuid::Uuid;

use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ObjectRef;
use crate::contexts::inventory::infrastructure::persistence::repositories::content_type_lookup::lookup_content_type_id;
use crate::contexts::inventory::infrastructure::persistence::{
    FhrpGroupRepository, InterfaceRepository, IpAddressRepository, VmInterfaceRepository,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

/// Evaluate the requested action with persisted target attributes and the
/// canonical policy service, including its API-token scope ceiling.
pub(crate) async fn ensure_association_target_access(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    object_type: &str,
    id: Uuid,
) -> Result<(), AppError> {
    let target = match object_type {
        "ipam.fhrpgroup" => state
            .repositories
            .fhrp_groups
            .get_by_id(id)
            .await?
            .map(serde_json::to_value)
            .transpose()?,
        "dcim.interface" => state
            .repositories
            .interfaces
            .get_by_id(id)
            .await?
            .map(|row| {
                serde_json::json!({
                    "id": row.id, "device_id": row.device_id, "name": row.name,
                    "label": row.label, "description": row.description, "type": row.type_,
                    "enabled": row.enabled, "mgmt_only": row.mgmt_only,
                })
            }),
        "virtualization.vminterface" => state.repositories.vm_interfaces.get_by_id(id).await?.map(
            |row| {
                serde_json::json!({
                    "id": row.id, "virtual_machine_id": row.virtual_machine_id, "name": row.name,
                    "enabled": row.enabled, "mtu": row.mtu, "mode": row.mode,
                    "description": row.description,
                })
            },
        ),
        "ipam.ipaddress" => state
            .repositories
            .ip_addresses
            .get_by_id(id)
            .await?
            .map(|row| {
                serde_json::json!({
                    "id": row.id, "address": row.address, "status": row.status, "role": row.role,
                    "dns_name": row.dns_name, "description": row.description,
                    "assigned_object_id": row.assigned_object_id,
                    "assigned_object_type_id": row.assigned_object_type_id,
                    "tenant_id": row.tenant_id, "vrf_id": row.vrf_id,
                })
            }),
        _ => {
            return Err(AppError::Validation(
                "Unsupported association target type".into(),
            ));
        }
    }
    .ok_or_else(|| {
        if action == Action::Read {
            AppError::NotFound("Association target not found".into())
        } else {
            AppError::Validation("Association target does not exist for its type".into())
        }
    })?;
    let mut attributes: HashMap<String, serde_json::Value> = serde_json::from_value(target)?;
    attributes.insert("object_type".into(), serde_json::json!(object_type));
    state
        .services
        .policy_evaluation
        .ensure_access_with_attributes(
            context,
            action,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
            Some(attributes),
        )
        .await
}

/// Preserve reference identity, but never resolve a FHRP label without target
/// READ permission. A forbidden or missing target uses the existing label-less
/// ObjectRef convention; infrastructure/policy evaluation errors still fail.
pub(crate) async fn resolve_association_refs(
    state: &AppState,
    context: &RequestContext,
    pairs: &[(Uuid, Uuid)],
) -> Result<HashMap<(Uuid, Uuid), ObjectRef>, AppError> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let group_type = lookup_content_type_id(&state.db_pool, "ipam", "fhrpgroup").await?;
    let mut permitted = Vec::new();
    let mut withheld = HashMap::new();
    for &(type_id, id) in pairs {
        if Some(type_id) == group_type {
            let pair = (type_id, id);
            if withheld.contains_key(&pair) || permitted.contains(&pair) {
                continue;
            }
            match ensure_association_target_access(
                state,
                context,
                Action::Read,
                "ipam.fhrpgroup",
                id,
            )
            .await
            {
                Ok(()) => permitted.push(pair),
                // Forbidden labels are withheld, and missing targets retain
                // the resolver's established dangling-reference representation.
                Err(AppError::Forbidden(_) | AppError::NotFound(_)) => {
                    withheld.insert(
                        pair,
                        ObjectRef {
                            object_type: "ipam.fhrpgroup".into(),
                            object_id: id,
                            label: None,
                        },
                    );
                }
                Err(error) => return Err(error),
            }
        } else {
            permitted.push((type_id, id));
        }
    }
    let mut refs = state
        .repositories
        .object_ref_resolver
        .resolve(&permitted)
        .await?;
    refs.extend(withheld);
    Ok(refs)
}
