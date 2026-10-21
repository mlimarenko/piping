use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::errors::AppError;

use super::super::content_type_lookup::lookup_content_type_id_tx;
use super::super::helpers::derive_slug;

const PLATFORM_CREATION_LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('piping.dcim_platform.tree_id', 0))";
const PLATFORM_INSERT_SQL: &str = r#"
    INSERT INTO dcim_platform (
        id, custom_field_data, name, slug, description, manufacturer_id,
        level, lft, rght, tree_id, comments
    )
    VALUES (
        uuidv7(), '{}'::jsonb, $1, $2, '', NULL,
        0, 1, 2, (SELECT COALESCE(MAX(tree_id), 0) + 1 FROM dcim_platform), ''
    )
    ON CONFLICT DO NOTHING
    RETURNING id
    "#;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectedFactsInventoryKind {
    Device,
    VirtualMachine,
}

impl CollectedFactsInventoryKind {
    fn as_token(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::VirtualMachine => "virtual_machine",
        }
    }

    fn interface_content_type(self) -> (&'static str, &'static str) {
        match self {
            Self::Device => ("dcim", "interface"),
            Self::VirtualMachine => ("virtualization", "vminterface"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, Default)]
pub struct CollectedPlatformFacts {
    pub name: Option<String>,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, Default)]
pub struct CollectedHardwareFacts {
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub product_uuid: Option<String>,
    pub cpu_count: Option<i32>,
    pub memory_mb: Option<i32>,
    pub disk_mb: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, Default)]
pub struct CollectedInterfaceFacts {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub interface_type: Option<String>,
    pub enabled: Option<bool>,
    pub mtu: Option<i32>,
    pub mac: Option<String>,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, Default)]
