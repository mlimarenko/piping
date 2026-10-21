//! Per-step resource INSERTs for template runs.
//!
//! Each handler here owns one canonical INSERT into a foreign table. The outer
//! transaction is owned by `SqlxTemplateRepository::execute_template_run`; these
//! functions just borrow `tx` and run one statement per step.
//!
//! Dispatch is an OPEN registry keyed by the `resource_type` string
//! ([`SUPPORTED_RESOURCE_TYPES`] is the single source of truth). Adding a new
//! creatable entity means adding one `create_*_step` function and one arm to
//! the step creator dispatch plus its key to [`SUPPORTED_RESOURCE_TYPES`]; the
//! run-transaction atomicity, `{{steps.x.id}}` placeholder resolution, and
//! conflict-policy handling in `template_repository.rs` are entity-agnostic and
//! need no change.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::template_repository::StepResourceExecutor;
use crate::contexts::inventory::domain::native::template_reference::{self, TemplateSelection};
use crate::errors::AppError;

/// Validate persisted selections using only tables owned by the reference registry.
pub(crate) async fn validate_template_selections(
    pool: &sqlx::PgPool,
    selections: &[TemplateSelection],
) -> Result<(), AppError> {
    let targets = template_reference::targets()?;
    let mut grouped: BTreeMap<&str, BTreeSet<Uuid>> = BTreeMap::new();
    for selection in selections {
        if !targets
            .iter()
            .any(|target| target.resource == selection.resource)
        {
            return Err(AppError::Validation(format!(
                "Unsupported template selection resource: {}",
                selection.resource
            )));
        }
        grouped
            .entry(selection.resource.as_str())
            .or_default()
            .insert(selection.id);
    }

    for (resource, ids) in grouped {
        let target = targets
            .iter()
            .find(|target| target.resource == resource)
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "Unsupported template selection resource: {resource}"
                ))
            })?;
        let ids: Vec<Uuid> = ids.into_iter().collect();
        let sql = format!("SELECT id FROM {} WHERE id = ANY($1)", target.table);
        let found: BTreeSet<Uuid> = sqlx::query_scalar::<_, Uuid>(&sql)
            .bind(&ids)
            .fetch_all(pool)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to validate template selections for {resource}: {e}"
                ))
            })?
            .into_iter()
            .collect();
        if let Some(missing) = ids.iter().find(|id| !found.contains(id)) {
            return Err(AppError::Validation(format!(
                "Template selection {resource} references missing id {missing}"
            )));
        }
    }
    Ok(())
}

/// Canonical allow-list of template-step `resource_type` values that the run
/// executor can CREATE. This is the single source of truth consulted by both
/// the dispatch registry (`dispatch_step_creator`) and authoring-time
/// validation (`template_service::validate_template_steps`).
pub const SUPPORTED_RESOURCE_TYPES: &[&str] = &[
    // DCIM / tenancy single-table creates
    "tenant",
    "tenant_group",
    "region",
    "site",
    "site_group",
    "location",
    "prefix",
    "rack",
    "rack_role",
    "rack_type",
    "manufacturer",
    "device_role",
    "device_type",
    "platform",
    "device",
    "device_interface",
    "contact",
    "contact_assignment",
    // virtualization
    "cluster",
    "cluster_type",
    "cluster_group",
    "virtual_machine",
    "vm_interface",
    // ipam
    "ip_address",
    // VPN
    "tunnel_group",
    "tunnel",
    "tunnel_termination",
];

/// True when `value` is a recognised creatable template-step resource type.
#[must_use]
pub fn is_supported_resource_type(value: &str) -> bool {
    SUPPORTED_RESOURCE_TYPES.contains(&value)
}

/// Comma-separated rendering of [`SUPPORTED_RESOURCE_TYPES`] for error messages.
#[must_use]
pub fn supported_resource_types_csv() -> String {
    SUPPORTED_RESOURCE_TYPES.join(", ")
}

/// Dispatch a single template step to its tx-aware creator. Returns
/// `Err(AppError::Validation)` for any `resource_type` not in
/// [`SUPPORTED_RESOURCE_TYPES`]. Every key in that allow-list MUST have a
/// matching arm in the dispatch chain below.
async fn dispatch_step_creator(
    tx: &mut Transaction<'_, Postgres>,
    resource_type: &str,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    match resource_type {
        "prefix" => create_prefix_step(tx, payload).await,
        "rack" => create_rack_step(tx, payload).await,
        "rack_role" => create_rack_role_step(tx, payload).await,
        "rack_type" => create_rack_type_step(tx, payload).await,
        "manufacturer" => create_manufacturer_step(tx, payload).await,
        "device_role" => create_device_role_step(tx, payload).await,
        "device_type" => create_device_type_step(tx, payload).await,
        "platform" => create_platform_step(tx, payload).await,
        other => dispatch_tenancy_site_step_creator(tx, other, payload).await,
    }
}

