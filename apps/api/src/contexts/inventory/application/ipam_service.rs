use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;

use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::contexts::inventory::domain::content_type;
use crate::contexts::inventory::infrastructure::persistence::repositories::content_type_lookup::lookup_content_type_id_tx;
use crate::errors::AppError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpFamily {
    V4,
    V6,
}

#[derive(Clone)]
pub struct IpamService {
    pool: PgPool,
}

impl IpamService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn map_ip_addresses(&self, ids: &[Uuid]) -> Result<HashMap<Uuid, String>, AppError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query(
            r#"
            SELECT id, address::text AS address
            FROM ipam_ipaddress
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load IP addresses: {}", e)))?;

        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read IP id: {}", e)))?;
            let address: String = row
                .try_get("address")
                .map_err(|e| AppError::Database(format!("Failed to read IP address: {}", e)))?;
            map.insert(id, display_ip_address(&address));
        }

        Ok(map)
    }

    pub async fn set_primary_ip_for_device(
        &self,
        device_id: Uuid,
        tenant_id: Option<Uuid>,
        address: Option<&str>,
    ) -> Result<Option<Uuid>, AppError> {
        let Some((normalized, family)) = normalize_ip_address(address) else {
            self.clear_device_primary_ips(device_id).await?;
            return Ok(None);
        };

        let mut tx = self.pool.begin().await.map_err(|e| {
            AppError::Database(format!("Failed to start IP assignment transaction: {}", e))
        })?;

        let interface_id = ensure_device_interface(&mut tx, device_id).await?;
        let content_type_id = lookup_content_type(&mut tx, "dcim", "interface").await?;
        let ip_id = ensure_ip_address(
            &mut tx,
            &normalized,
            tenant_id,
            content_type_id,
            interface_id,
        )
        .await?;

        match family {
            IpFamily::V4 => {
                sqlx::query("UPDATE dcim_device SET primary_ip4_id = $2 WHERE id = $1")
                    .bind(device_id)
                    .bind(ip_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        AppError::Database(format!("Failed to update device IP: {}", e))
                    })?;
            }
            IpFamily::V6 => {
                sqlx::query("UPDATE dcim_device SET primary_ip6_id = $2 WHERE id = $1")
                    .bind(device_id)
                    .bind(ip_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        AppError::Database(format!("Failed to update device IP: {}", e))
                    })?;
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit IP assignment transaction: {}", e))
        })?;

        Ok(Some(ip_id))
    }

    pub async fn set_primary_ip_for_vm(
        &self,
        vm_id: Uuid,
        tenant_id: Option<Uuid>,
        address: Option<&str>,
    ) -> Result<Option<Uuid>, AppError> {
        let Some((normalized, family)) = normalize_ip_address(address) else {
            self.clear_vm_primary_ips(vm_id).await?;
            return Ok(None);
        };

        let mut tx = self.pool.begin().await.map_err(|e| {
            AppError::Database(format!("Failed to start IP assignment transaction: {}", e))
        })?;

        let interface_id = ensure_vm_interface(&mut tx, vm_id).await?;
        let content_type_id = lookup_content_type(&mut tx, "virtualization", "vminterface").await?;
        let ip_id = ensure_ip_address(
            &mut tx,
            &normalized,
            tenant_id,
            content_type_id,
            interface_id,
        )
        .await?;

        match family {
            IpFamily::V4 => {
                sqlx::query(
                    "UPDATE virtualization_virtualmachine SET primary_ip4_id = $2 WHERE id = $1",
                )
                .bind(vm_id)
                .bind(ip_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::Database(format!("Failed to update VM IP: {}", e)))?;
            }
            IpFamily::V6 => {
                sqlx::query(
                    "UPDATE virtualization_virtualmachine SET primary_ip6_id = $2 WHERE id = $1",
                )
                .bind(vm_id)
                .bind(ip_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::Database(format!("Failed to update VM IP: {}", e)))?;
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit IP assignment transaction: {}", e))
        })?;

        Ok(Some(ip_id))
    }

    async fn clear_device_primary_ips(&self, device_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE dcim_device SET primary_ip4_id = NULL, primary_ip6_id = NULL WHERE id = $1",
        )
        .bind(device_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to clear device IPs: {}", e)))?;
        Ok(())
    }

    async fn clear_vm_primary_ips(&self, vm_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE virtualization_virtualmachine SET primary_ip4_id = NULL, primary_ip6_id = NULL WHERE id = $1",
        )
        .bind(vm_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to clear VM IPs: {}", e)))?;
        Ok(())
    }
}

