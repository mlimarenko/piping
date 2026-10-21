use std::net::IpAddr;
use std::str::FromStr;

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

const DCIM_INTERFACE_APP_LABEL: &str = "dcim";
/// Canonical assignment validity is separate from primary-management ownership:
/// a primary IP with a dangling interface assignment is not a valid binding.
pub(crate) async fn address_assignment_valid(
    connection: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<bool, AppError> {
    sqlx::query_scalar("SELECT (ip.assigned_object_id IS NULL AND ip.assigned_object_type_id IS NULL) OR (ct.app_label = 'dcim' AND ct.model = 'interface' AND EXISTS (SELECT 1 FROM dcim_interface WHERE id = ip.assigned_object_id)) OR (ct.app_label = 'virtualization' AND ct.model = 'vminterface' AND EXISTS (SELECT 1 FROM virtualization_vminterface WHERE id = ip.assigned_object_id)) FROM ipam_ipaddress ip LEFT JOIN inventory_content_type ct ON ct.id = ip.assigned_object_type_id WHERE ip.id = $1")
        .bind(id).fetch_optional(connection).await.map_err(crate::errors::classify_sqlx_error_owned).map(|value: Option<Option<bool>>| value.flatten().unwrap_or(false))
}
/// Resolve typed inventory ownership, not equality of address strings.
pub(crate) async fn address_host_owners(
    connection: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<
    std::collections::BTreeSet<
        crate::contexts::inventory::domain::native::dataflow::DataflowEndpoint,
    >,
    AppError,
> {
    use crate::contexts::inventory::domain::native::dataflow::{DataflowEndpoint, EndpointType};
    let rows: Vec<(String, Uuid)> = sqlx::query_as("SELECT 'dcim.device' AS object_type, id AS object_id FROM dcim_device WHERE primary_ip4_id = $1 OR primary_ip6_id = $1 UNION SELECT 'virtualization.virtualmachine', id FROM virtualization_virtualmachine WHERE primary_ip4_id = $1 OR primary_ip6_id = $1 UNION SELECT 'dcim.device', i.device_id FROM ipam_ipaddress ip JOIN inventory_content_type ct ON ct.id = ip.assigned_object_type_id JOIN dcim_interface i ON i.id = ip.assigned_object_id WHERE ip.id = $1 AND ct.app_label = 'dcim' AND ct.model = 'interface' UNION SELECT 'virtualization.virtualmachine', i.virtual_machine_id FROM ipam_ipaddress ip JOIN inventory_content_type ct ON ct.id = ip.assigned_object_type_id JOIN virtualization_vminterface i ON i.id = ip.assigned_object_id WHERE ip.id = $1 AND ct.app_label = 'virtualization' AND ct.model = 'vminterface' ORDER BY object_type, object_id")
        .bind(id).fetch_all(connection).await.map_err(crate::errors::classify_sqlx_error_owned)?;
    rows.into_iter()
        .map(|(object_type, object_id)| {
            Ok(DataflowEndpoint {
                object_type: EndpointType::parse(&object_type)?,
                object_id,
            })
        })
        .collect()
}
const DCIM_INTERFACE_MODEL: &str = "interface";
const VIRTUALIZATION_VM_INTERFACE_APP_LABEL: &str = "virtualization";
const VIRTUALIZATION_VM_INTERFACE_MODEL: &str = "vminterface";

pub const IP_ADDRESS_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "address",
        "ip.address",
        "inventory.ipAddresses.columns.address",
    ),
    ListColumn::enum_filter(
        "status",
        "ip.status",
        "status",
        "inventory.ipAddresses.columns.status",
    ),
    ListColumn::enum_filter(
        "role",
        "ip.role",
        "role",
        "inventory.ipAddresses.columns.role",
    ),
    ListColumn::sortable(
        "dns_name",
        "ip.dns_name",
        "inventory.ipAddresses.columns.dnsName",
    ),
    ListColumn::sortable(
        "description",
        "ip.description",
        "inventory.ipAddresses.columns.description",
    ),
    ListColumn::fk_filter(
        "assigned_object_id",
        "ip.assigned_object_id",
        "assigned_object_id",
        "ip.assigned_object_id",
        "interfaces",
        "inventory.ipAddresses.columns.interface",
    ),
    ListColumn::fk_filter(
        "assigned_object_type_id",
        "ip.assigned_object_type_id",
        "assigned_object_type_id",
        "assigned_object_type_id",
        "contentTypes",
        "inventory.ipAddresses.columns.assignedObjectType",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "ip.tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.ipAddresses.columns.tenant",
    ),
    ListColumn::fk_filter(
        "vrf_id",
        "ip.vrf_id",
        "vrf_id",
        "vrf_id",
        "vrfs",
        "inventory.ipAddresses.columns.vrf",
    ),
];