async fn dispatch_tenancy_site_step_creator(
    tx: &mut Transaction<'_, Postgres>,
    resource_type: &str,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    match resource_type {
        "tenant" => create_tenant_step(tx, payload).await,
        "tenant_group" => create_tenant_group_step(tx, payload).await,
        "region" => create_region_step(tx, payload).await,
        "site" => create_site_step(tx, payload).await,
        "site_group" => create_site_group_step(tx, payload).await,
        "location" => create_location_step(tx, payload).await,
        other => dispatch_target_step_creator(tx, other, payload).await,
    }
}

/// Target, network and VPN creates share the same caller-owned transaction.
async fn dispatch_target_step_creator(
    tx: &mut Transaction<'_, Postgres>,
    resource_type: &str,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    match resource_type {
        "device" => create_device_step(tx, payload).await,
        "device_interface" => create_device_interface_step(tx, payload).await,
        "contact" => create_contact_step(tx, payload).await,
        "contact_assignment" => create_contact_assignment_step(tx, payload).await,
        "cluster" => create_cluster_step(tx, payload).await,
        "cluster_type" => create_cluster_type_step(tx, payload).await,
        "cluster_group" => create_cluster_group_step(tx, payload).await,
        "virtual_machine" => create_virtual_machine_step(tx, payload).await,
        "vm_interface" => create_vm_interface_step(tx, payload).await,
        "ip_address" => create_ip_address_step(tx, payload).await,
        "tunnel_group" => create_tunnel_group_step(tx, payload).await,
        "tunnel" => create_tunnel_step(tx, payload).await,
        "tunnel_termination" => create_tunnel_termination_step(tx, payload).await,
        other => Err(AppError::Validation(format!(
            "Unsupported template step resource_type: {other}"
        ))),
    }
}

#[derive(Clone, Default)]
pub struct SqlxTemplateStepResourceRepository;

impl SqlxTemplateStepResourceRepository {
    pub fn new() -> Self {
        Self
    }
}

impl StepResourceExecutor for SqlxTemplateStepResourceRepository {
    fn execute<'a, 'tx>(
        &'a self,
        tx: &'a mut Transaction<'tx, Postgres>,
        step: &'a Value,
        resolved_payload: &'a Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<Uuid>, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let resource_type = step
                .get("resource_type")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AppError::Validation("step.resource_type is required".to_string())
                })?;
            dispatch_step_creator(tx, resource_type, resolved_payload).await
        })
    }
}

// ---------------------------------------------------------------------------
// Per-resource INSERTs
// ---------------------------------------------------------------------------

async fn create_tenant_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let group_id = p.optional_uuid("group_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO tenancy_tenant (
            id, custom_field_data, name, slug, description, comments, group_id
        )
        VALUES ($1, '{}'::jsonb, $2, $3, '', '', $4)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(group_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create tenant from step: {e}")))?;
    Ok(vec![id])
}

async fn create_site_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let tenant_id = p
        .optional_uuid("tenant_id")?
        .or(match p.optional_string("tenant") {
            Some(value) => lookup_tenant_id(tx, &value).await?,
            None => None,
        });
    let region_id = p
        .optional_uuid("region_id")?
        .or(match p.optional_string("region") {
            Some(value) => lookup_region_id(tx, &value).await?,
            None => None,
        });
    let site_group_id =
        p.optional_uuid("site_group_id")?
            .or(match p.optional_string("site_group") {
                Some(value) => lookup_site_group_id(tx, &value).await?,
                None => None,
            });
    let tenant_id = tenant_id.or(match p.optional_string("tenant") {
        Some(value) => lookup_tenant_id(tx, &value).await?,
        None => None,
    });
    let facility = p.optional_string("facility").unwrap_or_default();
    let time_zone = p.optional_string("time_zone");
    let physical_address = p.optional_string("physical_address").unwrap_or_default();
    let shipping_address = p.optional_string("shipping_address").unwrap_or_default();
    let latitude = p.optional_f64("latitude");
    let longitude = p.optional_f64("longitude");
    let description = p.optional_string("description").unwrap_or_default();
    let comments = p.optional_string("comments").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_site (
            id, custom_field_data, name, slug, status, tenant_id, region_id,
            facility, time_zone, group_id, physical_address, shipping_address,
            latitude, longitude, description, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(status)
    .bind(tenant_id)
    .bind(region_id)
    .bind(facility)
    .bind(time_zone.as_deref())
    .bind(site_group_id)
    .bind(physical_address)
    .bind(shipping_address)
    .bind(latitude)
    .bind(longitude)
    .bind(description)
    .bind(comments)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create site from step: {e}")))?;
    Ok(vec![id])
}

