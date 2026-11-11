//! Bulk resource repository — canonical SQL surface for cross-table bulk
//! operations on inventory resources (delete / set status / assign FK columns).
//!
//! The repo owns the table-and-column whitelist because routing from
//! `entity_type` to a physical table is a SQL concern; validation of unknown
//! entity_types is surfaced via `AppError::Validation`.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkColumn {
    Status,
    TenantId,
    RoleId,
    SiteId,
    ClusterId,
    DeviceTypeId,
}

impl BulkColumn {
    fn op_label(self) -> &'static str {
        match self {
            BulkColumn::Status => "set_status",
            BulkColumn::TenantId => "assign_tenant",
            BulkColumn::RoleId => "set_role",
            BulkColumn::SiteId => "set_site",
            BulkColumn::ClusterId => "set_cluster",
            BulkColumn::DeviceTypeId => "set_device_type",
        }
    }
}

pub trait BulkResourceRepository: Send + Sync {
    fn delete_by_id<'a>(
        &'a self,
        entity_type: &'a str,
        id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

    fn update_text_column<'a>(
        &'a self,
        entity_type: &'a str,
        column: BulkColumn,
        id: Uuid,
        value: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

    fn update_uuid_column<'a>(
        &'a self,
        entity_type: &'a str,
        column: BulkColumn,
        id: Uuid,
        value: Option<Uuid>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;
}

#[derive(Clone)]
pub struct SqlxBulkResourceRepository {
    pool: PgPool,
}

impl SqlxBulkResourceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn table_for_delete(entity_type: &str) -> Result<&'static str, AppError> {
        match entity_type {
            "owners" => Ok("users_owner"),
            "owner_groups" => Ok("users_ownergroup"),
            "tenant_groups" => Ok("tenancy_tenantgroup"),
            "tenants" => Ok("tenancy_tenant"),
            "regions" => Ok("dcim_region"),
            "site_groups" => Ok("dcim_sitegroup"),
            "wireless_lan_groups" => Ok("wireless_wirelesslangroup"),
            "wireless_lans" => Ok("wireless_wirelesslan"),
            "wireless_links" => Ok("wireless_wirelesslink"),
            "sites" => Ok("dcim_site"),
            "cluster_types" => Ok("virtualization_clustertype"),
            "cluster_groups" => Ok("virtualization_clustergroup"),
            "clusters" => Ok("virtualization_cluster"),
            "manufacturers" => Ok("dcim_manufacturer"),
            "device_types" => Ok("dcim_devicetype"),
            "device_roles" => Ok("dcim_devicerole"),
            "devices" => Ok("dcim_device"),
            "virtual_machines" => Ok("virtualization_virtualmachine"),
            other => Err(AppError::Validation(format!(
                "Delete is not supported for entity_type: {other}"
            ))),
        }
    }

    fn statement_for_column(
        entity_type: &str,
        column: BulkColumn,
    ) -> Result<&'static str, AppError> {
        let statement = match (entity_type, column) {
            ("sites", BulkColumn::Status) => "UPDATE dcim_site SET status = $2 WHERE id = $1",
            ("devices", BulkColumn::Status) => "UPDATE dcim_device SET status = $2 WHERE id = $1",
            ("virtual_machines", BulkColumn::Status) => {
                "UPDATE virtualization_virtualmachine SET status = $2 WHERE id = $1"
            }

            ("sites", BulkColumn::TenantId) => "UPDATE dcim_site SET tenant_id = $2 WHERE id = $1",
            ("devices", BulkColumn::TenantId) => {
                "UPDATE dcim_device SET tenant_id = $2 WHERE id = $1"
            }
            ("virtual_machines", BulkColumn::TenantId) => {
                "UPDATE virtualization_virtualmachine SET tenant_id = $2 WHERE id = $1"
            }

            ("devices", BulkColumn::RoleId) => "UPDATE dcim_device SET role_id = $2 WHERE id = $1",
            ("virtual_machines", BulkColumn::RoleId) => {
                "UPDATE virtualization_virtualmachine SET role_id = $2 WHERE id = $1"
            }

            ("devices", BulkColumn::SiteId) => "UPDATE dcim_device SET site_id = $2 WHERE id = $1",
            ("virtual_machines", BulkColumn::SiteId) => {
                "UPDATE virtualization_virtualmachine SET site_id = $2 WHERE id = $1"
            }

            ("virtual_machines", BulkColumn::ClusterId) => {
                "UPDATE virtualization_virtualmachine SET cluster_id = $2 WHERE id = $1"
            }
            ("devices", BulkColumn::DeviceTypeId) => {
                "UPDATE dcim_device SET device_type_id = $2 WHERE id = $1"
            }

            (other, col) => {
                return Err(AppError::Validation(format!(
                    "{} is not supported for entity_type: {}",
                    col.op_label(),
                    other
                )));
            }
        };
        Ok(statement)
    }
}

impl BulkResourceRepository for SqlxBulkResourceRepository {
    fn delete_by_id<'a>(
        &'a self,
        entity_type: &'a str,
        id: Uuid,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            let table = Self::table_for_delete(entity_type)?;
            let affected =
                super::delete_guard::delete_inventory_record(&self.pool, table, id).await?;
            if affected == 0 {
                return Err(AppError::NotFound("Resource not found".to_string()));
            }
            Ok(())
        })
    }

    fn update_text_column<'a>(
        &'a self,
        entity_type: &'a str,
        column: BulkColumn,
        id: Uuid,
        value: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            let statement = Self::statement_for_column(entity_type, column)?;
            let op = column.op_label();
            let result = sqlx::query(statement)
                .bind(id)
                .bind(value)
                .execute(&self.pool)
                .await
                .map_err(|error| AppError::Database(format!("Failed to execute {op}: {error}")))?;
            if result.rows_affected() == 0 {
                return Err(AppError::NotFound("Resource not found".to_string()));
            }
            Ok(())
        })
    }

    fn update_uuid_column<'a>(
        &'a self,
        entity_type: &'a str,
        column: BulkColumn,
        id: Uuid,
        value: Option<Uuid>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            let statement = Self::statement_for_column(entity_type, column)?;
            let op = column.op_label();
            let result = sqlx::query(statement)
                .bind(id)
                .bind(value)
                .execute(&self.pool)
                .await
                .map_err(|error| AppError::Database(format!("Failed to execute {op}: {error}")))?;
            if result.rows_affected() == 0 {
                return Err(AppError::NotFound("Resource not found".to_string()));
            }
            Ok(())
        })
    }
}