fn normalize_ip_address(address: Option<&str>) -> Option<(String, IpFamily)> {
    let address = address?.trim();
    if address.is_empty() {
        return None;
    }

    if let Some((ip_str, prefix)) = address.split_once('/') {
        let ip = IpAddr::from_str(ip_str).ok()?;
        let family = if ip.is_ipv4() {
            IpFamily::V4
        } else {
            IpFamily::V6
        };
        return Some((format!("{}/{}", ip, prefix), family));
    }

    let ip = IpAddr::from_str(address).ok()?;
    let family = if ip.is_ipv4() {
        IpFamily::V4
    } else {
        IpFamily::V6
    };
    let prefix = if ip.is_ipv4() { 32 } else { 128 };
    Some((format!("{}/{}", ip, prefix), family))
}

fn display_ip_address(address: &str) -> String {
    address.split('/').next().unwrap_or(address).to_string()
}

/// Resolve a content type by `(app_label, model)`. Pure SELECT against the
/// registry-seeded `inventory_content_type` table — zero-legacy, no write-time
/// INSERT. The only callers below use `dcim.interface` / `virtualization.vminterface`,
/// both seeded by the `seed_content_types` migration, so this always resolves.
async fn lookup_content_type(
    tx: &mut Transaction<'_, Postgres>,
    app_label: &str,
    model: &str,
) -> Result<Uuid, AppError> {
    lookup_content_type_id_tx(tx, app_label, model)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!(
                "Unknown content type {:?}: not a seeded inventory content type",
                content_type::token(app_label, model)
            ))
        })
}

async fn ensure_device_interface(
    tx: &mut Transaction<'_, Postgres>,
    device_id: Uuid,
) -> Result<Uuid, AppError> {
    if let Some(row) =
        sqlx::query("SELECT id FROM dcim_interface WHERE device_id = $1 ORDER BY name LIMIT 1")
            .bind(device_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load device interface: {}", e)))?
    {
        return row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to read interface id: {}", e)));
    }

    let id = Uuid::now_v7();
    let name = "mgmt0";
    sqlx::query(
        r#"
        INSERT INTO dcim_interface (
            id,
            custom_field_data,
            name,
            label,
            description,
            mark_connected,
            enabled,
            _name,
            type,
            mgmt_only,
            device_id
        )
        VALUES ($1, '{}'::jsonb, $2, $2, '', false, true, $2, 'virtual', true, $3)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(device_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to insert device interface: {}", e)))?;
    Ok(id)
}

async fn ensure_vm_interface(
    tx: &mut Transaction<'_, Postgres>,
    vm_id: Uuid,
) -> Result<Uuid, AppError> {
    if let Some(row) = sqlx::query(
        "SELECT id FROM virtualization_vminterface WHERE virtual_machine_id = $1 ORDER BY name LIMIT 1",
    )
    .bind(vm_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to load VM interface: {}", e)))?
    {
        return row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to read VM interface id: {}", e)));
    }

    let id = Uuid::now_v7();
    let name = "mgmt0";
    sqlx::query(
        r#"
        INSERT INTO virtualization_vminterface (
            id,
            custom_field_data,
            enabled,
            name,
            _name,
            description,
            virtual_machine_id
        )
        VALUES ($1, '{}'::jsonb, true, $2, $2, '', $3)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(vm_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to insert VM interface: {}", e)))?;
    Ok(id)
}

async fn ensure_ip_address(
    tx: &mut Transaction<'_, Postgres>,
    address: &str,
    tenant_id: Option<Uuid>,
    assigned_object_type_id: Uuid,
    assigned_object_id: Uuid,
) -> Result<Uuid, AppError> {
    if let Some(row) = sqlx::query(
        r#"
        SELECT id
        FROM ipam_ipaddress
        WHERE address = $1::inet
          AND assigned_object_type_id = $2
          AND assigned_object_id = $3
        LIMIT 1
        "#,
    )
    .bind(address)
    .bind(assigned_object_type_id)
    .bind(assigned_object_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to load IP address: {}", e)))?
    {
        return row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to read IP id: {}", e)));
    }

    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO ipam_ipaddress (
            id,
            custom_field_data,
            address,
            status,
            dns_name,
            description,
            assigned_object_id,
            assigned_object_type_id,
            tenant_id,
            comments
        )
        VALUES ($1, '{}'::jsonb, $2::inet, 'active', '', '', $3, $4, $5, '')
        "#,
    )
    .bind(id)
    .bind(address)
    .bind(assigned_object_id)
    .bind(assigned_object_type_id)
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("Failed to insert IP address: {}", e)))?;
    Ok(id)
}