async fn create_device_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let site_id = p
        .optional_uuid("site_id")?
        .or(match p.optional_string("site") {
            Some(value) => lookup_site_id(tx, &value).await?,
            None => None,
        })
        .ok_or_else(|| AppError::Validation("payload.site_id is required".to_string()))?;
    let role_id = p
        .optional_uuid("role_id")?
        .or(match p.optional_string("role") {
            Some(value) => lookup_device_role_id(tx, &value).await?,
            None => None,
        })
        .ok_or_else(|| AppError::Validation("payload.role_id is required".to_string()))?;
    let device_type_id = p
        .optional_uuid("device_type_id")?
        .or(match p.optional_string("device_type") {
            Some(value) => lookup_device_type_id(tx, &value).await?,
            None => None,
        })
        .ok_or_else(|| AppError::Validation("payload.device_type_id is required".to_string()))?;
    let tenant_id = p
        .optional_uuid("tenant_id")?
        .or(match p.optional_string("tenant") {
            Some(value) => lookup_tenant_id(tx, &value).await?,
            None => None,
        });
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_device (
            id, custom_field_data, name, serial, status, comments, role_id,
            device_type_id, site_id, tenant_id, description,
            console_port_count, console_server_port_count, power_port_count,
            power_outlet_count, interface_count, front_port_count, rear_port_count,
            device_bay_count, module_bay_count, inventory_item_count
        )
        VALUES ($1, '{}'::jsonb, $2, '', $3, '', $4, $5, $6, $7, '',
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(status)
    .bind(role_id)
    .bind(device_type_id)
    .bind(site_id)
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create device from step: {e}")))?;
    Ok(vec![id])
}

async fn create_virtual_machine_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let cluster_id = p.optional_uuid("cluster_id")?;
    let tenant_id = p.optional_uuid("tenant_id")?;
    let role_id = p.optional_uuid("role_id")?;
    let site_id = p.optional_uuid("site_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO virtualization_virtualmachine (
            id, custom_field_data, name, status, comments, description,
            interface_count, virtual_disk_count, serial, start_on_boot,
            cluster_id, tenant_id, role_id, site_id
        )
        VALUES ($1, '{}'::jsonb, $2, $3, '', '', 0, 0, '', 'false', $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(status)
    .bind(cluster_id)
    .bind(tenant_id)
    .bind(role_id)
    .bind(site_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create virtual machine from step: {e}")))?;
    Ok(vec![id])
}

async fn create_device_interface_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let device_id = p.required_uuid("device_id")?;
    let interface_type = p
        .optional_string("type")
        .unwrap_or_else(|| "virtual".to_string());
    let mgmt_only = p.optional_bool("mgmt_only").unwrap_or(false);
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_interface (
            id, custom_field_data, name, label, description, mark_connected,
            enabled, _name, type, mgmt_only, device_id
        )
        VALUES ($1, '{}'::jsonb, $2, $2, '', false, true, $2, $3, $4, $5)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(interface_type)
    .bind(mgmt_only)
    .bind(device_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create device interface from step: {e}")))?;
    Ok(vec![id])
}

async fn create_prefix_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let prefix = p.required_string("prefix")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let site_id = p
        .optional_uuid("site_id")?
        .or(match p.optional_string("site") {
            Some(value) => lookup_site_id(tx, &value).await?,
            None => None,
        });
    let tenant_id = p
        .optional_uuid("tenant_id")?
        .or(match p.optional_string("tenant") {
            Some(value) => lookup_tenant_id(tx, &value).await?,
            None => None,
        });
    let description = p.optional_string("description").unwrap_or_default();
    let comments = p.optional_string("comments").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO ipam_prefix (
            id, custom_field_data, prefix, status, is_pool, description,
            tenant_id, vrf_id, _children, _depth, mark_utilized, comments, _site_id, tags
        )
        VALUES ($1, '{}'::jsonb, $2::cidr, $3, false, $4, $5, NULL, 0, 0, false, $6, $7, '{}')
        "#,
    )
    .bind(id)
    .bind(prefix)
    .bind(status)
    .bind(description)
    .bind(tenant_id)
    .bind(comments)
    .bind(site_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create prefix from step: {e}")))?;
    Ok(vec![id])
}

async fn create_vm_interface_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let vm_id = p.required_uuid("virtual_machine_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO virtualization_vminterface (
            id, custom_field_data, enabled, name, _name, description, virtual_machine_id
        )
        VALUES ($1, '{}'::jsonb, true, $2, $2, '', $3)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(vm_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create VM interface from step: {e}")))?;
    Ok(vec![id])
}

