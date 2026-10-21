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
    fn column_name(self) -> &'static str {
        match self {
            BulkColumn::Status => "status",
            BulkColumn::TenantId => "tenant_id",
            BulkColumn::RoleId => "role_id",
            BulkColumn::SiteId => "site_id",
            BulkColumn::ClusterId => "cluster_id",
            BulkColumn::DeviceTypeId => "device_type_id",
        }
    }

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

    fn table_for_column(entity_type: &str, column: BulkColumn) -> Result<&'static str, AppError> {
        let table = match (entity_type, column) {
            ("sites", BulkColumn::Status) => "dcim_site",
            ("devices", BulkColumn::Status) => "dcim_device",
            ("virtual_machines", BulkColumn::Status) => "virtualization_virtualmachine",

            ("sites", BulkColumn::TenantId) => "dcim_site",
            ("devices", BulkColumn::TenantId) => "dcim_device",
            ("virtual_machines", BulkColumn::TenantId) => "virtualization_virtualmachine",

            ("devices", BulkColumn::RoleId) => "dcim_device",
            ("virtual_machines", BulkColumn::RoleId) => "virtualization_virtualmachine",

            ("devices", BulkColumn::SiteId) => "dcim_device",
            ("virtual_machines", BulkColumn::SiteId) => "virtualization_virtualmachine",

            ("virtual_machines", BulkColumn::ClusterId) => "virtualization_virtualmachine",
            ("devices", BulkColumn::DeviceTypeId) => "dcim_device",

            (other, col) => {
                return Err(AppError::Validation(format!(
                    "{} is not supported for entity_type: {}",
                    col.op_label(),
                    other
                )));
            }
        };
        Ok(table)
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
            let table = Self::table_for_column(entity_type, column)?;
            let col = column.column_name();
            let op = column.op_label();
            let statement = format!("UPDATE {table} SET {col} = $2 WHERE id = $1");
            let result = sqlx::query(statement.as_str())
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
            let table = Self::table_for_column(entity_type, column)?;
            let col = column.column_name();
            let op = column.op_label();
            let statement = format!("UPDATE {table} SET {col} = $2 WHERE id = $1");
            let result = sqlx::query(statement.as_str())
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
