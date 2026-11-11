use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{Acquire, PgPool, Postgres, QueryBuilder, Transaction};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::errors::AppError;

use super::super::content_type_lookup::lookup_content_type_id_tx;
use super::super::helpers::derive_slug;
use super::ipam_repository::normalize_cidr;

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
    /// Every observed interface address as one CIDR per entry, IPv4 and IPv6 in
    /// the same list. The request boundary validates and canonicalizes them; an
    /// empty list is an absent observation, never an observed empty set.
    #[serde(default)]
    pub addresses: Vec<String>,
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
    /// When the source observed these facts. Defaults to the ingest time.
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    pub raw_facts: Option<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectedFactsGroupStatus {
    Success,
    Skipped,
    Error,
}

/// Per-group outcome of one ingest. A skipped group is a decision (policy off,
/// no data, manual override or stale observation), not a failure. An error is a
/// failed write for that group only; the remaining groups still commit.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CollectedFactsGroupOutcome {
    pub group: String,
    pub status: CollectedFactsGroupStatus,
    pub reason: Option<String>,
    pub updated: usize,
    pub skipped: usize,
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
    /// Effective observation time stored with the accepted fields.
    pub observed_at: DateTime<Utc>,
    /// Outcome per write group: success, skipped or error, with a reason.
    pub groups: Vec<CollectedFactsGroupOutcome>,
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