async fn create_ip_address_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let address = p.required_string("address")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let dns_name = p.optional_string("dns_name").unwrap_or_default();
    let description = p.optional_string("description").unwrap_or_default();
    let comments = p.optional_string("comments").unwrap_or_default();
    let tenant_id = p.optional_uuid("tenant_id")?;
    let interface_id = p.optional_uuid("interface_id")?;
    let vm_interface_id = p.optional_uuid("vm_interface_id")?;
    let set_primary = p.optional_bool("set_primary").unwrap_or(false);
    let id = Uuid::now_v7();

    let (assigned_object_type_id, assigned_object_id, assigned_kind) =
        if let Some(interface_id) = interface_id {
            (
                Some(ensure_content_type(tx, "dcim", "interface").await?),
                Some(interface_id),
                Some("device"),
            )
        } else if let Some(vm_interface_id) = vm_interface_id {
            (
                Some(ensure_content_type(tx, "virtualization", "vminterface").await?),
                Some(vm_interface_id),
                Some("virtual_machine"),
            )
        } else {
            (None, None, None)
        };

    sqlx::query(
        r#"
        INSERT INTO ipam_ipaddress (
            id, custom_field_data, address, status, dns_name, description,
            tenant_id, assigned_object_type_id, assigned_object_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2::inet, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(&address)
    .bind(status)
    .bind(dns_name)
    .bind(description)
    .bind(tenant_id)
    .bind(assigned_object_type_id)
    .bind(assigned_object_id)
    .bind(comments)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create IP address from step: {e}")))?;
    if set_primary {
        match assigned_kind {
            Some("device") => {
                set_device_primary_from_interface(tx, interface_id, id, &address).await?
            }
            Some("virtual_machine") => {
                set_vm_primary_from_interface(tx, vm_interface_id, id, &address).await?
            }
            _ => {}
        }
    }
    Ok(vec![id])
}

async fn create_contact_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let title = p.optional_string("title").unwrap_or_default();
    let phone = p.optional_string("phone").unwrap_or_default();
    let email = p.optional_string("email").unwrap_or_default();
    let address = p.optional_string("address").unwrap_or_default();
    let comments = p.optional_string("comments").unwrap_or_default();
    let link = p.optional_string("link").unwrap_or_default();
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO tenancy_contact (
            id, custom_field_data, name, title, phone, email, address,
            comments, link, description
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(title)
    .bind(phone)
    .bind(email)
    .bind(address)
    .bind(comments)
    .bind(link)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create contact from step: {e}")))?;
    Ok(vec![id])
}

async fn create_contact_assignment_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let object_type = p.required_string("object_type")?;
    let object_id = p.required_uuid("object_id")?;
    let contact_id = p.required_uuid("contact_id")?;
    let role_id = p.required_uuid("role_id")?;
    let priority = p.optional_string("priority");
    let Some((app_label, model)) = object_type.split_once('.') else {
        return Err(AppError::Validation(
            "payload.object_type must look like dcim.site".to_string(),
        ));
    };
    let object_type_id = ensure_content_type(tx, app_label, model).await?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO tenancy_contactassignment (
            id, object_id, priority, contact_id, object_type_id, role_id, custom_field_data
        )
        VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb)
        ON CONFLICT (object_type_id, object_id, contact_id, role_id) DO NOTHING
        "#,
    )
    .bind(id)
    .bind(object_id)
    .bind(priority.as_deref())
    .bind(contact_id)
    .bind(object_type_id)
    .bind(role_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        AppError::Database(format!(
            "Failed to create contact assignment from step: {e}"
        ))
    })?;
    Ok(vec![id])
}

async fn create_tunnel_group_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let comments = p.optional_string("comments").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO vpn_tunnelgroup (
            id, custom_field_data, name, slug, description, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(comments)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create tunnel group from step: {e}")))?;
    Ok(vec![id])
}

async fn create_tunnel_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let encapsulation = p
        .optional_string("encapsulation")
        .unwrap_or_else(|| "ipsec-tunnel".to_string());
    let description = p.optional_string("description").unwrap_or_default();
    let comments = p.optional_string("comments").unwrap_or_default();
    let group_id = p.optional_uuid("group_id")?;
    let ipsec_profile_id = p.optional_uuid("ipsec_profile_id")?;
    let tunnel_id = p.optional_i64("tunnel_id");
    let tenant_id = p.optional_uuid("tenant_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO vpn_tunnel (
            id, custom_field_data, name, status, encapsulation, description, comments,
            group_id, ipsec_profile_id, tunnel_id, tenant_id
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(status)
    .bind(encapsulation)
    .bind(description)
    .bind(comments)
    .bind(group_id)
    .bind(ipsec_profile_id)
    .bind(tunnel_id)
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create tunnel from step: {e}")))?;
    Ok(vec![id])
}