/// Canonical view of an `ipam_ipaddress` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct IpAddressRecord {
    pub id: Uuid,
    /// CIDR form (`address::text`), e.g. `10.10.10.250/24`.
    pub address: String,
    pub status: String,
    pub role: Option<String>,
    pub dns_name: String,
    pub description: String,
    pub assigned_object_id: Option<Uuid>,
    pub assigned_object_type_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub vrf_id: Option<Uuid>,
    pub interface_name: Option<String>,
    pub tenant_name: Option<String>,
    pub vrf_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct IpAddressListFilter {
    pub search: Option<String>,
    pub parent_prefix: Option<String>,
    pub device_id: Option<Uuid>,
    pub virtual_machine_id: Option<Uuid>,
    pub status: Vec<String>,
    pub role: Vec<String>,
    pub assigned_object_id: Vec<Uuid>,
    pub assigned_object_type_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub vrf_id: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewIpAddress {
    pub id: Uuid,
    /// CIDR form after normalization (always contains `/`).
    pub address: String,
    pub status: String,
    pub interface_id: Uuid,
    pub dns_name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub vrf_id: Option<Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateIpAddress {
    pub address: Option<String>,
    pub status: Option<String>,
    pub interface_id: Option<Uuid>,
    pub dns_name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<Option<Uuid>>,
    pub vrf_id: Option<Option<Uuid>>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait IpAddressRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<IpAddressRecord>, AppError>;

    /// When a parent id is provided, results are joined through the concrete
    /// interface table for that parent (`dcim_interface` or
    /// `virtualization_vminterface`) and filtered by the parent FK.
    async fn list(
        &self,
        filter: IpAddressListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<IpAddressRecord>, i64), AppError>;

    async fn create(&self, new_ip: &NewIpAddress) -> Result<IpAddressRecord, AppError>;

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateIpAddress,
    ) -> Result<Option<IpAddressRecord>, AppError>;

    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxIpAddressRepository {
    pool: PgPool,
}

impl SqlxIpAddressRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn apply_nullable_uuid_update(
        change: Option<Option<Uuid>>,
        current: Option<Uuid>,
    ) -> Option<Uuid> {
        change.unwrap_or(current)
    }

    fn parse_row(row: &PgRow) -> Result<IpAddressRecord, AppError> {
        Ok(IpAddressRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read IP id: {}", e)))?,
            address: row
                .try_get("address")
                .map_err(|e| AppError::Database(format!("Failed to read IP address: {}", e)))?,
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read IP status: {}", e)))?,
            role: row
                .try_get::<Option<String>, _>("role")
                .map_err(|e| AppError::Database(format!("Failed to read ipaddress role: {}", e)))?,
            dns_name: row
                .try_get("dns_name")
                .map_err(|e| AppError::Database(format!("Failed to read IP dns_name: {}", e)))?,
            description: row
                .try_get("description")
                .map_err(|e| AppError::Database(format!("Failed to read IP description: {}", e)))?,
            assigned_object_id: row
                .try_get::<Option<Uuid>, _>("assigned_object_id")
                .map_err(|e| {
                    AppError::Database(format!(
                        "Failed to read ipaddress assigned_object_id: {}",
                        e
                    ))
                })?,
            assigned_object_type_id: row
                .try_get::<Option<Uuid>, _>("assigned_object_type_id")
                .map_err(|e| {
                    AppError::Database(format!(
                        "Failed to read ipaddress assigned_object_type_id: {}",
                        e
                    ))
                })?,
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read ipaddress tenant_id: {}", e))
            })?,
            vrf_id: row.try_get::<Option<Uuid>, _>("vrf_id").map_err(|e| {
                AppError::Database(format!("Failed to read ipaddress vrf_id: {}", e))
            })?,
            interface_name: row
                .try_get::<Option<String>, _>("interface_name")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read ipaddress interface_name: {}", e))
                })?,
            tenant_name: row
                .try_get::<Option<String>, _>("tenant_name")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read ipaddress tenant_name: {}", e))
                })?,
            vrf_name: row.try_get::<Option<String>, _>("vrf_name").map_err(|e| {
                AppError::Database(format!("Failed to read ipaddress vrf_name: {}", e))
            })?,
        })
    }

    async fn ensure_content_type_id(&self, app_label: &str, model: &str) -> Result<Uuid, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO inventory_content_type (id, app_label, model)
            VALUES ($1, $2, $3)
            ON CONFLICT (app_label, model) DO UPDATE SET app_label = EXCLUDED.app_label
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(app_label)
        .bind(model)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to ensure content type: {}", e)))?;

        row.try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to read content type id: {}", e)))
    }

    async fn resolve_interface_content_type_id(
        &self,
        interface_id: Uuid,
    ) -> Result<Uuid, AppError> {
        let row = sqlx::query(
            r#"
            SELECT
                EXISTS(SELECT 1 FROM dcim_interface WHERE id = $1) AS is_device_interface,
                EXISTS(SELECT 1 FROM virtualization_vminterface WHERE id = $1) AS is_vm_interface
            "#,
        )
        .bind(interface_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to resolve interface owner: {}", e)))?;

        let is_device_interface: bool = row.try_get("is_device_interface").map_err(|e| {
            AppError::Database(format!("Failed to read device-interface match flag: {}", e))
        })?;
        let is_vm_interface: bool = row.try_get("is_vm_interface").map_err(|e| {
            AppError::Database(format!("Failed to read VM-interface match flag: {}", e))
        })?;

        match (is_device_interface, is_vm_interface) {
            (true, false) => {
                self.ensure_content_type_id(DCIM_INTERFACE_APP_LABEL, DCIM_INTERFACE_MODEL)
                    .await
            }
            (false, true) => {
                self.ensure_content_type_id(
                    VIRTUALIZATION_VM_INTERFACE_APP_LABEL,
                    VIRTUALIZATION_VM_INTERFACE_MODEL,
                )
                .await
            }
            (false, false) => Err(AppError::Validation(
                "Referenced resource does not exist or cannot be used here".to_string(),
            )),
            (true, true) => Err(AppError::Database(
                "Interface identifier is ambiguous across inventory interface tables".to_string(),
            )),
        }
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &IpAddressListFilter) {
        let has_filter = filter.search.is_some()
            || filter.parent_prefix.is_some()
            || filter.device_id.is_some()
            || filter.virtual_machine_id.is_some()
            || !filter.status.is_empty()
            || !filter.role.is_empty()
            || !filter.assigned_object_id.is_empty()
            || !filter.assigned_object_type_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.vrf_id.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();

        if let Some(search) = filter.search.clone() {
            where_state.and(builder);
            builder.push("(");
            builder.push("ip.address::text ILIKE ");
            builder.push_bind(search.clone());
            builder.push(" OR COALESCE(ip.dns_name, '') ILIKE ");
            builder.push_bind(search.clone());
            builder.push(" OR COALESCE(ip.description, '') ILIKE ");
            builder.push_bind(search.clone());
            builder.push(" OR COALESCE(iface.name, viface.name, '') ILIKE ");
            builder.push_bind(search.clone());
            builder.push(" OR COALESCE(t.name, '') ILIKE ");
            builder.push_bind(search.clone());
            builder.push(" OR COALESCE(v.name, '') ILIKE ");
            builder.push_bind(search);
            builder.push(")");
        }
        if let Some(parent_prefix) = filter.parent_prefix.clone() {
            where_state.and(builder);
            builder.push("ip.address <<= ");
            builder.push_bind(parent_prefix);
            builder.push("::cidr");
        }
        if let Some(device_id) = filter.device_id {
            where_state.and(builder);
            builder.push("iface.device_id = ");
            builder.push_bind(device_id);
        }
        if let Some(virtual_machine_id) = filter.virtual_machine_id {
            where_state.and(builder);
            builder.push("viface.virtual_machine_id = ");
            builder.push_bind(virtual_machine_id);
        }
        push_string_in(builder, &mut where_state, "ip.status", &filter.status);
        push_string_in(builder, &mut where_state, "ip.role", &filter.role);
        push_uuid_in(
            builder,
            &mut where_state,
            "ip.assigned_object_id",
            &filter.assigned_object_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            "ip.assigned_object_type_id",
            &filter.assigned_object_type_id,
        );
        push_uuid_in(builder, &mut where_state, "ip.tenant_id", &filter.tenant_id);
        push_uuid_in(builder, &mut where_state, "ip.vrf_id", &filter.vrf_id);
    }

    async fn count_list(&self, filter: &IpAddressListFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT COUNT(*)
            FROM ipam_ipaddress ip
            LEFT JOIN dcim_interface iface ON iface.id = ip.assigned_object_id
            LEFT JOIN virtualization_vminterface viface ON viface.id = ip.assigned_object_id
            LEFT JOIN tenancy_tenant t ON t.id = ip.tenant_id
            LEFT JOIN ipam_vrf v ON v.id = ip.vrf_id
            "#,
        );
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count IP addresses: {}", e)))
    }
}