/// Source and time attached to every accepted field observation.
#[derive(Debug, Clone)]
struct FieldObservation {
    source_kind: String,
    source_id: Uuid,
    observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldDecision {
    Apply,
    ManualOverride,
    Stale,
}

/// What a single group did. `Skipped` carries a stable reason token.
enum GroupApply {
    Applied { updated: usize, skipped: usize },
    Skipped(&'static str),
}

/// Current stored values of the object-level fields this path owns.
struct ObjectFields {
    platform_id: Value,
    device_type_id: Value,
    serial: Value,
    vcpus: Value,
    memory: Value,
    disk: Value,
    ansible: Value,
}

impl SqlxCollectedFactsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn ingest(
        &self,
        job_token_id: Option<Uuid>,
        mut request: CollectedFactsIngestRequest,
    ) -> Result<CollectedFactsIngestResponse, AppError> {
        let observed_at = request.observed_at.unwrap_or_else(Utc::now);
        validate_observed_at(observed_at, Utc::now())?;
        normalize_request_addresses(&mut request)?;
        let mut tx = self.pool.begin().await?;
        let (source_kind, source_id) = if let Some(token_id) = job_token_id {
            Self::ensure_job_token_scope(&mut tx, token_id, &request).await?
        } else {
            ("run_group".to_string(), request.run_group_id)
        };
        let observation = FieldObservation {
            source_kind,
            source_id,
            observed_at,
        };
        Self::ensure_inventory_object_exists(&mut tx, &request).await?;

        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;

        let mut groups = Vec::new();
        let mut counters = IngestCounters::default();
        let mut platform_id = None;
        let mut device_type_id = None;

        groups.push(if !request.write_policy.write_platform {
            group_outcome(
                "platform",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else if request
            .platform
            .as_ref()
            .and_then(|value| clean_optional(value.name.as_deref()))
            .is_none()
        {
            group_outcome(
                "platform",
                CollectedFactsGroupStatus::Skipped,
                Some("no_data"),
                0,
                0,
            )
        } else {
            let mut savepoint = tx.begin().await?;
            match Self::apply_platform(&mut savepoint, &request, &observation).await {
                Ok((apply, resolved)) => {
                    savepoint.commit().await?;
                    platform_id = resolved;
                    apply_outcome("platform", apply)
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    tracing::warn!(group = "platform", error = %error, "collected facts group failed");
                    group_outcome(
                        "platform",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

        groups.push(if request.inventory_kind != CollectedFactsInventoryKind::Device {
            group_outcome(
                "device_type",
                CollectedFactsGroupStatus::Skipped,
                Some("not_applicable"),
                0,
                0,
            )
        } else if !request.write_policy.write_device_type {
            group_outcome(
                "device_type",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else if device_type_identity(request.hardware.as_ref()).is_none() {
            group_outcome(
                "device_type",
                CollectedFactsGroupStatus::Skipped,
                Some("no_data"),
                0,
                0,
            )
        } else {
            let mut savepoint = tx.begin().await?;
            match Self::apply_device_type(&mut savepoint, &request, &observation).await {
                Ok((apply, resolved)) => {
                    savepoint.commit().await?;
                    device_type_id = resolved;
                    apply_outcome("device_type", apply)
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    tracing::warn!(group = "device_type", error = %error, "collected facts group failed");
                    group_outcome(
                        "device_type",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

        groups.push(if !request.write_policy.write_serial {
            group_outcome(
                "serial",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else if collected_serial(&request).is_none() {
            group_outcome(
                "serial",
                CollectedFactsGroupStatus::Skipped,
                Some("no_data"),
                0,
                0,
            )
        } else {
            let mut savepoint = tx.begin().await?;
            match Self::apply_serial(&mut savepoint, &request, &observation).await {
                Ok(apply) => {
                    savepoint.commit().await?;
                    apply_outcome("serial", apply)
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    tracing::warn!(group = "serial", error = %error, "collected facts group failed");
                    group_outcome(
                        "serial",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

        groups.push(if request.inventory_kind != CollectedFactsInventoryKind::VirtualMachine {
            group_outcome(
                "resources",
                CollectedFactsGroupStatus::Skipped,
                Some("not_applicable"),
                0,
                0,
            )
        } else if !request.write_policy.write_resources {
            group_outcome(
                "resources",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else if !has_resource_facts(request.hardware.as_ref()) {
            group_outcome(
                "resources",
                CollectedFactsGroupStatus::Skipped,
                Some("no_data"),
                0,
                0,
            )
        } else {
            let mut savepoint = tx.begin().await?;
            match Self::apply_resources(&mut savepoint, &request, &observation).await {
                Ok(apply) => {
                    savepoint.commit().await?;
                    apply_outcome("resources", apply)
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    tracing::warn!(group = "resources", error = %error, "collected facts group failed");
                    group_outcome(
                        "resources",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

        groups.push(if !request.write_policy.write_ansible_custom_fields {
            group_outcome(
                "ansible",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else {
            let mut savepoint = tx.begin().await?;
            match Self::apply_ansible(&mut savepoint, &request, &observation).await {
                Ok(apply) => {
                    savepoint.commit().await?;
                    apply_outcome("ansible", apply)
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    tracing::warn!(group = "ansible", error = %error, "collected facts group failed");
                    group_outcome(
                        "ansible",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

        let interfaces_failed;
        groups.push(if !request.write_policy.write_interfaces {
            interfaces_failed = false;
            group_outcome(
                "interfaces",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else if request.interfaces.is_empty() {
            interfaces_failed = false;
            group_outcome(
                "interfaces",
                CollectedFactsGroupStatus::Skipped,
                Some("no_data"),
                0,
                0,
            )
        } else {
            let before_interface = counters.interface_count;
            let before_ip = counters.ip_address_count;
            let before_mac = counters.mac_address_count;
            let before_skipped = counters.skipped_count;
            let before_conflict = counters.conflict_count;
            let mut savepoint = tx.begin().await?;
            match Self::upsert_interfaces(&mut savepoint, &request, &mut counters).await {
                Ok(()) => {
                    savepoint.commit().await?;
                    interfaces_failed = false;
                    group_outcome(
                        "interfaces",
                        CollectedFactsGroupStatus::Success,
                        None,
                        counters.interface_count,
                        counters.skipped_count,
                    )
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    counters.interface_count = before_interface;
                    counters.ip_address_count = before_ip;
                    counters.mac_address_count = before_mac;
                    counters.skipped_count = before_skipped;
                    counters.conflict_count = before_conflict;
                    tracing::warn!(group = "interfaces", error = %error, "collected facts group failed");
                    interfaces_failed = true;
                    group_outcome(
                        "interfaces",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

        groups.push(group_outcome(
            "ip_addresses",
            if !request.write_policy.write_ip_addresses {
                CollectedFactsGroupStatus::Skipped
            } else if !request.write_policy.write_interfaces {
                CollectedFactsGroupStatus::Skipped
            } else if interfaces_failed {
                CollectedFactsGroupStatus::Error
            } else if !has_interface_addresses(&request, true) {
                CollectedFactsGroupStatus::Skipped
            } else {
                CollectedFactsGroupStatus::Success
            },
            if !request.write_policy.write_ip_addresses {
                Some("write_policy_disabled")
            } else if !request.write_policy.write_interfaces {
                Some("interfaces_disabled")
            } else if interfaces_failed {
                Some("write_failed")
            } else if !has_interface_addresses(&request, true) {
                Some("no_data")
            } else {
                None
            },
            counters.ip_address_count,
            0,
        ));

        groups.push(group_outcome(
            "mac_addresses",
            if !request.write_policy.write_mac_addresses {
                CollectedFactsGroupStatus::Skipped
            } else if !request.write_policy.write_interfaces {
                CollectedFactsGroupStatus::Skipped
            } else if interfaces_failed {
                CollectedFactsGroupStatus::Error
            } else if !has_interface_addresses(&request, false) {
                CollectedFactsGroupStatus::Skipped
            } else {
                CollectedFactsGroupStatus::Success
            },
            if !request.write_policy.write_mac_addresses {
                Some("write_policy_disabled")
            } else if !request.write_policy.write_interfaces {
                Some("interfaces_disabled")
            } else if interfaces_failed {
                Some("write_failed")
            } else if !has_interface_addresses(&request, false) {
                Some("no_data")
            } else {
                None
            },
            counters.mac_address_count,
            0,
        ));

        groups.push(if request.inventory_kind != CollectedFactsInventoryKind::VirtualMachine {
            group_outcome(
                "virtual_disks",
                CollectedFactsGroupStatus::Skipped,
                Some("not_applicable"),
                0,
                0,
            )
        } else if !request.write_policy.write_virtual_disks {
            group_outcome(
                "virtual_disks",
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled"),
                0,
                0,
            )
        } else if request.virtual_disks.is_empty() {
            group_outcome(
                "virtual_disks",
                CollectedFactsGroupStatus::Skipped,
                Some("no_data"),
                0,
                0,
            )
        } else {
            let before_disk = counters.disk_count;
            let before_skipped = counters.skipped_count;
            let mut savepoint = tx.begin().await?;
            match Self::upsert_virtual_disks(&mut savepoint, &request, &observation, &mut counters)
                .await
            {
                Ok(()) => {
                    savepoint.commit().await?;
                    group_outcome(
                        "virtual_disks",
                        CollectedFactsGroupStatus::Success,
                        None,
                        counters.disk_count,
                        counters.skipped_count,
                    )
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    counters.disk_count = before_disk;
                    counters.skipped_count = before_skipped;
                    tracing::warn!(group = "virtual_disks", error = %error, "collected facts group failed");
                    group_outcome(
                        "virtual_disks",
                        CollectedFactsGroupStatus::Error,
                        Some("write_failed"),
                        0,
                        0,
                    )
                }
            }
        });

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
            observed_at,
            groups,
        })
    }

    async fn ensure_job_token_scope(
        tx: &mut Transaction<'_, Postgres>,
        token_id: Uuid,
        request: &CollectedFactsIngestRequest,
    ) -> Result<(String, Uuid), AppError> {
        let run_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT jt.run_id
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
              AND jt.metadata->>'audience' = 'sandbox'
            "#,
        )
        .bind(token_id)
        .bind(request.run_id)
        .bind(request.run_group_id)
        .bind(request.connection_target_id)
        .bind(request.inventory_kind.as_token())
        .bind(request.inventory_id)
        .fetch_optional(&mut **tx)
        .await?;

        run_id
            .map(|run_id| ("playbook_run".to_string(), run_id))
            .ok_or_else(|| {
                AppError::Forbidden(
                    "playbook job token is not scoped to this inventory target".to_string(),
                )
            })
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

    async fn apply_platform(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        observation: &FieldObservation,
    ) -> Result<(GroupApply, Option<Uuid>), AppError> {
        let current =
            Self::read_object_fields(tx, request.inventory_kind, request.inventory_id).await?;
        match Self::decide_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "platform_id",
            &current.platform_id,
            observation,
        )
        .await?
        {
            FieldDecision::ManualOverride => {
                return Ok((GroupApply::Skipped("manual_override"), None));
            }
            FieldDecision::Stale => return Ok((GroupApply::Skipped("stale"), None)),
            FieldDecision::Apply => {}
        }
        let Some(platform_id) = Self::resolve_platform(tx, request.platform.as_ref()).await? else {
            return Ok((GroupApply::Skipped("no_data"), None));
        };
        Self::write_object_uuid(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "platform_id",
            platform_id,
        )
        .await?;
        Self::record_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "platform_id",
            observation,
            &json!(platform_id),
            false,
        )
        .await?;
        Ok((
            GroupApply::Applied {
                updated: 1,
                skipped: 0,
            },
            Some(platform_id),
        ))
    }

    async fn apply_device_type(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        observation: &FieldObservation,
    ) -> Result<(GroupApply, Option<Uuid>), AppError> {
        let current =
            Self::read_object_fields(tx, request.inventory_kind, request.inventory_id).await?;
        match Self::decide_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "device_type_id",
            &current.device_type_id,
            observation,
        )
        .await?
        {
            FieldDecision::ManualOverride => {
                return Ok((GroupApply::Skipped("manual_override"), None));
            }
            FieldDecision::Stale => return Ok((GroupApply::Skipped("stale"), None)),
            FieldDecision::Apply => {}
        }
        let Some(device_type_id) = Self::resolve_device_type(tx, request.hardware.as_ref()).await?
        else {
            return Ok((GroupApply::Skipped("no_data"), None));
        };
        Self::write_object_uuid(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "device_type_id",
            device_type_id,
        )
        .await?;
        Self::record_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "device_type_id",
            observation,
            &json!(device_type_id),
            false,
        )
        .await?;
        Ok((
            GroupApply::Applied {
                updated: 1,
                skipped: 0,
            },
            Some(device_type_id),
        ))
    }

    async fn apply_serial(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        observation: &FieldObservation,
    ) -> Result<GroupApply, AppError> {
        let current =
            Self::read_object_fields(tx, request.inventory_kind, request.inventory_id).await?;
        let Some(serial) = collected_serial(request) else {
            return Ok(GroupApply::Skipped("no_data"));
        };
        match Self::decide_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "serial",
            &current.serial,
            observation,
        )
        .await?
        {
            FieldDecision::ManualOverride => return Ok(GroupApply::Skipped("manual_override")),
            FieldDecision::Stale => return Ok(GroupApply::Skipped("stale")),
            FieldDecision::Apply => {}
        }
        let sql = match request.inventory_kind {
            CollectedFactsInventoryKind::Device => {
                "UPDATE dcim_device SET serial = $2 WHERE id = $1"
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                "UPDATE virtualization_virtualmachine SET serial = $2 WHERE id = $1"
            }
        };
        sqlx::query(sql)
            .bind(request.inventory_id)
            .bind(&serial)
            .execute(&mut **tx)
            .await?;
        Self::record_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "serial",
            observation,
            &json!(serial),
            false,
        )
        .await?;
        Ok(GroupApply::Applied {
            updated: 1,
            skipped: 0,
        })
    }

    async fn apply_resources(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        observation: &FieldObservation,
    ) -> Result<GroupApply, AppError> {
        let current =
            Self::read_object_fields(tx, request.inventory_kind, request.inventory_id).await?;
        let hardware = request.hardware.as_ref();
        let incoming = [
            (
                "resources.vcpus",
                "vcpus",
                hardware
                    .and_then(|value| positive_i32(value.cpu_count))
                    .map(|value| json!(value)),
                &current.vcpus,
            ),
            (
                "resources.memory",
                "memory",
                hardware
                    .and_then(|value| positive_i32(value.memory_mb))
                    .map(|value| json!(value)),
                &current.memory,
            ),
            (
                "resources.disk",
                "disk",
                hardware
                    .and_then(|value| positive_i32(value.disk_mb))
                    .map(|mb| json!(mb_to_gb(mb))),
                &current.disk,
            ),
        ];
        let mut updated = 0;
        let mut skipped = 0;
        let mut reason = None;
        for (field_key, column, value, current_value) in incoming {
            let Some(value) = value else { continue };
            match Self::decide_field(
                tx,
                request.inventory_kind,
                request.inventory_id,
                field_key,
                current_value,
                observation,
            )
            .await?
            {
                FieldDecision::Apply => {
                    Self::write_resource_field(tx, request.inventory_id, column, &value).await?;
                    Self::record_field(
                        tx,
                        request.inventory_kind,
                        request.inventory_id,
                        field_key,
                        observation,
                        &value,
                        false,
                    )
                    .await?;
                    updated += 1;
                }
                FieldDecision::ManualOverride => {
                    skipped += 1;
                    reason.get_or_insert("manual_override");
                }
                FieldDecision::Stale => {
                    skipped += 1;
                    reason.get_or_insert("stale");
                }
            }
        }
        if updated == 0 {
            Ok(GroupApply::Skipped(reason.unwrap_or("no_data")))
        } else {
            Ok(GroupApply::Applied { updated, skipped })
        }
    }

    async fn apply_ansible(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        observation: &FieldObservation,
    ) -> Result<GroupApply, AppError> {
        let current =
            Self::read_object_fields(tx, request.inventory_kind, request.inventory_id).await?;
        match Self::decide_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "ansible",
            &current.ansible,
            observation,
        )
        .await?
        {
            FieldDecision::ManualOverride => return Ok(GroupApply::Skipped("manual_override")),
            FieldDecision::Stale => return Ok(GroupApply::Skipped("stale")),
            FieldDecision::Apply => {}
        }
        let context = Self::build_ansible_context(request);
        let sql = match request.inventory_kind {
            CollectedFactsInventoryKind::Device => {
                r#"
                UPDATE dcim_device
                SET local_context_data = jsonb_set(
                    COALESCE(local_context_data, '{}'::jsonb), '{ansible}', $2, true
                )
                WHERE id = $1
                "#
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                r#"
                UPDATE virtualization_virtualmachine
                SET local_context_data = jsonb_set(
                    COALESCE(local_context_data, '{}'::jsonb), '{ansible}', $2, true
                )
                WHERE id = $1
                "#
            }
        };
        sqlx::query(sql)
            .bind(request.inventory_id)
            .bind(&context)
            .execute(&mut **tx)
            .await?;
        Self::record_field(
            tx,
            request.inventory_kind,
            request.inventory_id,
            "ansible",
            observation,
            &context,
            false,
        )
        .await?;
        Ok(GroupApply::Applied {
            updated: 1,
            skipped: 0,
        })
    }

    async fn read_object_fields(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        inventory_id: Uuid,
    ) -> Result<ObjectFields, AppError> {
        match kind {
            CollectedFactsInventoryKind::Device => {
                let (platform_id, device_type_id, serial, ansible) =
                    sqlx::query_as::<_, (Value, Value, Value, Value)>(
                        r#"
                        SELECT COALESCE(to_jsonb(platform_id), 'null'::jsonb),
                               COALESCE(to_jsonb(device_type_id), 'null'::jsonb),
                               COALESCE(to_jsonb(serial), 'null'::jsonb),
                               COALESCE(local_context_data->'ansible', 'null'::jsonb)
                        FROM dcim_device
                        WHERE id = $1
                        "#,
                    )
                    .bind(inventory_id)
                    .fetch_one(&mut **tx)
                    .await?;
                Ok(ObjectFields {
                    platform_id,
                    device_type_id,
                    serial,
                    vcpus: Value::Null,
                    memory: Value::Null,
                    disk: Value::Null,
                    ansible,
                })
            }
            CollectedFactsInventoryKind::VirtualMachine => {
                let (platform_id, serial, vcpus, memory, disk, ansible) =
                    sqlx::query_as::<_, (Value, Value, Value, Value, Value, Value)>(
                        r#"
                        SELECT COALESCE(to_jsonb(platform_id), 'null'::jsonb),
                               COALESCE(to_jsonb(serial), 'null'::jsonb),
                               COALESCE(to_jsonb(vcpus), 'null'::jsonb),
                               COALESCE(to_jsonb(memory), 'null'::jsonb),
                               COALESCE(to_jsonb(disk), 'null'::jsonb),
                               COALESCE(local_context_data->'ansible', 'null'::jsonb)
                        FROM virtualization_virtualmachine
                        WHERE id = $1
                        "#,
                    )
                    .bind(inventory_id)
                    .fetch_one(&mut **tx)
                    .await?;
                Ok(ObjectFields {
                    platform_id,
                    device_type_id: Value::Null,
                    serial,
                    vcpus,
                    memory,
                    disk,
                    ansible,
                })
            }
        }
    }

    async fn write_object_uuid(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        inventory_id: Uuid,
        column: &str,
        value: Uuid,
    ) -> Result<(), AppError> {
        let sql = match (kind, column) {
            (CollectedFactsInventoryKind::Device, "platform_id") => {
                "UPDATE dcim_device SET platform_id = $2 WHERE id = $1"
            }
            (CollectedFactsInventoryKind::Device, "device_type_id") => {
                "UPDATE dcim_device SET device_type_id = $2 WHERE id = $1"
            }
            (CollectedFactsInventoryKind::VirtualMachine, "platform_id") => {
                "UPDATE virtualization_virtualmachine SET platform_id = $2 WHERE id = $1"
            }
            _ => {
                return Err(AppError::Validation(format!(
                    "unsupported collected-facts column {column}"
                )));
            }
        };
        sqlx::query(sql)
            .bind(inventory_id)
            .bind(value)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn write_resource_field(
        tx: &mut Transaction<'_, Postgres>,
        inventory_id: Uuid,
        column: &str,
        value: &Value,
    ) -> Result<(), AppError> {
        let sql = match column {
            "vcpus" => "UPDATE virtualization_virtualmachine SET vcpus = $2 WHERE id = $1",
            "memory" => "UPDATE virtualization_virtualmachine SET memory = $2 WHERE id = $1",
            "disk" => "UPDATE virtualization_virtualmachine SET disk = $2 WHERE id = $1",
            _ => {
                return Err(AppError::Validation(format!(
                    "unsupported collected-facts resource column {column}"
                )));
            }
        };
        let number = value.as_i64().ok_or_else(|| {
            AppError::Validation("collected resource value is not an integer".to_string())
        })?;
        sqlx::query(sql)
            .bind(inventory_id)
            .bind(number as i32)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn decide_field(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        inventory_id: Uuid,
        field_key: &str,
        current: &Value,
        observation: &FieldObservation,
    ) -> Result<FieldDecision, AppError> {
        let existing = sqlx::query_as::<_, (Option<Value>, DateTime<Utc>)>(
            r#"
            SELECT observed_value, observed_at
            FROM inventory_collected_field
            WHERE inventory_kind = $1 AND inventory_id = $2 AND field_key = $3
            "#,
        )
        .bind(kind.as_token())
        .bind(inventory_id)
        .bind(field_key)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((observed_value, observed_at)) = existing else {
            return Ok(FieldDecision::Apply);
        };
        let Some(observed_value) = observed_value else {
            return Ok(FieldDecision::Apply);
        };
        if !json_equivalent(current, &observed_value) {
            return Ok(FieldDecision::ManualOverride);
        }
        if observation.observed_at <= observed_at {
            return Ok(FieldDecision::Stale);
        }
        Ok(FieldDecision::Apply)
    }

    async fn record_field(
        tx: &mut Transaction<'_, Postgres>,
        kind: CollectedFactsInventoryKind,
        inventory_id: Uuid,
        field_key: &str,
        observation: &FieldObservation,
        value: &Value,
        absent: bool,
    ) -> Result<(), AppError> {
        let observed: Option<Value> = Some(value.clone());
        sqlx::query(
            r#"
            INSERT INTO inventory_collected_field (
                inventory_kind, inventory_id, field_key, source_kind, source_id,
                observed_at, observed_value, absent
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (inventory_kind, inventory_id, field_key)
            DO UPDATE SET
                source_kind = EXCLUDED.source_kind,
                source_id = EXCLUDED.source_id,
                observed_at = EXCLUDED.observed_at,
                observed_value = EXCLUDED.observed_value,
                absent = EXCLUDED.absent,
                updated_at = now()
            "#,
        )
        .bind(kind.as_token())
        .bind(inventory_id)
        .bind(field_key)
        .bind(&observation.source_kind)
        .bind(observation.source_id)
        .bind(observation.observed_at)
        .bind(observed)
        .bind(absent)
        .execute(&mut **tx)
        .await?;
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
        for address in &item.addresses {
            match Self::upsert_ip_address(tx, content_type_id, interface_id, address, run_group_id)
                .await?
            {
                Some(id) => {
                    if is_ipv4_cidr(address) {
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
                        mark_connected, enabled, _name, type, mgmt_only, device_id, mtu
                    )
                    VALUES (
                        uuidv7(), '{}'::jsonb, $1, $2, $3,
                        false, $4, $1, $5, false, $6, $7
                    )
                    ON CONFLICT ON CONSTRAINT dcim_interface_unique_device_name
                    DO UPDATE SET
                        label = EXCLUDED.label,
                        description = EXCLUDED.description,
                        enabled = EXCLUDED.enabled,
                        type = EXCLUDED.type,
                        mtu = COALESCE(EXCLUDED.mtu, dcim_interface.mtu)
                    RETURNING id
                    "#,
                )
                .bind(&stored_name)
                .bind(label)
                .bind(description)
                .bind(item.enabled.unwrap_or(true))
                .bind(interface_type)
                .bind(inventory_id)
                .bind(item.mtu)
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
        sqlx::query(sql)
            .bind(inventory_id)
            .bind(ip_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn upsert_virtual_disks(
        tx: &mut Transaction<'_, Postgres>,
        request: &CollectedFactsIngestRequest,
        observation: &FieldObservation,
        counters: &mut IngestCounters,
    ) -> Result<(), AppError> {
        for disk in &request.virtual_disks {
            let Some(name) = clean_optional(Some(disk.name.as_str())) else {
                counters.skipped_count += 1;
                continue;
            };
            let stored_name = bounded(&name, 64);
            let description = clean_optional(disk.description.as_deref()).unwrap_or_default();
            let field_key = format!("virtual_disk.{stored_name}.size");
            let current: Option<i32> = sqlx::query_scalar(
                "SELECT size FROM virtualization_virtualdisk WHERE virtual_machine_id = $1 AND name = $2",
            )
            .bind(request.inventory_id)
            .bind(&stored_name)
            .fetch_optional(&mut **tx)
            .await?;

            // Payload megabytes are converted to the model's gigabytes; an
            // absent, non-positive or sub-gigabyte size is not an observed zero
            // in a GB column, so it must not overwrite a known size.
            let observed_gb = disk
                .size_mb
                .filter(|value| *value > 0)
                .map(mb_to_gb)
                .filter(|gigabytes| *gigabytes > 0);
            let mut write_size = observed_gb;
            let mut record = None;
            if let Some(current_size) = current {
                let current_json = json!(current_size);
                match observed_gb {
                    Some(gigabytes) => {
                        match Self::decide_field(
                            tx,
                            CollectedFactsInventoryKind::VirtualMachine,
                            request.inventory_id,
                            &field_key,
                            &current_json,
                            observation,
                        )
                        .await?
                        {
                            FieldDecision::Apply => {
                                record = Some((json!(gigabytes), false));
                            }
                            FieldDecision::ManualOverride | FieldDecision::Stale => {
                                write_size = None;
                                counters.skipped_count += 1;
                            }
                        }
                    }
                    None => {
                        write_size = None;
                        match Self::decide_field(
                            tx,
                            CollectedFactsInventoryKind::VirtualMachine,
                            request.inventory_id,
                            &field_key,
                            &current_json,
                            observation,
                        )
                        .await?
                        {
                            FieldDecision::Apply => {
                                record = Some((current_json.clone(), true));
                            }
                            FieldDecision::ManualOverride | FieldDecision::Stale => {
                                counters.skipped_count += 1;
                            }
                        }
                    }
                }
            } else {
                record = Some(match observed_gb {
                    Some(value) => (json!(value), false),
                    None => (json!(0), true),
                });
            }

            let mut query = build_virtual_disk_upsert_query(
                stored_name,
                description,
                write_size,
                request.inventory_id,
            );
            query.build().execute(&mut **tx).await?;
            if let Some((value, absent)) = record {
                Self::record_field(
                    tx,
                    CollectedFactsInventoryKind::VirtualMachine,
                    request.inventory_id,
                    &field_key,
                    observation,
                    &value,
                    absent,
                )
                .await?;
            }
            counters.disk_count += 1;
        }
        Ok(())
    }
}

fn primary_ipv4_update_sql(kind: CollectedFactsInventoryKind) -> &'static str {
    match kind {
        CollectedFactsInventoryKind::Device => {
            "UPDATE dcim_device SET primary_ip4_id = $2 WHERE id = $1 AND primary_ip4_id IS NULL"
        }
        CollectedFactsInventoryKind::VirtualMachine => {
            "UPDATE virtualization_virtualmachine SET primary_ip4_id = $2 WHERE id = $1 AND primary_ip4_id IS NULL"
        }
    }
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
) -> QueryBuilder<Postgres> {
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

/// Model `disk` and virtual-disk `size` are gigabytes; the payload is megabytes.
fn mb_to_gb(megabytes: i32) -> i32 {
    megabytes / 1024
}

/// A small forward skew is allowed for clock drift; a future observation time
/// beyond it would let a caller pin ownership and reject later real runs.
const OBSERVATION_FUTURE_SKEW_SECONDS: i64 = 300;

fn validate_observed_at(observed_at: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), AppError> {
    if observed_at > now + chrono::Duration::seconds(OBSERVATION_FUTURE_SKEW_SECONDS) {
        return Err(AppError::Validation(
            "collected facts observed_at is in the future".to_string(),
        ));
    }
    Ok(())
}

fn json_equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        _ => left == right,
    }
}

fn collected_serial(request: &CollectedFactsIngestRequest) -> Option<String> {
    let hardware = request.hardware.as_ref()?;
    hardware
        .serial
        .as_deref()
        .and_then(|value| clean_optional(Some(value)))
        .or_else(|| {
            hardware
                .product_uuid
                .as_deref()
                .and_then(|value| clean_optional(Some(value)))
        })
        .filter(|value| !is_placeholder(value))
        .map(|value| bounded(&value, 50))
}

fn has_resource_facts(hardware: Option<&CollectedHardwareFacts>) -> bool {
    let Some(hardware) = hardware else {
        return false;
    };
    positive_i32(hardware.cpu_count).is_some()
        || positive_i32(hardware.memory_mb).is_some()
        || positive_i32(hardware.disk_mb).is_some()
}

fn has_interface_addresses(request: &CollectedFactsIngestRequest, ip: bool) -> bool {
    request.interfaces.iter().any(|item| {
        if ip {
            !item.addresses.is_empty()
        } else {
            item.mac
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        }
    })
}

fn group_outcome(
    group: &str,
    status: CollectedFactsGroupStatus,
    reason: Option<&str>,
    updated: usize,
    skipped: usize,
) -> CollectedFactsGroupOutcome {
    CollectedFactsGroupOutcome {
        group: group.to_string(),
        status,
        reason: reason.map(ToOwned::to_owned),
        updated,
        skipped,
    }
}

fn apply_outcome(group: &str, apply: GroupApply) -> CollectedFactsGroupOutcome {
    match apply {
        GroupApply::Applied { updated, skipped } => group_outcome(
            group,
            CollectedFactsGroupStatus::Success,
            None,
            updated,
            skipped,
        ),
        GroupApply::Skipped(reason) => group_outcome(
            group,
            CollectedFactsGroupStatus::Skipped,
            Some(reason),
            0,
            0,
        ),
    }
}

/// Validate every observed interface address at the request boundary and keep
/// one canonical CIDR per interface, in observation order. The parser, the
/// family and the prefix range come from the IPAM owner
/// (`ipam_repository::normalize_cidr`); an invalid address rejects the whole
/// request before any group write.
fn normalize_request_addresses(request: &mut CollectedFactsIngestRequest) -> Result<(), AppError> {
    for interface in &mut request.interfaces {
        let mut canonical: Vec<String> = Vec::with_capacity(interface.addresses.len());
        for address in std::mem::take(&mut interface.addresses) {
            let value = normalize_cidr(&address)?;
            if !canonical.contains(&value) {
                canonical.push(value);
            }
        }
        interface.addresses = canonical;
    }
    Ok(())
}

/// A canonical CIDR string carries a parsed address; read the family back
/// through the standard parser instead of a second address grammar.
fn is_ipv4_cidr(value: &str) -> bool {
    value
        .split_once('/')
        .and_then(|(address, _)| address.parse::<IpAddr>().ok())
        .is_some_and(|address| address.is_ipv4())
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
        let mut query = sqlx::QueryBuilder::<Postgres>::new("CREATE SCHEMA ");
        query.push(&schema);
        query
            .build()
            .execute(pool)
            .await
            .expect("create test schema");
        schema
    }

    async fn create_platform_table(pool: &PgPool, schema: &str) {
        let mut query = sqlx::QueryBuilder::<Postgres>::new("CREATE TABLE ");
        query.push(schema).push(
            r#".dcim_platform (
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
            "#,
        );
        query
            .build()
            .execute(pool)
            .await
            .expect("create platform test table");
        let mut query =
            sqlx::QueryBuilder::<Postgres>::new("CREATE UNIQUE INDEX dcim_platform_name_");
        query
            .push(schema)
            .push(" ON ")
            .push(schema)
            .push(".dcim_platform (name) WHERE manufacturer_id IS NULL");
        query
            .build()
            .execute(pool)
            .await
            .expect("create platform name index");
        let mut query =
            sqlx::QueryBuilder::<Postgres>::new("CREATE UNIQUE INDEX dcim_platform_slug_");
        query
            .push(schema)
            .push(" ON ")
            .push(schema)
            .push(".dcim_platform (slug) WHERE manufacturer_id IS NULL");
        query
            .build()
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
        let mut query = sqlx::QueryBuilder::<Postgres>::new("DROP SCHEMA ");
        query.push(schema).push(" CASCADE");
        query.build().execute(pool).await.expect("drop test schema");
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
        let mut query = sqlx::QueryBuilder::<Postgres>::new("CREATE TABLE ");
        query
            .push(&schema)
            .push(".dcim_device (id uuid PRIMARY KEY, primary_ip4_id uuid)");
        query
            .build()
            .execute(&pool)
            .await
            .expect("create device test table");
        let mut query = sqlx::QueryBuilder::<Postgres>::new("CREATE TABLE ");
        query
            .push(&schema)
            .push(".virtualization_virtualmachine (id uuid PRIMARY KEY, primary_ip4_id uuid)");
        query
            .build()
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
            let mut query = sqlx::QueryBuilder::<Postgres>::new("INSERT INTO ");
            query
                .push(table)
                .push(" (id, primary_ip4_id) VALUES ($1, $2), ($3, NULL)");
            query
                .build()
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
        let mut query = sqlx::QueryBuilder::<Postgres>::new("CREATE FUNCTION ");
        query.push(&schema).push(
            r#".delay_platform_insert()
            RETURNS trigger
            LANGUAGE plpgsql
            AS $$
            BEGIN
                PERFORM pg_sleep(0.2);
                RETURN NEW;
            END;
            $$
            "#,
        );
        query
            .build()
            .execute(&pool)
            .await
            .expect("create platform insert delay function");
        let mut query = sqlx::QueryBuilder::<Postgres>::new(
            "CREATE TRIGGER delay_platform_insert BEFORE INSERT ON ",
        );
        query
            .push(&schema)
            .push(".dcim_platform FOR EACH ROW EXECUTE FUNCTION ")
            .push(&schema)
            .push(".delay_platform_insert()");
        query
            .build()
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
        let mut query =
            sqlx::QueryBuilder::<Postgres>::new("SELECT COUNT(*), COUNT(DISTINCT tree_id) FROM ");
        query.push(&schema).push(".dcim_platform");
        let counts = query
            .build_query_as::<(i64, i64)>()
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

        assert!(
            !unknown_size_query
                .sql()
                .as_str()
                .contains("size = EXCLUDED.size")
        );
        assert!(
            known_size_query
                .sql()
                .as_str()
                .contains("size = EXCLUDED.size")
        );
    }

    #[test]
    fn payload_megabytes_are_converted_to_model_gigabytes() {
        assert_eq!(mb_to_gb(102_400), 100);
        assert_eq!(mb_to_gb(1024), 1);
        assert_eq!(mb_to_gb(512), 0);
    }

    #[test]
    fn future_observation_times_are_rejected() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("fixed timestamp");
        assert!(validate_observed_at(now, now).is_ok());
        assert!(
            validate_observed_at(now + chrono::Duration::seconds(60), now).is_ok(),
            "a small clock skew is accepted"
        );
        assert!(
            validate_observed_at(now + chrono::Duration::seconds(3600), now).is_err(),
            "a future observation must not pin ownership"
        );
    }

    #[test]
    fn numeric_representations_compare_equal() {
        assert!(json_equivalent(&json!(4), &json!(4.00)));
        assert!(json_equivalent(&json!("SER-1"), &json!("SER-1")));
        assert!(!json_equivalent(&json!("SER-1"), &json!("SER-2")));
        assert!(!json_equivalent(&json!(4), &Value::Null));
    }

    async fn create_collected_facts_tables(pool: &PgPool, schema: &str) {
        for statement in [
            format!(
                "CREATE TABLE {schema}.dcim_device (
                    id uuid PRIMARY KEY,
                    platform_id uuid,
                    device_type_id uuid,
                    serial varchar(50),
                    local_context_data jsonb
                )"
            ),
            format!(
                "CREATE TABLE {schema}.inventory_collected_field (
                    inventory_kind text NOT NULL,
                    inventory_id uuid NOT NULL,
                    field_key text NOT NULL,
                    source_kind text NOT NULL,
                    source_id uuid NOT NULL,
                    observed_at timestamp with time zone NOT NULL,
                    observed_value jsonb,
                    absent boolean NOT NULL DEFAULT false,
                    updated_at timestamp with time zone NOT NULL DEFAULT now(),
                    PRIMARY KEY (inventory_kind, inventory_id, field_key)
                )"
            ),
            format!(
                "CREATE FUNCTION {schema}.dcim_acquire_hardware_graph()
                 RETURNS void LANGUAGE sql AS 'SELECT 1'"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(pool)
                .await
                .expect("create collected facts test object");
        }
    }

    async fn create_virtualization_tables(pool: &PgPool, schema: &str) {
        for statement in [
            format!(
                "CREATE TABLE {schema}.virtualization_virtualmachine (
                    id uuid PRIMARY KEY,
                    platform_id uuid,
                    serial varchar(50),
                    vcpus numeric(6,2),
                    memory integer,
                    disk integer,
                    local_context_data jsonb
                )"
            ),
            format!(
                "CREATE TABLE {schema}.virtualization_virtualdisk (
                    id uuid PRIMARY KEY,
                    custom_field_data jsonb NOT NULL,
                    name varchar(64) NOT NULL,
                    description varchar(200) NOT NULL,
                    size integer NOT NULL CHECK (size >= 0),
                    virtual_machine_id uuid NOT NULL,
                    CONSTRAINT virtualization_virtualdisk_unique_virtual_machine_name
                        UNIQUE (virtual_machine_id, name)
                )"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(pool)
                .await
                .expect("create virtualization test object");
        }
    }

    fn group_status(
        response: &CollectedFactsIngestResponse,
        group: &str,
    ) -> Option<(CollectedFactsGroupStatus, Option<String>)> {
        response
            .groups
            .iter()
            .find(|outcome| outcome.group == group)
            .map(|outcome| (outcome.status, outcome.reason.clone()))
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn ingest_records_provenance_and_protects_manual_values() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_platform_table(&pool, &schema).await;
        create_collected_facts_tables(&pool, &schema).await;

        let url = std::env::var("PIPING_DATABASE__URL").expect("database url");
        let scoped_schema = schema.clone();
        let scoped_pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _| {
                let schema = scoped_schema.clone();
                Box::pin(async move {
                    sqlx::query(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .expect("connect scoped collected-facts pool");

        let device_id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id) VALUES ($1)")
            .bind(device_id)
            .execute(&scoped_pool)
            .await
            .expect("insert device");
        let run_group_id = Uuid::now_v7();
        let first_observed = DateTime::<Utc>::from_timestamp(Utc::now().timestamp(), 0)
            .expect("whole-second observation time");
        let request =
            |observed_at: DateTime<Utc>, serial: &str, platform_name: &str, slug: &str| {
                CollectedFactsIngestRequest {
                    run_group_id,
                    run_id: Uuid::now_v7(),
                    connection_target_id: Uuid::now_v7(),
                    inventory_kind: CollectedFactsInventoryKind::Device,
                    inventory_id: device_id,
                    target_name: Some("host".to_string()),
                    platform: Some(platform(platform_name, slug)),
                    hardware: Some(CollectedHardwareFacts {
                        serial: Some(serial.to_string()),
                        ..Default::default()
                    }),
                    interfaces: Vec::new(),
                    virtual_disks: Vec::new(),
                    write_policy: CollectedFactsWritePolicy::default(),
                    observed_at: Some(observed_at),
                    raw_facts: Some(json!({"ansible_hostname": "host"})),
                }
            };
        let repository = SqlxCollectedFactsRepository::new(scoped_pool.clone());

        let first = repository
            .ingest(
                None,
                request(first_observed, "SER-1", "Debian 13", "debian-13"),
            )
            .await
            .expect("first ingest");
        assert_eq!(first.observed_at, first_observed);
        assert_eq!(
            group_status(&first, "platform"),
            Some((CollectedFactsGroupStatus::Success, None))
        );
        assert_eq!(
            group_status(&first, "serial"),
            Some((CollectedFactsGroupStatus::Success, None))
        );
        assert_eq!(
            group_status(&first, "device_type"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("no_data".to_string())
            ))
        );
        assert_eq!(
            group_status(&first, "resources"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("not_applicable".to_string())
            ))
        );
        assert_eq!(
            group_status(&first, "interfaces"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("no_data".to_string())
            ))
        );
        assert_eq!(
            group_status(&first, "virtual_disks"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("not_applicable".to_string())
            ))
        );

        let (serial, platform_id): (String, Uuid) =
            sqlx::query_as("SELECT serial, platform_id FROM dcim_device WHERE id = $1")
                .bind(device_id)
                .fetch_one(&scoped_pool)
                .await
                .expect("read device after first ingest");
        assert_eq!(serial, "SER-1");

        let (source_kind, source_id, observed_at, observed_value): (
            String,
            Uuid,
            DateTime<Utc>,
            Value,
        ) = sqlx::query_as(
            r#"
            SELECT source_kind, source_id, observed_at, observed_value
            FROM inventory_collected_field
            WHERE inventory_kind = 'device' AND inventory_id = $1 AND field_key = 'serial'
            "#,
        )
        .bind(device_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read serial provenance");
        assert_eq!(source_kind, "run_group");
        assert_eq!(source_id, run_group_id);
        assert_eq!(observed_at, first_observed);
        assert_eq!(observed_value, json!("SER-1"));

        sqlx::query("UPDATE dcim_device SET serial = 'MANUAL' WHERE id = $1")
            .bind(device_id)
            .execute(&scoped_pool)
            .await
            .expect("simulate manual edit");
        sqlx::query(
            r#"
            UPDATE dcim_device
            SET local_context_data = jsonb_set(
                COALESCE(local_context_data, '{}'::jsonb), '{ansible}', '{"manual":true}'::jsonb, true
            )
            WHERE id = $1
            "#,
        )
        .bind(device_id)
        .execute(&scoped_pool)
        .await
        .expect("simulate manual ansible edit");
        let manual = repository
            .ingest(
                None,
                request(
                    first_observed + chrono::Duration::seconds(60),
                    "SER-2",
                    "Debian 13",
                    "debian-13",
                ),
            )
            .await
            .expect("second ingest");
        assert_eq!(
            group_status(&manual, "serial"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("manual_override".to_string())
            ))
        );
        assert_eq!(
            group_status(&manual, "ansible"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("manual_override".to_string())
            ))
        );
        let stored_ansible: Value = sqlx::query_scalar(
            "SELECT local_context_data->'ansible' FROM dcim_device WHERE id = $1",
        )
        .bind(device_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read preserved ansible context");
        assert_eq!(stored_ansible, json!({"manual": true}));
        let serial: String = sqlx::query_scalar("SELECT serial FROM dcim_device WHERE id = $1")
            .bind(device_id)
            .fetch_one(&scoped_pool)
            .await
            .expect("read preserved serial");
        assert_eq!(serial, "MANUAL");

        let stale = repository
            .ingest(
                None,
                request(
                    first_observed - chrono::Duration::seconds(60),
                    "SER-3",
                    "Debian 12",
                    "debian-12",
                ),
            )
            .await
            .expect("stale ingest");
        assert_eq!(
            group_status(&stale, "platform"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("stale".to_string())
            ))
        );
        let stored_platform: Uuid =
            sqlx::query_scalar("SELECT platform_id FROM dcim_device WHERE id = $1")
                .bind(device_id)
                .fetch_one(&scoped_pool)
                .await
                .expect("read preserved platform");
        assert_eq!(stored_platform, platform_id);

        scoped_pool.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn group_error_is_isolated_from_other_groups() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_platform_table(&pool, &schema).await;
        create_collected_facts_tables(&pool, &schema).await;

        let url = std::env::var("PIPING_DATABASE__URL").expect("database url");
        let scoped_schema = schema.clone();
        let scoped_pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _| {
                let schema = scoped_schema.clone();
                Box::pin(async move {
                    sqlx::query(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .expect("connect scoped collected-facts pool");

        let device_id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id) VALUES ($1)")
            .bind(device_id)
            .execute(&scoped_pool)
            .await
            .expect("insert device");
        // `dcim_manufacturer` is intentionally absent: resolving the device
        // type fails, while the other groups must still commit.
        let request = CollectedFactsIngestRequest {
            run_group_id: Uuid::now_v7(),
            run_id: Uuid::now_v7(),
            connection_target_id: Uuid::now_v7(),
            inventory_kind: CollectedFactsInventoryKind::Device,
            inventory_id: device_id,
            target_name: Some("host".to_string()),
            platform: Some(platform("Debian 13", "debian-13")),
            hardware: Some(CollectedHardwareFacts {
                model: Some("PowerEdge R650".to_string()),
                serial: Some("SER-9".to_string()),
                ..Default::default()
            }),
            interfaces: Vec::new(),
            virtual_disks: Vec::new(),
            write_policy: CollectedFactsWritePolicy::default(),
            observed_at: Some(Utc::now()),
            raw_facts: None,
        };
        let response = SqlxCollectedFactsRepository::new(scoped_pool.clone())
            .ingest(None, request)
            .await
            .expect("ingest with a failing group");
        assert_eq!(
            group_status(&response, "device_type"),
            Some((
                CollectedFactsGroupStatus::Error,
                Some("write_failed".to_string())
            ))
        );
        assert_eq!(
            group_status(&response, "serial"),
            Some((CollectedFactsGroupStatus::Success, None))
        );
        let serial: String = sqlx::query_scalar("SELECT serial FROM dcim_device WHERE id = $1")
            .bind(device_id)
            .fetch_one(&scoped_pool)
            .await
            .expect("read serial after isolated group error");
        assert_eq!(serial, "SER-9");

        scoped_pool.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn virtual_disk_size_is_converted_and_unknown_does_not_overwrite() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_collected_facts_tables(&pool, &schema).await;
        create_virtualization_tables(&pool, &schema).await;

        let url = std::env::var("PIPING_DATABASE__URL").expect("database url");
        let scoped_schema = schema.clone();
        let scoped_pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _| {
                let schema = scoped_schema.clone();
                Box::pin(async move {
                    sqlx::query(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .expect("connect scoped collected-facts pool");

        let vm_id = Uuid::now_v7();
        sqlx::query("INSERT INTO virtualization_virtualmachine (id) VALUES ($1)")
            .bind(vm_id)
            .execute(&scoped_pool)
            .await
            .expect("insert virtual machine");
        let repository = SqlxCollectedFactsRepository::new(scoped_pool.clone());
        let request =
            |size_mb: Option<i32>, observed_at: DateTime<Utc>| CollectedFactsIngestRequest {
                run_group_id: Uuid::now_v7(),
                run_id: Uuid::now_v7(),
                connection_target_id: Uuid::now_v7(),
                inventory_kind: CollectedFactsInventoryKind::VirtualMachine,
                inventory_id: vm_id,
                target_name: Some("vm".to_string()),
                platform: None,
                hardware: None,
                interfaces: Vec::new(),
                virtual_disks: vec![CollectedDiskFacts {
                    name: "data".to_string(),
                    size_mb,
                    description: Some("/dev/sdb".to_string()),
                }],
                write_policy: CollectedFactsWritePolicy::default(),
                observed_at: Some(observed_at),
                raw_facts: None,
            };
        let t1 = DateTime::<Utc>::from_timestamp(Utc::now().timestamp(), 0)
            .expect("whole-second observation time");

        repository
            .ingest(None, request(Some(102_400), t1))
            .await
            .expect("ingest known disk size");
        let size: i32 = sqlx::query_scalar(
            "SELECT size FROM virtualization_virtualdisk WHERE virtual_machine_id = $1 AND name = 'data'",
        )
        .bind(vm_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read converted disk size");
        assert_eq!(size, 100, "payload megabytes become model gigabytes");

        repository
            .ingest(None, request(None, t1 + chrono::Duration::seconds(60)))
            .await
            .expect("ingest unknown disk size");
        let size: i32 = sqlx::query_scalar(
            "SELECT size FROM virtualization_virtualdisk WHERE virtual_machine_id = $1 AND name = 'data'",
        )
        .bind(vm_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read preserved disk size");
        assert_eq!(size, 100, "an unknown size must not overwrite with zero");
        let (absent, observed_value): (bool, Value) = sqlx::query_as(
            r#"
            SELECT absent, observed_value
            FROM inventory_collected_field
            WHERE inventory_kind = 'virtual_machine' AND inventory_id = $1
              AND field_key = 'virtual_disk.data.size'
            "#,
        )
        .bind(vm_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read disk-size provenance");
        assert!(absent, "unknown size is recorded as absent");
        assert_eq!(observed_value, json!(100));

        repository
            .ingest(None, request(Some(512), t1 + chrono::Duration::seconds(90)))
            .await
            .expect("ingest sub-gigabyte disk size");
        let size: i32 = sqlx::query_scalar(
            "SELECT size FROM virtualization_virtualdisk WHERE virtual_machine_id = $1 AND name = 'data'",
        )
        .bind(vm_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read disk size after sub-gigabyte observation");
        assert_eq!(
            size, 100,
            "a sub-gigabyte observation must not overwrite a known size with zero"
        );

        repository
            .ingest(
                None,
                request(Some(2048), t1 + chrono::Duration::seconds(120)),
            )
            .await
            .expect("ingest newer disk size");
        let size: i32 = sqlx::query_scalar(
            "SELECT size FROM virtualization_virtualdisk WHERE virtual_machine_id = $1 AND name = 'data'",
        )
        .bind(vm_id)
        .fetch_one(&scoped_pool)
        .await
        .expect("read updated disk size");
        assert_eq!(size, 2, "a newer known size is applied");

        scoped_pool.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    async fn create_interface_tables(pool: &PgPool, schema: &str) {
        for statement in [
            format!(
                "CREATE TABLE {schema}.inventory_content_type (
                    id uuid PRIMARY KEY,
                    app_label text NOT NULL,
                    model text NOT NULL
                )"
            ),
            format!(
                "CREATE TABLE {schema}.dcim_interface (
                    id uuid PRIMARY KEY,
                    custom_field_data jsonb NOT NULL,
                    name varchar(64) NOT NULL,
                    label text NOT NULL DEFAULT '',
                    description text NOT NULL DEFAULT '',
                    mark_connected boolean NOT NULL DEFAULT false,
                    enabled boolean NOT NULL DEFAULT true,
                    _name varchar(64) NOT NULL,
                    type varchar(50) NOT NULL DEFAULT 'other',
                    mgmt_only boolean NOT NULL DEFAULT false,
                    device_id uuid NOT NULL,
                    mtu integer,
                    primary_mac_address_id uuid,
                    CONSTRAINT dcim_interface_unique_device_name UNIQUE (device_id, name)
                )"
            ),
            format!(
                "CREATE TABLE {schema}.virtualization_vminterface (
                    id uuid PRIMARY KEY,
                    custom_field_data jsonb NOT NULL,
                    enabled boolean NOT NULL DEFAULT true,
                    mtu integer,
                    name varchar(64) NOT NULL,
                    _name varchar(64) NOT NULL,
                    description text NOT NULL DEFAULT '',
                    virtual_machine_id uuid NOT NULL,
                    primary_mac_address_id uuid,
                    CONSTRAINT virtualization_vminterface_unique_virtual_machine_name
                        UNIQUE (virtual_machine_id, name)
                )"
            ),
            format!(
                "CREATE TABLE {schema}.ipam_ipaddress (
                    id uuid PRIMARY KEY,
                    custom_field_data jsonb NOT NULL,
                    address inet NOT NULL,
                    status text NOT NULL,
                    role text,
                    assigned_object_id uuid,
                    dns_name text NOT NULL DEFAULT '',
                    description text NOT NULL DEFAULT '',
                    assigned_object_type_id uuid,
                    comments text NOT NULL DEFAULT ''
                )"
            ),
            format!(
                "CREATE UNIQUE INDEX ipam_ipaddress_address ON {schema}.ipam_ipaddress (address)"
            ),
            format!(
                "CREATE TABLE {schema}.dcim_macaddress (
                    id uuid PRIMARY KEY,
                    custom_field_data jsonb NOT NULL,
                    mac_address macaddr NOT NULL,
                    assigned_object_type_id uuid,
                    assigned_object_id uuid,
                    description text NOT NULL DEFAULT '',
                    comments text NOT NULL DEFAULT ''
                )"
            ),
            format!(
                "CREATE UNIQUE INDEX dcim_macaddress_address ON {schema}.dcim_macaddress (mac_address)"
            ),
            format!("ALTER TABLE {schema}.dcim_device ADD COLUMN primary_ip4_id uuid"),
            format!(
                "ALTER TABLE {schema}.virtualization_virtualmachine ADD COLUMN primary_ip4_id uuid"
            ),
            format!(
                "INSERT INTO {schema}.inventory_content_type (id, app_label, model) VALUES
                    (uuidv7(), 'dcim', 'interface'),
                    (uuidv7(), 'virtualization', 'vminterface')"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(pool)
                .await
                .expect("create interface test object");
        }
    }

    async fn scoped_pool(schema: &str) -> PgPool {
        let url = std::env::var("PIPING_DATABASE__URL").expect("database url");
        let scoped_schema = schema.to_string();
        PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _| {
                let schema = scoped_schema.clone();
                Box::pin(async move {
                    sqlx::query(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .expect("connect scoped collected-facts pool")
    }

    fn device_interface_request(
        device_id: Uuid,
        run_group_id: Uuid,
        addresses: &[&str],
    ) -> CollectedFactsIngestRequest {
        CollectedFactsIngestRequest {
            run_group_id,
            run_id: Uuid::now_v7(),
            connection_target_id: Uuid::now_v7(),
            inventory_kind: CollectedFactsInventoryKind::Device,
            inventory_id: device_id,
            target_name: None,
            platform: None,
            hardware: None,
            interfaces: vec![CollectedInterfaceFacts {
                name: "eth0".to_string(),
                label: Some("eth0".to_string()),
                description: None,
                interface_type: None,
                enabled: Some(true),
                mtu: Some(1500),
                mac: Some("02:00:00:00:00:10".to_string()),
                addresses: addresses.iter().map(|value| value.to_string()).collect(),
            }],
            virtual_disks: Vec::new(),
            write_policy: CollectedFactsWritePolicy::default(),
            observed_at: None,
            raw_facts: None,
        }
    }

    async fn stored_addresses(pool: &PgPool, device_id: Uuid) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT address::text FROM ipam_ipaddress
             WHERE assigned_object_id = (SELECT id FROM dcim_interface WHERE device_id = $1)
             ORDER BY address",
        )
        .bind(device_id)
        .fetch_all(pool)
        .await
        .expect("read stored interface addresses")
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn two_ipv4_addresses_on_one_interface_store_both_and_leave_primary_empty() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_collected_facts_tables(&pool, &schema).await;
        create_virtualization_tables(&pool, &schema).await;
        create_interface_tables(&pool, &schema).await;
        let scoped = scoped_pool(&schema).await;

        let device_id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id) VALUES ($1)")
            .bind(device_id)
            .execute(&scoped)
            .await
            .expect("insert device");
        let repository = SqlxCollectedFactsRepository::new(scoped.clone());

        let response = repository
            .ingest(
                None,
                device_interface_request(
                    device_id,
                    Uuid::now_v7(),
                    &["10.10.0.10/24", "10.10.0.11/24"],
                ),
            )
            .await
            .expect("ingest two IPv4 addresses on one interface");
        assert_eq!(
            group_status(&response, "ip_addresses"),
            Some((CollectedFactsGroupStatus::Success, None))
        );
        assert_eq!(
            stored_addresses(&scoped, device_id).await,
            vec!["10.10.0.10/24", "10.10.0.11/24"],
            "both observed addresses are stored"
        );
        let primary: Option<Uuid> =
            sqlx::query_scalar("SELECT primary_ip4_id FROM dcim_device WHERE id = $1")
                .bind(device_id)
                .fetch_one(&scoped)
                .await
                .expect("read device primary IPv4");
        assert!(
            primary.is_none(),
            "two candidates on one interface are ambiguous and must not set primary"
        );

        scoped.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn manual_primary_ipv4_survives_a_new_single_candidate() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_collected_facts_tables(&pool, &schema).await;
        create_virtualization_tables(&pool, &schema).await;
        create_interface_tables(&pool, &schema).await;
        let scoped = scoped_pool(&schema).await;

        let device_id = Uuid::now_v7();
        let manual_primary = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id, primary_ip4_id) VALUES ($1, $2)")
            .bind(device_id)
            .bind(manual_primary)
            .execute(&scoped)
            .await
            .expect("insert device with a manual primary");
        let repository = SqlxCollectedFactsRepository::new(scoped.clone());

        repository
            .ingest(
                None,
                device_interface_request(device_id, Uuid::now_v7(), &["10.10.0.20/24"]),
            )
            .await
            .expect("ingest one IPv4 candidate");
        let primary: Option<Uuid> =
            sqlx::query_scalar("SELECT primary_ip4_id FROM dcim_device WHERE id = $1")
                .bind(device_id)
                .fetch_one(&scoped)
                .await
                .expect("read device primary IPv4");
        assert_eq!(
            primary,
            Some(manual_primary),
            "an existing manual primary is never replaced"
        );

        scoped.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn mixed_family_addresses_persist_once_and_a_true_singleton_sets_primary() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_collected_facts_tables(&pool, &schema).await;
        create_virtualization_tables(&pool, &schema).await;
        create_interface_tables(&pool, &schema).await;
        let scoped = scoped_pool(&schema).await;

        let device_id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id) VALUES ($1)")
            .bind(device_id)
            .execute(&scoped)
            .await
            .expect("insert device");
        let repository = SqlxCollectedFactsRepository::new(scoped.clone());

        let response = repository
            .ingest(
                None,
                device_interface_request(
                    device_id,
                    Uuid::now_v7(),
                    &[
                        "10.10.0.30/24",
                        "2001:DB8::30/64",
                        "10.10.0.30/24",
                        "2001:db8::30/64",
                    ],
                ),
            )
            .await
            .expect("ingest mixed families with duplicates");
        assert_eq!(
            stored_addresses(&scoped, device_id).await,
            vec!["10.10.0.30/24", "2001:db8::30/64"],
            "canonical equivalents collapse to one stored address per family"
        );
        let primary: Option<String> = sqlx::query_scalar(
            "SELECT address::text FROM ipam_ipaddress
             WHERE id = (SELECT primary_ip4_id FROM dcim_device WHERE id = $1)",
        )
        .bind(device_id)
        .fetch_optional(&scoped)
        .await
        .expect("read primary address");
        assert_eq!(
            primary.as_deref(),
            Some("10.10.0.30/24"),
            "one IPv4 candidate is unambiguous and sets the primary"
        );
        assert_eq!(
            group_status(&response, "ip_addresses"),
            Some((CollectedFactsGroupStatus::Success, None))
        );

        scoped.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn invalid_address_rejects_the_request_before_any_write() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_collected_facts_tables(&pool, &schema).await;
        create_virtualization_tables(&pool, &schema).await;
        create_interface_tables(&pool, &schema).await;
        let scoped = scoped_pool(&schema).await;

        let device_id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id) VALUES ($1)")
            .bind(device_id)
            .execute(&scoped)
            .await
            .expect("insert device");
        let repository = SqlxCollectedFactsRepository::new(scoped.clone());

        for invalid in ["10.10.0.40/33", "10.10.0.400/24", "not-an-address/24", "2001:db8::40/129"] {
            let error = repository
                .ingest(
                    None,
                    device_interface_request(device_id, Uuid::now_v7(), &["10.10.0.41/24", invalid]),
                )
                .await
                .expect_err("invalid address must be rejected");
            assert!(
                matches!(error, AppError::Validation(_)),
                "unexpected error for {invalid}"
            );
        }

        let interfaces: i64 =
            sqlx::query_scalar("SELECT count(*) FROM dcim_interface WHERE device_id = $1")
                .bind(device_id)
                .fetch_one(&scoped)
                .await
                .expect("count interfaces after rejected requests");
        assert_eq!(interfaces, 0, "a rejected request writes no interface");
        let addresses: i64 = sqlx::query_scalar("SELECT count(*) FROM ipam_ipaddress")
            .fetch_one(&scoped)
            .await
            .expect("count addresses after rejected requests");
        assert_eq!(addresses, 0, "a rejected request writes no address");
        let provenance: i64 = sqlx::query_scalar("SELECT count(*) FROM inventory_collected_field")
            .fetch_one(&scoped)
            .await
            .expect("count provenance after rejected requests");
        assert_eq!(provenance, 0, "a rejected request records no provenance");

        scoped.close().await;
        drop_test_schema(&pool, &schema).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL (run with the dev stack or in coverage:backend)"]
    async fn typed_disabled_address_flag_writes_no_addresses() {
        let pool = test_pool().await;
        let schema = create_test_schema(&pool).await;
        create_collected_facts_tables(&pool, &schema).await;
        create_virtualization_tables(&pool, &schema).await;
        create_interface_tables(&pool, &schema).await;
        let scoped = scoped_pool(&schema).await;

        let device_id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_device (id) VALUES ($1)")
            .bind(device_id)
            .execute(&scoped)
            .await
            .expect("insert device");
        let repository = SqlxCollectedFactsRepository::new(scoped.clone());

        let mut value = serde_json::to_value(device_interface_request(
            device_id,
            Uuid::now_v7(),
            &["10.10.0.50/24"],
        ))
        .expect("serialize request");
        value["write_policy"]["write_ip_addresses"] = json!(false);
        let request: CollectedFactsIngestRequest =
            serde_json::from_value(value).expect("typed request with a disabled address group");
        assert!(
            !request.write_policy.write_ip_addresses,
            "a typed false must survive deserialization"
        );

        let response = repository
            .ingest(None, request)
            .await
            .expect("ingest with disabled address group");
        assert_eq!(
            group_status(&response, "ip_addresses"),
            Some((
                CollectedFactsGroupStatus::Skipped,
                Some("write_policy_disabled".to_string())
            ))
        );
        let interfaces: i64 =
            sqlx::query_scalar("SELECT count(*) FROM dcim_interface WHERE device_id = $1")
                .bind(device_id)
                .fetch_one(&scoped)
                .await
                .expect("count interfaces");
        assert_eq!(interfaces, 1, "interfaces are still written");
        let addresses: i64 = sqlx::query_scalar("SELECT count(*) FROM ipam_ipaddress")
            .fetch_one(&scoped)
            .await
            .expect("count addresses");
        assert_eq!(addresses, 0, "a disabled address group writes no address");

        scoped.close().await;
        drop_test_schema(&pool, &schema).await;
    }
}