async fn create_tunnel_termination_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let role = p.required_string("role")?;
    let tunnel_id = p.required_uuid("tunnel_id")?;
    let (termination_type_id, termination_id) = resolve_tunnel_termination_object(tx, &p).await?;
    let outside_ip_id = match p.optional_uuid("outside_ip_id")? {
        Some(id) => Some(id),
        None => match p.optional_string("outside_ip") {
            Some(address) => Some(ensure_template_ip_address(tx, &address).await?),
            None => None,
        },
    };
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO vpn_tunneltermination (
            id, custom_field_data, role, tunnel_id, termination_type_id, termination_id,
            outside_ip_id
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(role)
    .bind(tunnel_id)
    .bind(termination_type_id)
    .bind(termination_id)
    .bind(outside_ip_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        AppError::Database(format!(
            "Failed to create tunnel termination from step: {e}"
        ))
    })?;
    Ok(vec![id])
}

async fn resolve_tunnel_termination_object(
    tx: &mut Transaction<'_, Postgres>,
    p: &StepPayload<'_>,
) -> Result<(Uuid, Option<Uuid>), AppError> {
    if let Some(interface_id) = p.optional_uuid("interface_id")? {
        return Ok((
            ensure_content_type(tx, "dcim", "interface").await?,
            Some(interface_id),
        ));
    }
    if let Some(vm_interface_id) = p.optional_uuid("vm_interface_id")? {
        return Ok((
            ensure_content_type(tx, "virtualization", "vminterface").await?,
            Some(vm_interface_id),
        ));
    }
    let termination_type_id = p.required_uuid("termination_type_id")?;
    // A raw content-type ref must also identify the terminated object, otherwise
    // the termination is orphaned and never matches discovery joins.
    let termination_id = Some(p.required_uuid("termination_id")?);
    Ok((termination_type_id, termination_id))
}

async fn create_tenant_group_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let parent_id = p.optional_uuid("parent_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO tenancy_tenantgroup (
            id, custom_field_data, name, slug, description,
            lft, rght, tree_id, level, parent_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, 0, 0, 0, 0, $5, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(parent_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create tenant group from step: {e}")))?;
    Ok(vec![id])
}

async fn create_region_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let parent_id = p.optional_uuid("parent_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_region (
            id, custom_field_data, name, slug, description,
            lft, rght, tree_id, level, parent_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, 0, 0, 0, 0, $5, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(parent_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create region from step: {e}")))?;
    Ok(vec![id])
}

async fn create_site_group_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let parent_id = p.optional_uuid("parent_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_sitegroup (
            id, custom_field_data, name, slug, description,
            lft, rght, tree_id, level, parent_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, 0, 0, 0, 0, $5, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(parent_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create site group from step: {e}")))?;
    Ok(vec![id])
}

async fn create_location_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let site_id = p.required_uuid("site_id")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let description = p.optional_string("description").unwrap_or_default();
    let facility = p.optional_string("facility").unwrap_or_default();
    let parent_id = p.optional_uuid("parent_id")?;
    let tenant_id = p.optional_uuid("tenant_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_location (
            id, custom_field_data, name, slug, description, lft, rght, tree_id,
            level, parent_id, site_id, tenant_id, status, facility, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, 0, 0, 0, 0, $5, $6, $7, $8, $9, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(parent_id)
    .bind(site_id)
    .bind(tenant_id)
    .bind(status)
    .bind(facility)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create location from step: {e}")))?;
    Ok(vec![id])
}

async fn create_rack_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let site_id = p.required_uuid("site_id")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let width = p.optional_i64("width").unwrap_or(19);
    let u_height = p.optional_i64("u_height").unwrap_or(42);
    let desc_units = p.optional_bool("desc_units").unwrap_or(false);
    let facility_id = p.optional_string("facility_id");
    let serial = p.optional_string("serial").unwrap_or_default();
    let asset_tag = p.optional_string("asset_tag");
    let form_factor = p.optional_string("form_factor");
    let tenant_id = p.optional_uuid("tenant_id")?;
    let role_id = p.optional_uuid("role_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_rack (
            id, custom_field_data, name, site_id, status, facility_id, tenant_id,
            role_id, serial, asset_tag, form_factor, width, u_height, desc_units,
            comments, description, starting_unit
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, '', '', 1)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(site_id)
    .bind(status)
    .bind(facility_id)
    .bind(tenant_id)
    .bind(role_id)
    .bind(serial)
    .bind(asset_tag)
    .bind(form_factor)
    .bind(i16::try_from(width).unwrap_or(19))
    .bind(i16::try_from(u_height).unwrap_or(42))
    .bind(desc_units)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create rack from step: {e}")))?;
    Ok(vec![id])
}

async fn create_rack_role_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let color = p
        .optional_string("color")
        .unwrap_or_else(|| "9e9e9e".to_string());
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_rackrole (
            id, custom_field_data, name, slug, color, description, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(color)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create rack role from step: {e}")))?;
    Ok(vec![id])
}

async fn create_rack_type_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let model = p.required_string("model")?;
    let manufacturer_id = p.required_uuid("manufacturer_id")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&model));
    let form_factor = p
        .optional_string("form_factor")
        .unwrap_or_else(|| "4-post-frame".to_string());
    let width = p.optional_i64("width").unwrap_or(19);
    let u_height = p.optional_i64("u_height").unwrap_or(42);
    let starting_unit = p.optional_i64("starting_unit").unwrap_or(1);
    let desc_units = p.optional_bool("desc_units").unwrap_or(false);
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_racktype (
            id, custom_field_data, manufacturer_id, model, slug, form_factor,
            width, u_height, starting_unit, desc_units, description, comments, rack_count
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, '', 0)
        "#,
    )
    .bind(id)
    .bind(manufacturer_id)
    .bind(model)
    .bind(slug)
    .bind(form_factor)
    .bind(i16::try_from(width).unwrap_or(19))
    .bind(i16::try_from(u_height).unwrap_or(42))
    .bind(i16::try_from(starting_unit).unwrap_or(1))
    .bind(desc_units)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create rack type from step: {e}")))?;
    Ok(vec![id])
}