impl IpAddressRepository for SqlxIpAddressRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<IpAddressRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT ip.id,
                   ip.address::text AS address,
                   ip.status,
                   ip.role,
                   ip.dns_name,
                   ip.description,
                   ip.assigned_object_id,
                   ip.assigned_object_type_id,
                   ip.tenant_id,
                   ip.vrf_id,
                   COALESCE(iface.name, viface.name) AS interface_name,
                   t.name AS tenant_name,
                   v.name AS vrf_name
            FROM ipam_ipaddress ip
            LEFT JOIN dcim_interface iface ON iface.id = ip.assigned_object_id
            LEFT JOIN virtualization_vminterface viface ON viface.id = ip.assigned_object_id
            LEFT JOIN tenancy_tenant t ON t.id = ip.tenant_id
            LEFT JOIN ipam_vrf v ON v.id = ip.vrf_id
            WHERE ip.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load IP address: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: IpAddressListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<IpAddressRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT ip.id,
                   ip.address::text AS address,
                   ip.status,
                   ip.role,
                   ip.dns_name,
                   ip.description,
                   ip.assigned_object_id,
                   ip.assigned_object_type_id,
                   ip.tenant_id,
                   ip.vrf_id,
                   COALESCE(iface.name, viface.name) AS interface_name,
                   t.name AS tenant_name,
                   v.name AS vrf_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_ipaddress ip
            LEFT JOIN dcim_interface iface ON iface.id = ip.assigned_object_id
            LEFT JOIN virtualization_vminterface viface ON viface.id = ip.assigned_object_id
            LEFT JOIN tenancy_tenant t ON t.id = ip.tenant_id
            LEFT JOIN ipam_vrf v ON v.id = ip.vrf_id
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            IP_ADDRESS_COLUMNS,
            sort.as_ref(),
            "ip.address",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list IP addresses: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| {
                AppError::Database(format!("Failed to read IP address total_count: {}", e))
            })?
            .unwrap_or(0);
        let total = if rows.is_empty() {
            self.count_list(&filter).await?
        } else {
            total
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_ip: &NewIpAddress) -> Result<IpAddressRecord, AppError> {
        let content_type_id = self
            .resolve_interface_content_type_id(new_ip.interface_id)
            .await?;
        let dns_name = new_ip.dns_name.clone().unwrap_or_default();
        let description = new_ip.description.clone().unwrap_or_default();

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
                vrf_id,
                comments
            )
            VALUES ($1, '{}'::jsonb, $2::inet, $3, $4, $5, $6, $7, $8, $9, '')
            "#,
        )
        .bind(new_ip.id)
        .bind(&new_ip.address)
        .bind(&new_ip.status)
        .bind(&dns_name)
        .bind(&description)
        .bind(new_ip.interface_id)
        .bind(content_type_id)
        .bind(new_ip.tenant_id)
        .bind(new_ip.vrf_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create IP address: {}", e)))?;

        self.get_by_id(new_ip.id).await?.ok_or_else(|| {
            AppError::Database("Failed to reload IP address after create".to_string())
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateIpAddress,
    ) -> Result<Option<IpAddressRecord>, AppError> {
        let Some(current) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let address = changes.address.clone().unwrap_or(current.address.clone());
        let status = changes.status.clone().unwrap_or(current.status.clone());
        let dns_name = changes.dns_name.clone().unwrap_or(current.dns_name.clone());
        let description = changes
            .description
            .clone()
            .unwrap_or(current.description.clone());
        let tenant_id = Self::apply_nullable_uuid_update(changes.tenant_id, current.tenant_id);
        let vrf_id = Self::apply_nullable_uuid_update(changes.vrf_id, current.vrf_id);

        let (assigned_object_id, assigned_object_type_id) =
            if let Some(iface_id) = changes.interface_id {
                let ct = self.resolve_interface_content_type_id(iface_id).await?;
                (Some(iface_id), Some(ct))
            } else {
                (current.assigned_object_id, current.assigned_object_type_id)
            };

        sqlx::query(
            r#"
            UPDATE ipam_ipaddress
            SET address = $2::inet,
                status = $3,
                dns_name = $4,
                description = $5,
                assigned_object_id = $6,
                assigned_object_type_id = $7,
                tenant_id = $8,
                vrf_id = $9
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&address)
        .bind(&status)
        .bind(&dns_name)
        .bind(&description)
        .bind(assigned_object_id)
        .bind(assigned_object_type_id)
        .bind(tenant_id)
        .bind(vrf_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update IP address: {}", e)))?;

        self.get_by_id(id).await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_ipaddress", id)
            .await
            .map(|_| ())
    }
}

/// Ensure the input contains a CIDR prefix. Bare addresses get `/32` for
/// IPv4 and `/128` for IPv6 (canonical host form used throughout NetBox).
pub fn normalize_cidr(input: &str) -> Result<String, AppError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation("address is required".to_string()));
    }

    if let Some((ip_str, prefix)) = trimmed.split_once('/') {
        let ip = IpAddr::from_str(ip_str.trim())
            .map_err(|e| AppError::Validation(format!("Invalid IP address: {}", e)))?;
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return Err(AppError::Validation("CIDR prefix is required".to_string()));
        }
        let prefix_length = prefix
            .parse::<u8>()
            .map_err(|e| AppError::Validation(format!("Invalid CIDR prefix: {}", e)))?;
        let max_prefix_length = if ip.is_ipv4() { 32 } else { 128 };
        if prefix_length > max_prefix_length {
            return Err(AppError::Validation(format!(
                "Invalid CIDR prefix: expected 0..={} for IPv{}",
                max_prefix_length,
                if ip.is_ipv4() { 4 } else { 6 }
            )));
        }
        return Ok(format!("{}/{}", ip, prefix_length));
    }

    let ip = IpAddr::from_str(trimmed)
        .map_err(|e| AppError::Validation(format!("Invalid IP address: {}", e)))?;
    let prefix = if ip.is_ipv4() { 32 } else { 128 };
    Ok(format!("{}/{}", ip, prefix))
}