pub struct CollectedDiskFacts {
    pub name: String,
    pub size_mb: Option<i32>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CollectedFactsWritePolicy {
    #[serde(default = "default_true")]
    pub write_platform: bool,
    #[serde(default = "default_true")]
    pub write_device_type: bool,
    #[serde(default = "default_true")]
    pub write_serial: bool,
    #[serde(default = "default_true")]
    pub write_resources: bool,
    #[serde(default = "default_true")]
    pub write_interfaces: bool,
    #[serde(default = "default_true")]
    pub write_ip_addresses: bool,
    #[serde(default = "default_true")]
    pub write_mac_addresses: bool,
    #[serde(default = "default_true")]
    pub write_virtual_disks: bool,
    #[serde(default = "default_true")]
    pub write_ansible_custom_fields: bool,
}

impl Default for CollectedFactsWritePolicy {
    fn default() -> Self {
        Self {
            write_platform: true,
            write_device_type: true,
            write_serial: true,
            write_resources: true,
            write_interfaces: true,
            write_ip_addresses: true,
            write_mac_addresses: true,
            write_virtual_disks: true,
            write_ansible_custom_fields: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CollectedFactsIngestRequest {
    pub run_group_id: Uuid,
    pub run_id: Uuid,
    pub connection_target_id: Uuid,
    pub inventory_kind: CollectedFactsInventoryKind,
    pub inventory_id: Uuid,
    pub target_name: Option<String>,
    pub platform: Option<CollectedPlatformFacts>,
    pub hardware: Option<CollectedHardwareFacts>,
    #[serde(default)]
    pub interfaces: Vec<CollectedInterfaceFacts>,
    #[serde(default)]
    pub virtual_disks: Vec<CollectedDiskFacts>,
    #[serde(default)]
    pub write_policy: CollectedFactsWritePolicy,
    pub raw_facts: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CollectedFactsIngestResponse {
    pub ingested: bool,
    pub inventory_kind: CollectedFactsInventoryKind,
    pub inventory_id: Uuid,
    pub platform_id: Option<Uuid>,
    pub device_type_id: Option<Uuid>,
    pub interface_count: usize,
    pub ip_address_count: usize,
    pub mac_address_count: usize,
    pub disk_count: usize,
    pub conflict_count: usize,
    pub skipped_count: usize,
}

#[derive(Clone)]
pub struct SqlxCollectedFactsRepository {
    pool: PgPool,
}

#[derive(Default)]
struct IngestCounters {
    interface_count: usize,
    ip_address_count: usize,
    mac_address_count: usize,
    disk_count: usize,
    conflict_count: usize,
    skipped_count: usize,
}

#[derive(Default)]
struct PrimaryIpv4Candidate {
    id: Option<Uuid>,
    ambiguous: bool,
}

impl PrimaryIpv4Candidate {
    fn consider(&mut self, id: Uuid) {
        match self.id {
            None => self.id = Some(id),
            Some(existing) if existing != id => self.ambiguous = true,
            Some(_) => {}
        }
    }

    fn into_unambiguous(self) -> Option<Uuid> {
        (!self.ambiguous).then_some(self.id).flatten()
    }
}

/// Identifies the interface row and ingest run an interface's IPs belong to.
struct InterfaceIpTarget {
    content_type_id: Uuid,
    interface_id: Uuid,
    run_group_id: Uuid,
}

impl SqlxCollectedFactsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn ingest(
        &self,
        job_token_id: Option<Uuid>,
        request: CollectedFactsIngestRequest,
    ) -> Result<CollectedFactsIngestResponse, AppError> {
        let mut tx = self.pool.begin().await?;
        if let Some(token_id) = job_token_id {
            Self::ensure_job_token_scope(&mut tx, token_id, &request).await?;
        }
        Self::ensure_inventory_object_exists(&mut tx, &request).await?;

        let platform_id = if request.write_policy.write_platform {
            Self::resolve_platform(&mut tx, request.platform.as_ref()).await?
        } else {
            None
        };
        let device_type_id = if request.inventory_kind == CollectedFactsInventoryKind::Device
            && request.write_policy.write_device_type
        {
            Self::resolve_device_type(&mut tx, request.hardware.as_ref()).await?
        } else {
            None
        };

        Self::update_inventory_object(&mut tx, &request, platform_id, device_type_id).await?;

        let mut counters = IngestCounters::default();
        if request.write_policy.write_interfaces {
            Self::upsert_interfaces(&mut tx, &request, &mut counters).await?;
        }
        if request.inventory_kind == CollectedFactsInventoryKind::VirtualMachine
            && request.write_policy.write_virtual_disks
        {
            Self::upsert_virtual_disks(&mut tx, &request, &mut counters).await?;
        }

        tx.commit().await?;
        Ok(CollectedFactsIngestResponse {
            ingested: true,
            inventory_kind: request.inventory_kind,
            inventory_id: request.inventory_id,
            platform_id,
            device_type_id,
            interface_count: counters.interface_count,
            ip_address_count: counters.ip_address_count,
            mac_address_count: counters.mac_address_count,
            disk_count: counters.disk_count,
            conflict_count: counters.conflict_count,
            skipped_count: counters.skipped_count,
        })
    }

    async fn ensure_job_token_scope(
        tx: &mut Transaction<'_, Postgres>,
        token_id: Uuid,
        request: &CollectedFactsIngestRequest,
    ) -> Result<(), AppError> {
        let scoped = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM playbook_job_token jt
                JOIN playbook_run pr ON pr.id = jt.run_id
                WHERE jt.id = $1
                  AND jt.run_id = $2
                  AND pr.run_group_id = $3
                  AND jt.target_kind = 'connection_target'
                  AND jt.target_id = $4
                  AND jt.source_kind = $5
                  AND jt.source_id = $6
                  AND jt.status = 'active'
                  AND jt.revoked_at IS NULL
                  AND jt.expires_at > now()
            )
            "#,
        )
        .bind(token_id)
        .bind(request.run_id)
        .bind(request.run_group_id)
        .bind(request.connection_target_id)
        .bind(request.inventory_kind.as_token())
        .bind(request.inventory_id)
        .fetch_one(&mut **tx)
        .await?;

        if scoped {
            Ok(())
        } else {
            Err(AppError::Forbidden(
                "playbook job token is not scoped to this inventory target".to_string(),
            ))
        }
    }

    async fn ensure_inventory_object_exists(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
    ) -> Result<(), AppError> {
        let exists = match request.inventory_kind {
            CollectedFactsInventoryKind::Device => {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM dcim_device WHERE id = $1)",
                )
                .bind(request.inventory_id)
                .fetch_one(&mut **tx)
                .await?
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM virtualization_virtualmachine WHERE id = $1)",
                )
                .bind(request.inventory_id)
                .fetch_one(&mut **tx)
                .await?
            }
        };
        if exists {
            Ok(())
        } else {
            Err(AppError::NotFound(format!(
                "{} {} was not found",
                request.inventory_kind.as_token(),
                request.inventory_id
            )))
        }
    }

    async fn resolve_platform(
        tx: &mut Transaction<'_, Postgres>,
        platform: Option<&CollectedPlatformFacts>,
    ) -> Result<Option<Uuid>, AppError> {
        let Some(name) = platform
            .and_then(|value| clean_optional(value.name.as_deref()))
            .filter(|value| !is_placeholder(value))
        else {
            return Ok(None);
        };
        let slug = platform
            .and_then(|value| clean_optional(value.slug.as_deref()))
            .unwrap_or_else(|| bounded_slug(&name, 100));
        if let Some(id) = Self::find_platform(tx, &slug, &name).await? {
            return Ok(Some(id));
        }

        // Root tree IDs are allocated with MAX()+1 because the imported NetBox
        // schema has no sequence for them. Serialize only the rare creation path;
        // the second lookup avoids inserting after another creator wins the lock.
        sqlx::query(PLATFORM_CREATION_LOCK_SQL)
            .execute(&mut **tx)
            .await?;
        if let Some(id) = Self::find_platform(tx, &slug, &name).await? {
            return Ok(Some(id));
        }

        let inserted_id = sqlx::query_scalar::<_, Uuid>(PLATFORM_INSERT_SQL)
            .bind(bounded(&name, 100))
            .bind(bounded(&slug, 100))
            .fetch_optional(&mut **tx)
            .await?;
        if let Some(id) = inserted_id {
            return Ok(Some(id));
        }

        Self::find_platform(tx, &slug, &name)
            .await?
            .map(Some)
            .ok_or_else(|| {
                AppError::Database(
                    "platform insert conflicted without a matching platform".to_string(),
                )
            })
    }

    async fn find_platform(
        tx: &mut Transaction<'_, Postgres>,
        slug: &str,
        name: &str,
    ) -> Result<Option<Uuid>, AppError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM dcim_platform WHERE lower(slug) = lower($1) OR lower(name) = lower($2) ORDER BY id LIMIT 1",
        )
        .bind(slug)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(Into::into)
    }

    async fn resolve_device_type(
        tx: &mut Transaction<'_, Postgres>,
        hardware: Option<&CollectedHardwareFacts>,
    ) -> Result<Option<Uuid>, AppError> {
        let Some((manufacturer_name, model)) = device_type_identity(hardware) else {
            return Ok(None);
        };
        let manufacturer_id = Self::resolve_manufacturer(tx, &manufacturer_name).await?;
        let slug = bounded_slug(&model, 100);
        if let Some(id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id
            FROM dcim_devicetype
            WHERE manufacturer_id = $1
              AND (lower(slug) = lower($2) OR lower(model) = lower($3))
            ORDER BY id
            LIMIT 1
            "#,
        )
        .bind(manufacturer_id)
        .bind(&slug)
        .bind(&model)
        .fetch_optional(&mut **tx)
        .await?
        {
            return Ok(Some(id));
        }
        let id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO dcim_devicetype (
                id, custom_field_data, model, slug, part_number, u_height,
                is_full_depth, front_image, rear_image, comments, manufacturer_id,
                description, console_port_template_count, console_server_port_template_count,
                power_port_template_count, power_outlet_template_count,
                interface_template_count, front_port_template_count, rear_port_template_count,
                device_bay_template_count, module_bay_template_count,
                inventory_item_template_count, exclude_from_utilization, device_count
            )
            VALUES (
                uuidv7(), '{}'::jsonb, $1, $2, '', 1.0,
                false, '', '', '', $3,
                '', 0, 0,
                0, 0,
                0, 0, 0,
                0, 0,
                0, false, 0
            )
            ON CONFLICT ON CONSTRAINT dcim_devicetype_unique_manufacturer_slug
            DO UPDATE SET model = EXCLUDED.model
            RETURNING id
            "#,
        )
        .bind(bounded(&model, 100))
        .bind(bounded(&slug, 100))
        .bind(manufacturer_id)
        .fetch_one(&mut **tx)
        .await?;
        Ok(Some(id))
    }

    async fn resolve_manufacturer(
        tx: &mut Transaction<'_, Postgres>,
        name: &str,
    ) -> Result<Uuid, AppError> {
        let slug = bounded_slug(name, 100);
        if let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM dcim_manufacturer WHERE lower(slug) = lower($1) OR lower(name) = lower($2) ORDER BY id LIMIT 1",
        )
        .bind(&slug)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await?
        {
            return Ok(id);
        }
        let id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO dcim_manufacturer (
                id, custom_field_data, name, slug, description, comments
            )
            VALUES (uuidv7(), '{}'::jsonb, $1, $2, '', '')
            ON CONFLICT ON CONSTRAINT dcim_manufacturer_slug_key
            DO UPDATE SET name = EXCLUDED.name
            RETURNING id
            "#,
        )
        .bind(bounded(name, 100))
        .bind(bounded(&slug, 100))
        .fetch_one(&mut **tx)
        .await?;
        Ok(id)
    }

    async fn update_inventory_object(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        platform_id: Option<Uuid>,
        device_type_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        let hardware = request.hardware.as_ref();
        let serial = hardware
            .and_then(|value| clean_optional(value.serial.as_deref()))
            .or_else(|| hardware.and_then(|value| clean_optional(value.product_uuid.as_deref())))
            .filter(|value| !is_placeholder(value))
            .map(|value| bounded(&value, 50));
        let serial_binding = if request.write_policy.write_serial {
            serial
        } else {
            None
        };
        let local_context = if request.write_policy.write_ansible_custom_fields {
            Some(Self::build_ansible_context(request))
        } else {
            None
        };
        match request.inventory_kind {
            CollectedFactsInventoryKind::Device => {
                sqlx::query(
                    r#"
                    UPDATE dcim_device
                    SET platform_id = CASE WHEN $2::uuid IS NULL THEN platform_id ELSE $2 END,
                        device_type_id = CASE WHEN $3::uuid IS NULL THEN device_type_id ELSE $3 END,
                        serial = CASE WHEN $4::text IS NULL THEN serial ELSE $4 END,
                        local_context_data = CASE
                            WHEN $5::jsonb IS NULL THEN local_context_data
                            ELSE jsonb_set(COALESCE(local_context_data, '{}'::jsonb), '{ansible}', $5, true)
                        END
                    WHERE id = $1
                    "#,
                )
                .bind(request.inventory_id)
                .bind(platform_id)
                .bind(device_type_id)
                .bind(serial_binding)
                .bind(local_context)
                .execute(&mut **tx)
                .await?;
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                let (vcpus, memory, disk) =
                    resource_bindings(hardware, request.write_policy.write_resources);
                sqlx::query(
                    r#"
                    UPDATE virtualization_virtualmachine
                    SET platform_id = CASE WHEN $2::uuid IS NULL THEN platform_id ELSE $2 END,
                        serial = CASE WHEN $3::text IS NULL THEN serial ELSE $3 END,
                        vcpus = CASE WHEN $4::integer IS NULL THEN vcpus ELSE $4 END,
                        memory = CASE WHEN $5::integer IS NULL THEN memory ELSE $5 END,
                        disk = CASE WHEN $6::integer IS NULL THEN disk ELSE $6 END,
                        local_context_data = CASE
                            WHEN $7::jsonb IS NULL THEN local_context_data
                            ELSE jsonb_set(COALESCE(local_context_data, '{}'::jsonb), '{ansible}', $7, true)
                        END
                    WHERE id = $1
                    "#,
                )
                .bind(request.inventory_id)
                .bind(platform_id)
                .bind(serial_binding)
                .bind(vcpus)
                .bind(memory)
                .bind(disk)
                .bind(local_context)
                .execute(&mut **tx)
                .await?;
            }
        }
        Ok(())
    }

    fn build_ansible_context(request: &CollectedFactsIngestRequest) -> Value {
        let mut object = match request.raw_facts.clone() {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        object.insert(
            "collector".to_string(),
            json!("system-auto-update-inventory"),
        );
        object.insert("run_group_id".to_string(), json!(request.run_group_id));
        object.insert("run_id".to_string(), json!(request.run_id));
        object.insert(
            "connection_target_id".to_string(),
            json!(request.connection_target_id),
        );
        object.insert(
            "interface_count".to_string(),
            json!(request.interfaces.len()),
        );
        object.insert("disk_count".to_string(), json!(request.virtual_disks.len()));
        Value::Object(object)
    }

    async fn upsert_interfaces(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        counters: &mut IngestCounters,
    ) -> Result<(), AppError> {
        let (app_label, model) = request.inventory_kind.interface_content_type();
        let interface_content_type_id = lookup_content_type_id_tx(tx, app_label, model)
            .await?
            .ok_or_else(|| {
                AppError::Validation(format!("unknown content type {app_label}.{model}"))
            })?;

        let mut target_primary_ipv4 = PrimaryIpv4Candidate::default();
        for item in &request.interfaces {
            let Some(name) = clean_optional(Some(item.name.as_str())) else {
                counters.skipped_count += 1;
                continue;
            };
            if name == "lo" {
                counters.skipped_count += 1;
                continue;
            }
            let interface_id = Self::upsert_interface(
                tx,
                request.inventory_kind,
                request.inventory_id,
                item,
                &name,
            )
            .await?;
            counters.interface_count += 1;

            if request.write_policy.write_mac_addresses {
                Self::record_interface_mac(
                    tx,
                    request.inventory_kind,
                    interface_content_type_id,
                    interface_id,
                    item,
                    counters,
                )
                .await?;
            }

            if request.write_policy.write_ip_addresses {
                Self::record_interface_ips(
                    tx,
                    InterfaceIpTarget {
                        content_type_id: interface_content_type_id,
                        interface_id,
                        run_group_id: request.run_group_id,
                    },
                    item,
                    counters,
                    &mut target_primary_ipv4,
                )
                .await?;
            }
        }
        if let Some(ip_id) = target_primary_ipv4.into_unambiguous() {
            Self::set_primary_ipv4(tx, request.inventory_kind, request.inventory_id, ip_id).await?;
        }
        Ok(())
    }

    async fn record_interface_mac(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        content_type_id: Uuid,
        interface_id: Uuid,
        item: &CollectedInterfaceFacts,
        counters: &mut IngestCounters,
    ) -> Result<(), AppError> {
        let Some(mac) = validated_mac(item.mac.as_deref(), counters) else {
            return Ok(());
        };
        match Self::upsert_mac_address(tx, content_type_id, interface_id, &mac).await? {
            Some(mac_id) => {
                Self::set_interface_primary_mac(tx, kind, interface_id, mac_id).await?;
                counters.mac_address_count += 1;
            }
            None => counters.conflict_count += 1,
        }
        Ok(())
    }

    async fn record_interface_ips(
        tx: &mut Transaction<'_, Postgres>,
        target: InterfaceIpTarget,
        item: &CollectedInterfaceFacts,
        counters: &mut IngestCounters,
        primary_ipv4: &mut PrimaryIpv4Candidate,
    ) -> Result<(), AppError> {
        let InterfaceIpTarget {
            content_type_id,
            interface_id,
            run_group_id,
        } = target;
        for address in [item.ipv4.as_deref(), item.ipv6.as_deref()]
            .into_iter()
            .flatten()
        {
            let Some(address) = normalize_cidr(address) else {
                counters.skipped_count += 1;
                continue;
            };
            match Self::upsert_ip_address(tx, content_type_id, interface_id, &address, run_group_id)
                .await?
            {
                Some(id) => {
                    if address.contains('.') {
                        primary_ipv4.consider(id);
                    }
                    counters.ip_address_count += 1;
                }
                None => counters.conflict_count += 1,
            }
        }
        Ok(())
    }

    async fn upsert_interface(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        inventory_id: Uuid,
        item: &CollectedInterfaceFacts,
        name: &str,
    ) -> Result<Uuid, AppError> {
        let stored_name = bounded(name, 64);
        match kind {
            CollectedFactsInventoryKind::Device => {
                let label = clean_optional(item.label.as_deref()).unwrap_or_default();
                let description = clean_optional(item.description.as_deref()).unwrap_or_default();
                let interface_type = clean_optional(item.interface_type.as_deref())
                    .unwrap_or_else(|| "other".to_string());
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO dcim_interface (
                        id, custom_field_data, name, label, description,
                        mark_connected, enabled, _name, type, mgmt_only, device_id
                    )
                    VALUES (
                        uuidv7(), '{}'::jsonb, $1, $2, $3,
                        false, $4, $1, $5, false, $6
                    )
                    ON CONFLICT ON CONSTRAINT dcim_interface_unique_device_name
                    DO UPDATE SET
                        label = EXCLUDED.label,
                        description = EXCLUDED.description,
                        enabled = EXCLUDED.enabled,
                        type = EXCLUDED.type
                    RETURNING id
                    "#,
                )
                .bind(&stored_name)
                .bind(label)
                .bind(description)
                .bind(item.enabled.unwrap_or(true))
                .bind(interface_type)
                .bind(inventory_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                let description = clean_optional(item.description.as_deref()).unwrap_or_default();
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO virtualization_vminterface (
                        id, custom_field_data, enabled, mtu, name, _name,
                        description, virtual_machine_id
                    )
                    VALUES (uuidv7(), '{}'::jsonb, $1, $2, $3, $3, $4, $5)
                    ON CONFLICT ON CONSTRAINT virtualization_vminterface_unique_virtual_machine_name
                    DO UPDATE SET
                        description = EXCLUDED.description,
                        enabled = EXCLUDED.enabled,
                        mtu = EXCLUDED.mtu
                    RETURNING id
                "#,
                )
                .bind(item.enabled.unwrap_or(true))
                .bind(item.mtu)
                .bind(&stored_name)
                .bind(description)
                .bind(inventory_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)
            }
        }
    }

    async fn upsert_ip_address(
        tx: &mut Transaction<'_, Postgres>,
        content_type_id: Uuid,
        interface_id: Uuid,
        address: &str,
        run_group_id: Uuid,
    ) -> Result<Option<Uuid>, AppError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            r#"
            WITH existing AS (
                SELECT id, assigned_object_id
                FROM ipam_ipaddress
                WHERE address = $1::inet
                LIMIT 1
            ),
            updated AS (
                UPDATE ipam_ipaddress
                SET assigned_object_type_id = $2,
                    assigned_object_id = $3,
                    status = 'active',
                    description = CASE
                        WHEN description = '' THEN $4
                        ELSE description
                    END
                WHERE id IN (
                    SELECT id
                    FROM existing
                    WHERE assigned_object_id IS NULL OR assigned_object_id = $3
                )
                RETURNING id
            ),
            inserted AS (
                INSERT INTO ipam_ipaddress (
                    id, custom_field_data, address, status, role,
                    assigned_object_id, dns_name, description,
                    assigned_object_type_id, comments
                )
                SELECT uuidv7(), '{}'::jsonb, $1::inet, 'active', NULL,
                       $3, '', $4, $2, ''
                WHERE NOT EXISTS (SELECT 1 FROM existing)
                RETURNING id
            )
            SELECT id FROM updated
            UNION ALL
            SELECT id FROM inserted
            LIMIT 1
            "#,
        )
        .bind(address)
        .bind(content_type_id)
        .bind(interface_id)
        .bind(format!("Collected by Piping run {run_group_id}"))
        .fetch_optional(&mut **tx)
        .await?;
        Ok(id)
    }

    async fn upsert_mac_address(
        tx: &mut Transaction<'_, Postgres>,
        content_type_id: Uuid,
        interface_id: Uuid,
        mac: &str,
    ) -> Result<Option<Uuid>, AppError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            r#"
            WITH existing AS (
                SELECT id, assigned_object_id
                FROM dcim_macaddress
                WHERE mac_address = $1::macaddr
                LIMIT 1
            ),
            updated AS (
                UPDATE dcim_macaddress
                SET assigned_object_type_id = $2,
                    assigned_object_id = $3
                WHERE id IN (
                    SELECT id
                    FROM existing
                    WHERE assigned_object_id IS NULL OR assigned_object_id = $3
                )
                RETURNING id
            ),
            inserted AS (
                INSERT INTO dcim_macaddress (
                    id, custom_field_data, mac_address, assigned_object_type_id,
                    assigned_object_id, description, comments
                )
                SELECT uuidv7(), '{}'::jsonb, $1::macaddr, $2, $3, '', ''
                WHERE NOT EXISTS (SELECT 1 FROM existing)
                RETURNING id
            )
            SELECT id FROM updated
            UNION ALL
            SELECT id FROM inserted
            LIMIT 1
            "#,
        )
        .bind(mac)
        .bind(content_type_id)
        .bind(interface_id)
        .fetch_optional(&mut **tx)
        .await?;
        Ok(id)
    }

    async fn set_interface_primary_mac(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        interface_id: Uuid,
        mac_id: Uuid,
    ) -> Result<(), AppError> {
        let sql = match kind {
            CollectedFactsInventoryKind::Device => {
                "UPDATE dcim_interface SET primary_mac_address_id = $2 WHERE id = $1"
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                "UPDATE virtualization_vminterface SET primary_mac_address_id = $2 WHERE id = $1"
            }
        };
        sqlx::query(sql)
            .bind(interface_id)
            .bind(mac_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn set_primary_ipv4(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        inventory_id: Uuid,
        ip_id: Uuid,
    ) -> Result<(), AppError> {
        let sql = primary_ipv4_update_sql(kind);
        sqlx::query(&sql)
            .bind(inventory_id)
            .bind(ip_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn upsert_virtual_disks(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        counters: &mut IngestCounters,
    ) -> Result<(), AppError> {
        for disk in &request.virtual_disks {
            let Some(name) = clean_optional(Some(disk.name.as_str())) else {
                counters.skipped_count += 1;
                continue;
            };
            let stored_name = bounded(&name, 64);
            let description = clean_optional(disk.description.as_deref()).unwrap_or_default();
            let mut query = build_virtual_disk_upsert_query(
                stored_name,
                description,
                disk.size_mb,
                request.inventory_id,
            );
            query.build().execute(&mut **tx).await?;
            counters.disk_count += 1;
        }
        Ok(())
    }
}

fn primary_ipv4_update_sql(kind: CollectedFactsInventoryKind) -> String {
    let table = match kind {
        CollectedFactsInventoryKind::Device => "dcim_device",
        CollectedFactsInventoryKind::VirtualMachine => "virtualization_virtualmachine",
    };
    format!("UPDATE {table} SET primary_ip4_id = $2 WHERE id = $1 AND primary_ip4_id IS NULL")
}

fn default_true() -> bool {
    true
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn device_type_identity(hardware: Option<&CollectedHardwareFacts>) -> Option<(String, String)> {
    let model = hardware
        .and_then(|value| clean_optional(value.model.as_deref()))
        .filter(|value| !is_placeholder(value))?;
    let manufacturer = hardware
        .and_then(|value| clean_optional(value.manufacturer.as_deref()))
        .filter(|value| !is_placeholder(value))
        .unwrap_or_else(|| "Unknown".to_string());
    Some((manufacturer, model))
}

fn validated_mac(value: Option<&str>, counters: &mut IngestCounters) -> Option<String> {
    let mac = clean_optional(value)?;
    if is_valid_mac(&mac) {
        Some(mac)
    } else {
        counters.skipped_count += 1;
        None
    }
}

fn build_virtual_disk_upsert_query(
    name: String,
    description: String,
    size_mb: Option<i32>,
    virtual_machine_id: Uuid,
) -> QueryBuilder<'static, Postgres> {
    let mut query = QueryBuilder::new(
        r#"
        INSERT INTO virtualization_virtualdisk (
            id, custom_field_data, name, description, size, virtual_machine_id
        )
        VALUES (uuidv7(), '{}'::jsonb, "#,
    );
    query
        .push_bind(name)
        .push(", ")
        .push_bind(description)
        .push(", ");
    if let Some(size_mb) = size_mb {
        query.push_bind(size_mb);
    } else {
        query.push("0");
    }
    query.push(", ").push_bind(virtual_machine_id).push(
        r#")
        ON CONFLICT ON CONSTRAINT virtualization_virtualdisk_unique_virtual_machine_name
        DO UPDATE SET description = EXCLUDED.description"#,
    );
    if size_mb.is_some() {
        query.push(", size = EXCLUDED.size");
    }
    query
}

fn bounded(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn bounded_slug(value: &str, max: usize) -> String {
    let slug = derive_slug(value);
    let slug = slug
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    };
    bounded(&slug, max)
}

fn is_placeholder(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "none"
            | "null"
            | "unknown"
            | "default string"
            | "to be filled by o.e.m."
            | "to be filled by oem"
            | "system serial number"
            | "not specified"
            | "not available"
    )
}

fn positive_i32(value: Option<i32>) -> Option<i32> {
    value.filter(|value| *value > 0)
}

fn resource_bindings(
    hardware: Option<&CollectedHardwareFacts>,
    write_resources: bool,
) -> (Option<i32>, Option<i32>, Option<i32>) {
    if write_resources {
        (
            hardware.and_then(|value| positive_i32(value.cpu_count)),
            hardware.and_then(|value| positive_i32(value.memory_mb)),
            hardware.and_then(|value| positive_i32(value.disk_mb)),
        )
    } else {
        (None, None, None)
    }
}

fn normalize_cidr(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || !value.contains('/') {
        return None;
    }
    Some(value.to_string())
}

fn is_valid_mac(value: &str) -> bool {
    let parts = value.split(':').collect::<Vec<_>>();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use sqlx::postgres::PgPoolOptions;
    use tokio::sync::Barrier;

    use super::*;

    /// Pool for the tests below, which are `#[ignore]`d because they need a
    /// database. Absent configuration is a failure, not a skip: returning
    /// `None` here made every one of them report success while asserting
    /// nothing, which is exactly how a suite stops being evidence.
    async fn test_pool() -> PgPool {
        let database_url = std::env::var("PIPING_DATABASE__URL").expect(
            "these tests are #[ignore]d because they need PostgreSQL; run them with \
             PIPING_DATABASE__URL set (coverage:backend does, `make run-up` does locally)",
        );
        PgPoolOptions::new()
            .max_connections(6)
            .connect(&database_url)
            .await
            .expect("connect to test PostgreSQL")
    }

    async fn create_test_schema(pool: &PgPool) -> String {
        let schema = format!("collected_facts_{}", Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(pool)
            .await
            .expect("create test schema");
        schema
    }

    async fn create_platform_table(pool: &PgPool, schema: &str) {
        sqlx::query(&format!(
            r#"
            CREATE TABLE {schema}.dcim_platform (
                id uuid PRIMARY KEY,
                custom_field_data jsonb NOT NULL,
                name varchar(100) NOT NULL,
                slug varchar(100) NOT NULL,
                description varchar(200) NOT NULL,
                manufacturer_id uuid,
                level integer NOT NULL,
                lft integer NOT NULL,
                rght integer NOT NULL,
                tree_id integer NOT NULL,
                comments text NOT NULL,
                CONSTRAINT dcim_platform_manufacturer_name
                    UNIQUE (manufacturer_id, name),
                CONSTRAINT dcim_platform_manufacturer_slug
                    UNIQUE (manufacturer_id, slug)
            )
            "#
        ))
        .execute(pool)
        .await
        .expect("create platform test table");
        sqlx::query(&format!(
            "CREATE UNIQUE INDEX dcim_platform_name_{schema} ON {schema}.dcim_platform (name) WHERE manufacturer_id IS NULL"
        ))
        .execute(pool)
        .await
        .expect("create platform name index");
        sqlx::query(&format!(
            "CREATE UNIQUE INDEX dcim_platform_slug_{schema} ON {schema}.dcim_platform (slug) WHERE manufacturer_id IS NULL"
        ))
        .execute(pool)
        .await
        .expect("create platform slug index");
    }

    async fn set_search_path(tx: &mut Transaction<'_, Postgres>, schema: &str) {
        sqlx::query("SELECT set_config('search_path', $1, true)")
            .bind(schema)
            .execute(&mut **tx)
            .await
            .expect("set test search path");
    }

    async fn drop_test_schema(pool: &PgPool, schema: &str) {
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(pool)
            .await
            .expect("drop test schema");
    }

    fn platform(name: &str, slug: &str) -> CollectedPlatformFacts {
        CollectedPlatformFacts {
            name: Some(name.to_string()),
            slug: Some(slug.to_string()),
        }
    }

    async fn wait_for_transaction_lock(pool: &PgPool, application_name: &str) -> bool {
        for _ in 0..200 {
            let waiting = sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_stat_activity
                    WHERE application_name = $1
                      AND wait_event_type = 'Lock'
                )
                "#,
            )
            .bind(application_name)
            .fetch_one(pool)
            .await
            .expect("inspect resolver wait state");
            if waiting {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[test]
    fn primary_ipv4_candidate_rejects_ambiguous_interface_order() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);

        let mut single = PrimaryIpv4Candidate::default();
        single.consider(first);
        assert_eq!(single.into_unambiguous(), Some(first));

        for addresses in [[first, second], [second, first]] {
            let mut candidate = PrimaryIpv4Candidate::default();
            for address in addresses {
                candidate.consider(address);
            }
            assert_eq!(candidate.into_unambiguous(), None);
        }
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn primary_ipv4_update_only_fills_empty_values() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        sqlx::query(&format!(
            "CREATE TABLE {schema}.dcim_device (id uuid PRIMARY KEY, primary_ip4_id uuid)"
        ))
        .execute(&pool)
        .await
        .expect("create device test table");
        sqlx::query(&format!(
            "CREATE TABLE {schema}.virtualization_virtualmachine (id uuid PRIMARY KEY, primary_ip4_id uuid)"
        ))
        .execute(&pool)
        .await
        .expect("create virtual machine test table");

        let existing_ip = Uuid::from_u128(10);
        let collected_ip = Uuid::from_u128(11);
        let device_with_primary = Uuid::from_u128(20);
        let device_without_primary = Uuid::from_u128(21);
        let vm_with_primary = Uuid::from_u128(30);
        let vm_without_primary = Uuid::from_u128(31);
        let mut tx = pool.begin().await.expect("begin primary IP test");
        set_search_path(&mut tx, &schema).await;
        for (table, with_primary, without_primary) in [
            ("dcim_device", device_with_primary, device_without_primary),
            (
                "virtualization_virtualmachine",
                vm_with_primary,
                vm_without_primary,
            ),
        ] {
            sqlx::query(&format!(
                "INSERT INTO {table} (id, primary_ip4_id) VALUES ($1, $2), ($3, NULL)"
            ))
            .bind(with_primary)
            .bind(existing_ip)
            .bind(without_primary)
            .execute(&mut *tx)
            .await
            .expect("insert primary IP test rows");
        }

        for (kind, with_primary, without_primary) in [
            (
                CollectedFactsInventoryKind::Device,
                device_with_primary,
                device_without_primary,
            ),
            (
                CollectedFactsInventoryKind::VirtualMachine,
                vm_with_primary,
                vm_without_primary,
            ),
        ] {
            SqlxCollectedFactsRepository::set_primary_ipv4(
                &mut tx,
                kind,
                with_primary,
                collected_ip,
            )
            .await
            .expect("preserve existing primary IPv4");
            SqlxCollectedFactsRepository::set_primary_ipv4(
                &mut tx,
                kind,
                without_primary,
                collected_ip,
            )
            .await
            .expect("fill missing primary IPv4");
        }

        let device_values = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            "SELECT id, primary_ip4_id FROM dcim_device ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await
        .expect("read device primary IPv4 values");
        let vm_values = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            "SELECT id, primary_ip4_id FROM virtualization_virtualmachine ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await
        .expect("read virtual machine primary IPv4 values");
        tx.rollback().await.expect("rollback primary IP test");
        drop_test_schema(&pool, &schema).await;

        assert_eq!(
            device_values,
            vec![
                (device_with_primary, Some(existing_ip)),
                (device_without_primary, Some(collected_ip)),
            ]
        );
        assert_eq!(
            vm_values,
            vec![
                (vm_with_primary, Some(existing_ip)),
                (vm_without_primary, Some(collected_ip)),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn concurrent_platform_conflict_returns_winner() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_platform_table(&pool, &schema).await;
        let winner_id = Uuid::now_v7();
        let mut winner_tx = pool.begin().await.expect("begin winning platform insert");
        set_search_path(&mut winner_tx, &schema).await;
        sqlx::query(
            r#"
            INSERT INTO dcim_platform (
                id, custom_field_data, name, slug, description, manufacturer_id,
                level, lft, rght, tree_id, comments
            )
            VALUES ($1, '{}'::jsonb, 'Concurrent Linux', 'concurrent-linux', '', NULL,
                    0, 1, 2, 1, '')
            "#,
        )
        .bind(winner_id)
        .execute(&mut *winner_tx)
        .await
        .expect("insert uncommitted winning platform");

        let application_name = format!("collected-facts-conflict-{}", Uuid::now_v7().simple());
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let resolver_pool = pool.clone();
        let resolver_schema = schema.clone();
        let resolver_application_name = application_name.clone();
        let resolver = tokio::spawn(async move {
            let mut tx = resolver_pool
                .begin()
                .await
                .map_err(|error| error.to_string())?;
            set_search_path(&mut tx, &resolver_schema).await;
            sqlx::query("SELECT set_config('application_name', $1, true)")
                .bind(&resolver_application_name)
                .execute(&mut *tx)
                .await
                .map_err(|error| error.to_string())?;
            ready_tx.send(()).map_err(|_| "signal resolver readiness")?;
            let resolved = SqlxCollectedFactsRepository::resolve_platform(
                &mut tx,
                Some(&platform("Concurrent Linux", "concurrent-linux")),
            )
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "platform was not resolved".to_string())?;
            tx.commit().await.map_err(|error| error.to_string())?;
            Ok::<Uuid, String>(resolved)
        });
        ready_rx.await.expect("resolver readiness");
        let observed_conflict_wait = wait_for_transaction_lock(&pool, &application_name).await;
        winner_tx
            .commit()
            .await
            .expect("commit winning platform insert");
        let resolved_id = resolver
            .await
            .expect("join concurrent resolver")
            .expect("resolve concurrent platform");
        drop_test_schema(&pool, &schema).await;

        assert!(
            observed_conflict_wait,
            "resolver never reached the concurrent unique-key conflict"
        );
        assert_eq!(resolved_id, winner_id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn concurrent_new_platforms_receive_distinct_tree_ids() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_platform_table(&pool, &schema).await;
        sqlx::query(&format!(
            r#"
            CREATE FUNCTION {schema}.delay_platform_insert()
            RETURNS trigger
            LANGUAGE plpgsql
            AS $$
            BEGIN
                PERFORM pg_sleep(0.2);
                RETURN NEW;
            END;
            $$
            "#
        ))
        .execute(&pool)
        .await
        .expect("create platform insert delay function");
        sqlx::query(&format!(
            "CREATE TRIGGER delay_platform_insert BEFORE INSERT ON {schema}.dcim_platform FOR EACH ROW EXECUTE FUNCTION {schema}.delay_platform_insert()"
        ))
        .execute(&pool)
        .await
        .expect("create platform insert delay trigger");

        let barrier = Arc::new(Barrier::new(3));
        let mut resolvers = Vec::new();
        for (name, slug) in [
            ("Concurrent A", "concurrent-a"),
            ("Concurrent B", "concurrent-b"),
        ] {
            let resolver_pool = pool.clone();
            let resolver_schema = schema.clone();
            let resolver_barrier = Arc::clone(&barrier);
            resolvers.push(tokio::spawn(async move {
                let mut tx = resolver_pool
                    .begin()
                    .await
                    .map_err(|error| error.to_string())?;
                set_search_path(&mut tx, &resolver_schema).await;
                resolver_barrier.wait().await;
                let id = SqlxCollectedFactsRepository::resolve_platform(
                    &mut tx,
                    Some(&platform(name, slug)),
                )
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "platform was not resolved".to_string())?;
                tx.commit().await.map_err(|error| error.to_string())?;
                Ok::<Uuid, String>(id)
            }));
        }
        barrier.wait().await;
        let first = resolvers
            .remove(0)
            .await
            .expect("join first platform resolver");
        let second = resolvers
            .remove(0)
            .await
            .expect("join second platform resolver");
        let counts = sqlx::query_as::<_, (i64, i64)>(&format!(
            "SELECT COUNT(*), COUNT(DISTINCT tree_id) FROM {schema}.dcim_platform"
        ))
        .fetch_one(&pool)
        .await
        .expect("read allocated platform tree IDs");
        drop_test_schema(&pool, &schema).await;

        let first_id = first.expect("resolve first platform");
        let second_id = second.expect("resolve second platform");
        assert_ne!(first_id, second_id);
        assert_eq!(counts, (2, 2));
    }

    #[test]
    fn placeholder_model_does_not_resolve_to_a_device_type() {
        let hardware = CollectedHardwareFacts {
            manufacturer: Some("Hikvision".to_string()),
            model: Some("To Be Filled By O.E.M.".to_string()),
            ..CollectedHardwareFacts::default()
        };

        assert_eq!(device_type_identity(Some(&hardware)), None);
    }

    #[test]
    fn invalid_mac_is_distinguishable_from_missing_mac() {
        let mut missing_mac_counters = IngestCounters::default();
        let mut invalid_mac_counters = IngestCounters::default();

        assert_eq!(validated_mac(None, &mut missing_mac_counters), None);
        assert_eq!(missing_mac_counters.skipped_count, 0);
        assert_eq!(
            validated_mac(Some("00-50-56-AD-6C-1D"), &mut invalid_mac_counters),
            None
        );
        assert_eq!(invalid_mac_counters.skipped_count, 1);
    }

    #[test]
    fn unknown_virtual_disk_size_is_omitted_from_conflict_update() {
        let unknown_size_query =
            build_virtual_disk_upsert_query("data".to_string(), String::new(), None, Uuid::nil());
        let known_size_query = build_virtual_disk_upsert_query(
            "data".to_string(),
            String::new(),
            Some(512_000),
            Uuid::nil(),
        );

        assert!(!unknown_size_query.sql().contains("size = EXCLUDED.size"));
        assert!(known_size_query.sql().contains("size = EXCLUDED.size"));
    }
}