async fn create_manufacturer_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_manufacturer (id, custom_field_data, name, slug, description, comments)
        VALUES ($1, '{}'::jsonb, $2, $3, $4, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create manufacturer from step: {e}")))?;
    Ok(vec![id])
}

async fn create_device_role_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let color = p
        .optional_string("color")
        .unwrap_or_else(|| "9e9e9e".to_string());
    let vm_role = p.optional_bool("vm_role").unwrap_or(false);
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_devicerole (
            id, custom_field_data, name, slug, color, vm_role, description,
            config_template_id, level, lft, rght, tree_id, parent_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, NULL, 0, 0, 0, 0, NULL, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(color)
    .bind(vm_role)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create device role from step: {e}")))?;
    Ok(vec![id])
}

async fn create_device_type_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let model = p.required_string("model")?;
    let manufacturer_id = p.required_uuid("manufacturer_id")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&model));
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_devicetype (
            id, custom_field_data, manufacturer_id, model, slug, part_number,
            u_height, is_full_depth, front_image, rear_image, comments, description,
            console_port_template_count, console_server_port_template_count,
            power_port_template_count, power_outlet_template_count,
            interface_template_count, front_port_template_count, rear_port_template_count,
            device_bay_template_count, module_bay_template_count,
            inventory_item_template_count, exclude_from_utilization, device_count
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, '', 1.0, false, '', '', '', $5,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, 0)
        "#,
    )
    .bind(id)
    .bind(manufacturer_id)
    .bind(model)
    .bind(slug)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create device type from step: {e}")))?;
    Ok(vec![id])
}

async fn create_platform_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let manufacturer_id = p.optional_uuid("manufacturer_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO dcim_platform (
            id, custom_field_data, name, slug, description, manufacturer_id,
            config_template_id, parent_id, level, lft, rght, tree_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, NULL, NULL, 0, 0, 0, 0, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(manufacturer_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create platform from step: {e}")))?;
    Ok(vec![id])
}

async fn create_cluster_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let type_id = p.required_uuid("type_id")?;
    let status = p
        .optional_string("status")
        .unwrap_or_else(|| "active".to_string());
    let description = p.optional_string("description").unwrap_or_default();
    let group_id = p.optional_uuid("group_id")?;
    let tenant_id = p.optional_uuid("tenant_id")?;
    let site_id = p.optional_uuid("site_id")?;
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO virtualization_cluster (
            id, custom_field_data, name, comments, type_id, status, description,
            group_id, tenant_id, _site_id
        )
        VALUES ($1, '{}'::jsonb, $2, '', $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(type_id)
    .bind(status)
    .bind(description)
    .bind(group_id)
    .bind(tenant_id)
    .bind(site_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create cluster from step: {e}")))?;
    Ok(vec![id])
}

async fn create_cluster_type_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO virtualization_clustertype (
            id, custom_field_data, name, slug, description, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create cluster type from step: {e}")))?;
    Ok(vec![id])
}

async fn create_cluster_group_step(
    tx: &mut Transaction<'_, Postgres>,
    payload: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let p = StepPayload::new(payload);
    let name = p.required_string("name")?;
    let slug = p
        .optional_string("slug")
        .unwrap_or_else(|| derive_slug(&name));
    let description = p.optional_string("description").unwrap_or_default();
    let id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO virtualization_clustergroup (
            id, custom_field_data, name, slug, description, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, '')
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create cluster group from step: {e}")))?;
    Ok(vec![id])
}

async fn ensure_content_type(
    tx: &mut Transaction<'_, Postgres>,
    app_label: &str,
    model: &str,
) -> Result<Uuid, AppError> {
    if let Some(row) =
        sqlx::query("SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2")
            .bind(app_label)
            .bind(model)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load content type: {e}")))?
    {
        let id: Uuid = row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to parse content type id: {e}")))?;
        return Ok(id);
    }

    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_content_type (id, app_label, model) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(app_label)
        .bind(model)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to insert content type: {e}")))?;
    Ok(id)
}

async fn ensure_template_ip_address(
    tx: &mut Transaction<'_, Postgres>,
    address: &str,
) -> Result<Uuid, AppError> {
    if let Some(row) = sqlx::query("SELECT id FROM ipam_ipaddress WHERE address = $1::inet LIMIT 1")
        .bind(address)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load template IP address: {e}")))?
    {
        return row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to parse IP address id: {e}")));
    }

    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO ipam_ipaddress (
            id, custom_field_data, address, status, dns_name, description,
            tenant_id, assigned_object_type_id, assigned_object_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2::inet, 'active', '', '', NULL, NULL, NULL, '')
        "#,
    )
    .bind(id)
    .bind(address)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create template IP address: {e}")))?;
    Ok(id)
}

async fn lookup_region_id(
    tx: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    lookup_id_by_name_or_slug(tx, "dcim_region", value).await
}

async fn lookup_site_group_id(
    tx: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    lookup_id_by_name_or_slug(tx, "dcim_sitegroup", value).await
}

async fn lookup_tenant_id(
    tx: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    lookup_id_by_name_or_slug(tx, "tenancy_tenant", value).await
}

async fn lookup_site_id(
    tx: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    lookup_id_by_name_or_slug(tx, "dcim_site", value).await
}

async fn lookup_device_role_id(
    tx: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    lookup_id_by_name_or_slug(tx, "dcim_devicerole", value).await
}

async fn lookup_device_type_id(
    tx: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM dcim_devicetype
        WHERE slug = $1 OR model = $1
        ORDER BY model
        LIMIT 2
        "#,
    )
    .bind(value)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to lookup device type: {e}")))?;
    unambiguous_lookup_id(ids, "device type", value)
}