#[cfg(test)]
mod tests {
    use super::{SqlxIpAddressRepository, normalize_cidr};
    use uuid::Uuid;

    #[test]
    fn nullable_uuid_update_preserves_current_when_field_is_omitted() {
        let current = Some(Uuid::now_v7());
        assert_eq!(
            SqlxIpAddressRepository::apply_nullable_uuid_update(None, current),
            current
        );
    }

    #[test]
    fn nullable_uuid_update_clears_current_when_null_is_explicit() {
        let current = Some(Uuid::now_v7());
        assert_eq!(
            SqlxIpAddressRepository::apply_nullable_uuid_update(Some(None), current),
            None
        );
    }

    #[test]
    fn nullable_uuid_update_overrides_current_when_new_value_is_present() {
        let current = Some(Uuid::now_v7());
        let replacement = Uuid::now_v7();
        assert_eq!(
            SqlxIpAddressRepository::apply_nullable_uuid_update(Some(Some(replacement)), current),
            Some(replacement)
        );
    }

    #[test]
    fn normalize_cidr_rejects_prefix_lengths_outside_ip_family() {
        assert!(normalize_cidr("10.0.0.0/33").is_err());
        assert!(normalize_cidr("2001:db8::/129").is_err());
    }

    #[test]
    fn normalize_cidr_keeps_valid_network_prefixes() {
        assert_eq!(normalize_cidr("10.0.0.0/24").unwrap(), "10.0.0.0/24");
        assert_eq!(normalize_cidr("2001:db8::/64").unwrap(), "2001:db8::/64");
    }
}