async fn lookup_id_by_name_or_slug(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    let sql = match table {
        "dcim_region" => {
            "SELECT id FROM dcim_region WHERE slug = $1 OR name = $1 ORDER BY name LIMIT 2"
        }
        "dcim_sitegroup" => {
            "SELECT id FROM dcim_sitegroup WHERE slug = $1 OR name = $1 ORDER BY name LIMIT 2"
        }
        "tenancy_tenant" => {
            "SELECT id FROM tenancy_tenant WHERE slug = $1 OR name = $1 ORDER BY name LIMIT 2"
        }
        "dcim_site" => {
            "SELECT id FROM dcim_site WHERE slug = $1 OR name = $1 ORDER BY name LIMIT 2"
        }
        "dcim_devicerole" => {
            "SELECT id FROM dcim_devicerole WHERE slug = $1 OR name = $1 ORDER BY name LIMIT 2"
        }
        _ => {
            return Err(AppError::Validation(format!(
                "Unsupported lookup table: {table}"
            )));
        }
    };
    let ids = sqlx::query_scalar::<_, Uuid>(sql)
        .bind(value)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to lookup {table}: {e}")))?;
    unambiguous_lookup_id(ids, table, value)
}

fn unambiguous_lookup_id(
    ids: Vec<Uuid>,
    entity: &str,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    if ids.len() > 1 {
        return Err(AppError::Validation(format!(
            "Ambiguous {entity} reference: {value}"
        )));
    }
    Ok(ids.into_iter().next())
}

async fn set_device_primary_from_interface(
    tx: &mut Transaction<'_, Postgres>,
    interface_id: Option<Uuid>,
    ip_id: Uuid,
    address: &str,
) -> Result<(), AppError> {
    let Some(interface_id) = interface_id else {
        return Ok(());
    };
    let Some(row) = sqlx::query("SELECT device_id FROM dcim_interface WHERE id = $1")
        .bind(interface_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load interface owner: {e}")))?
    else {
        return Ok(());
    };
    let device_id: Uuid = row
        .try_get("device_id")
        .map_err(|e| AppError::Database(format!("Failed to parse interface device id: {e}")))?;
    let column = if address.contains(':') {
        "primary_ip6_id"
    } else {
        "primary_ip4_id"
    };
    let sql = format!("UPDATE dcim_device SET {column} = $2 WHERE id = $1");
    sqlx::query(&sql)
        .bind(device_id)
        .bind(ip_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to set device primary IP: {e}")))?;
    Ok(())
}

async fn set_vm_primary_from_interface(
    tx: &mut Transaction<'_, Postgres>,
    interface_id: Option<Uuid>,
    ip_id: Uuid,
    address: &str,
) -> Result<(), AppError> {
    let Some(interface_id) = interface_id else {
        return Ok(());
    };
    let Some(row) =
        sqlx::query("SELECT virtual_machine_id FROM virtualization_vminterface WHERE id = $1")
            .bind(interface_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load VM interface owner: {e}")))?
    else {
        return Ok(());
    };
    let vm_id: Uuid = row
        .try_get("virtual_machine_id")
        .map_err(|e| AppError::Database(format!("Failed to parse VM interface owner id: {e}")))?;
    let column = if address.contains(':') {
        "primary_ip6_id"
    } else {
        "primary_ip4_id"
    };
    let sql = format!("UPDATE virtualization_virtualmachine SET {column} = $2 WHERE id = $1");
    sqlx::query(&sql)
        .bind(vm_id)
        .bind(ip_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to set VM primary IP: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Payload helpers
// ---------------------------------------------------------------------------

struct StepPayload<'a> {
    value: &'a Value,
}

impl<'a> StepPayload<'a> {
    fn new(value: &'a Value) -> Self {
        Self { value }
    }

    fn required_string(&self, key: &str) -> Result<String, AppError> {
        let raw = self
            .value
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation(format!("payload.{key} is required")))?;
        let value = raw.trim();
        if value.is_empty() {
            return Err(AppError::Validation(format!(
                "payload.{key} cannot be empty"
            )));
        }
        Ok(value.to_string())
    }

    fn optional_string(&self, key: &str) -> Option<String> {
        self.value
            .get(key)
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    fn required_uuid(&self, key: &str) -> Result<Uuid, AppError> {
        let raw = self.required_string(key)?;
        Uuid::parse_str(&raw)
            .map_err(|e| AppError::Validation(format!("Invalid payload.{key}: {e}")))
    }

    fn optional_uuid(&self, key: &str) -> Result<Option<Uuid>, AppError> {
        let Some(raw) = self.optional_string(key) else {
            return Ok(None);
        };
        Uuid::parse_str(&raw)
            .map(Some)
            .map_err(|e| AppError::Validation(format!("Invalid payload.{key}: {e}")))
    }

    fn optional_bool(&self, key: &str) -> Option<bool> {
        self.value.get(key).and_then(Value::as_bool)
    }

    fn optional_f64(&self, key: &str) -> Option<f64> {
        self.value.get(key).and_then(Value::as_f64)
    }

    fn optional_i64(&self, key: &str) -> Option<i64> {
        self.value.get(key).and_then(Value::as_i64)
    }
}

fn derive_slug(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::{
        SUPPORTED_RESOURCE_TYPES, is_supported_resource_type, supported_resource_types_csv,
        unambiguous_lookup_id,
    };
    use crate::errors::AppError;
    use uuid::Uuid;

    #[test]
    fn manual_lookup_accepts_only_a_single_match() {
        let id = Uuid::now_v7();
        assert_eq!(
            unambiguous_lookup_id(vec![], "site", "Exact Name").unwrap(),
            None
        );
        assert_eq!(
            unambiguous_lookup_id(vec![id], "site", "Exact Name").unwrap(),
            Some(id)
        );
        assert!(matches!(
            unambiguous_lookup_id(vec![id, Uuid::now_v7()], "site", "Exact Name"),
            Err(AppError::Validation(_))
        ));
    }

    #[test]
    fn supported_resource_types_cover_the_canonical_set() {
        // Keep authoring validation and execution dispatch on one vocabulary.
        let expected = [
            "tenant",
            "tenant_group",
            "region",
            "site",
            "site_group",
            "location",
            "prefix",
            "rack",
            "rack_role",
            "rack_type",
            "manufacturer",
            "device_role",
            "device_type",
            "platform",
            "device",
            "device_interface",
            "contact",
            "contact_assignment",
            "cluster",
            "cluster_type",
            "cluster_group",
            "virtual_machine",
            "vm_interface",
            "ip_address",
            "tunnel_group",
            "tunnel",
            "tunnel_termination",
        ];
        for value in expected {
            assert!(
                is_supported_resource_type(value),
                "expected supported resource type: {value}"
            );
        }
        assert_eq!(SUPPORTED_RESOURCE_TYPES.len(), expected.len());
    }

    #[test]
    fn supported_allow_list_rejects_retired_aliases_and_unknown_types() {
        for value in [
            "tenants",
            "sites",
            "devices",
            "virtual_machines",
            "ipam_ipaddress",
            "totally_unknown",
        ] {
            assert!(
                !is_supported_resource_type(value),
                "alias/unknown should be rejected: {value}"
            );
        }
    }

    #[test]
    fn supported_resource_types_csv_lists_every_key() {
        let csv = supported_resource_types_csv();
        for value in SUPPORTED_RESOURCE_TYPES {
            assert!(
                csv.contains(value),
                "csv error message should mention {value}: {csv}"
            );
        }
    }
}
