use std::collections::HashSet;

use serde_json::{Map, json};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::{ConfigContext, ConfigContextBinding};
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, list_sort_column, push_order_by, push_string_in,
};
use crate::contexts::shared::pagination::{PageResult, clamp_limit, clamp_offset};
use crate::errors::AppError;

/// Config-context list columns. Sort SQL values are compile-time literals and
/// the ONLY thing interpolated into `ORDER BY`; user input never reaches SQL.
pub const CONFIG_CONTEXT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "configContexts.columns.name"),
    ListColumn::sortable("weight", "weight", "configContexts.columns.weight"),
    ListColumn::enum_filter_with_param(
        "status",
        "is_active",
        "is_active",
        "CASE WHEN is_active THEN 'true' ELSE 'false' END",
        "configContexts.columns.isActive",
    ),
];

pub const CONFIG_CONTEXT_BINDING_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "context_id",
        "context_id",
        "configContexts.bindings.columns.context",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "scope_type",
        "scope_type",
        "configContexts.bindings.columns.scopeType",
    ),
    ListColumn::sortable(
        "scope_id",
        "scope_id",
        "configContexts.bindings.columns.scopeId",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct ConfigContextBindingListFilters {
    pub scope_type: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ConfigContextRepository: Send + Sync {
    async fn get_by_id(&self, context_id: Uuid) -> Result<Option<ConfigContext>, AppError>;
    async fn list_contexts_page(
        &self,
        name: Option<&str>,
        is_active: &[String],
        sort: Option<&SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<ConfigContext>, AppError>;
    /// Full (unpaginated) list for internal resolvers that need every
    /// matching context. Prefer `list_contexts_page` for HTTP list endpoints.
    async fn list_all_contexts(
        &self,
        name: Option<&str>,
        is_active: &[String],
    ) -> Result<Vec<ConfigContext>, AppError>;
    async fn create_context(&self, context: ConfigContext) -> Result<ConfigContext, AppError>;
    async fn update_context(&self, context: ConfigContext) -> Result<ConfigContext, AppError>;
    async fn delete_context(&self, context_id: Uuid) -> Result<bool, AppError>;
    async fn list_for_scope(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<ConfigContext>, AppError>;
    async fn list_for_scope_hierarchy(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<ConfigContext>, AppError>;
    async fn list_bindings(&self, context_id: Uuid) -> Result<Vec<ConfigContextBinding>, AppError>;
    async fn list_bindings_page(
        &self,
        context_id: Uuid,
        filters: &ConfigContextBindingListFilters,
        sort: Option<&SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<ConfigContextBinding>, AppError>;
    async fn create_binding(
        &self,
        binding: ConfigContextBinding,
    ) -> Result<ConfigContextBinding, AppError>;
    async fn delete_binding(&self, binding_id: Uuid) -> Result<bool, AppError>;

    /// Builds the authoritative per-launch `{{target.*}}` namespace for a
    /// connection target from the inventory entity and its related entities.
    /// Returns `None` when the id matches neither a device nor a VM.
    async fn load_target_variable_namespace(
        &self,
        target_id: Uuid,
    ) -> Result<Option<serde_json::Value>, AppError>;
}

#[derive(Debug, Clone)]
pub struct SqlxConfigContextRepository {
    pool: PgPool,
}

#[derive(Clone, Copy)]
enum TargetIpOwner {
    Device,
    VirtualMachine,
}

fn push_scope(scopes: &mut HashSet<(String, Uuid)>, scope_type: &str, scope_id: Option<Uuid>) {
    if let Some(scope_id) = scope_id {
        scopes.insert((scope_type.to_string(), scope_id));
    }
}

#[derive(Default)]
struct HierarchyScopes {
    scopes: HashSet<(String, Uuid)>,
    tenant_ids: HashSet<Uuid>,
    runtime_target_context: Option<ConfigContext>,
}

impl HierarchyScopes {
    fn push(&mut self, scope_type: &str, id: Option<Uuid>) {
        push_scope(&mut self.scopes, scope_type, id);
    }

    fn push_tenant(&mut self, id: Option<Uuid>) {
        self.push("tenant", id);
        if let Some(id) = id {
            self.tenant_ids.insert(id);
        }
    }
}

impl SqlxConfigContextRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_context_row(row: &PgRow) -> Result<ConfigContext, AppError> {
        Ok(ConfigContext {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read context id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read context name: {}", e)))?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read context description: {}", e))
            })?,
            weight: row
                .try_get("weight")
                .map_err(|e| AppError::Database(format!("Failed to read context weight: {}", e)))?,
            data: row
                .try_get("data")
                .map_err(|e| AppError::Database(format!("Failed to read context data: {}", e)))?,
            is_active: row.try_get("is_active").map_err(|e| {
                AppError::Database(format!("Failed to read context is_active: {}", e))
            })?,
            created_at: row.try_get("created_at").map_err(|e| {
                AppError::Database(format!("Failed to read context created_at: {}", e))
            })?,
            updated_at: row.try_get("updated_at").map_err(|e| {
                AppError::Database(format!("Failed to read context updated_at: {}", e))
            })?,
        })
    }

    fn parse_binding_row(row: &PgRow) -> Result<ConfigContextBinding, AppError> {
        Ok(ConfigContextBinding {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read binding id: {}", e)))?,
            context_id: row.try_get("context_id").map_err(|e| {
                AppError::Database(format!("Failed to read binding context_id: {}", e))
            })?,
            scope_type: row.try_get("scope_type").map_err(|e| {
                AppError::Database(format!("Failed to read binding scope_type: {}", e))
            })?,
            scope_id: row.try_get("scope_id").map_err(|e| {
                AppError::Database(format!("Failed to read binding scope_id: {}", e))
            })?,
        })
    }

    async fn fetch_tenant_group_id(&self, tenant_id: Uuid) -> Result<Option<Uuid>, AppError> {
        let row = sqlx::query("SELECT group_id FROM tenancy_tenant WHERE id = $1")
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to load tenant group for tenant {}: {}",
                    tenant_id, e
                ))
            })?;

        match row {
            Some(row) => row
                .try_get("group_id")
                .map_err(|e| AppError::Database(format!("Failed to read tenant group_id: {}", e))),
            None => Ok(None),
        }
    }

    async fn load_ip_address_display(
        &self,
        ip_id: Option<Uuid>,
    ) -> Result<Option<String>, AppError> {
        let Some(ip_id) = ip_id else {
            return Ok(None);
        };

        let row = sqlx::query("SELECT address::text AS address FROM ipam_ipaddress WHERE id = $1")
            .bind(ip_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load IP address: {}", e)))?;

        let Some(row) = row else {
            return Ok(None);
        };
        let raw: String = row
            .try_get("address")
            .map_err(|e| AppError::Database(format!("Failed to read IP address: {}", e)))?;
        Ok(Some(
            raw.split('/').next().unwrap_or(raw.as_str()).to_string(),
        ))
    }

    async fn load_fallback_interface_ip_display(
        &self,
        owner: TargetIpOwner,
        target_id: Uuid,
        family: i32,
    ) -> Result<Option<String>, AppError> {
        let (sql, owner_label) = match owner {
            TargetIpOwner::Device => (
                r#"
                SELECT host(ip.address) AS address
                FROM dcim_interface iface
                JOIN ipam_ipaddress ip
                  ON ip.assigned_object_id = iface.id
                JOIN inventory_content_type ct
                  ON ct.id = ip.assigned_object_type_id
                 AND ct.app_label = 'dcim'
                 AND ct.model = 'interface'
                WHERE iface.device_id = $1
                  AND family(ip.address) = $2
                ORDER BY COALESCE(iface.name, ''), iface.id, host(ip.address), masklen(ip.address), ip.id
                LIMIT 1
                "#,
                "device",
            ),
            TargetIpOwner::VirtualMachine => (
                r#"
                SELECT host(ip.address) AS address
                FROM virtualization_vminterface iface
                JOIN ipam_ipaddress ip
                  ON ip.assigned_object_id = iface.id
                JOIN inventory_content_type ct
                  ON ct.id = ip.assigned_object_type_id
                 AND ct.app_label = 'virtualization'
                 AND ct.model = 'vminterface'
                WHERE iface.virtual_machine_id = $1
                  AND family(ip.address) = $2
                ORDER BY COALESCE(iface.name, ''), iface.id, host(ip.address), masklen(ip.address), ip.id
                LIMIT 1
                "#,
                "virtual machine",
            ),
        };

        let row = sqlx::query(sql)
            .bind(target_id)
            .bind(family)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to load fallback {owner_label} interface IP: {e}"
                ))
            })?;

        row.map(|row| {
            row.try_get("address").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read fallback {owner_label} interface IP: {e}"
                ))
            })
        })
        .transpose()
    }

    async fn resolve_target_endpoint_ips(
        &self,
        owner: TargetIpOwner,
        target_id: Uuid,
        primary_ip4_id: Option<Uuid>,
        primary_ip6_id: Option<Uuid>,
    ) -> Result<(Option<String>, Option<String>), AppError> {
        let primary_ip4 = match self.load_ip_address_display(primary_ip4_id).await? {
            Some(address) => Some(address),
            None => {
                self.load_fallback_interface_ip_display(owner, target_id, 4)
                    .await?
            }
        };
        let primary_ip6 = match self.load_ip_address_display(primary_ip6_id).await? {
            Some(address) => Some(address),
            None => {
                self.load_fallback_interface_ip_display(owner, target_id, 6)
                    .await?
            }
        };
        Ok((primary_ip4, primary_ip6))
    }

    async fn build_target_runtime_context(
        &self,
        owner: TargetIpOwner,
        target_id: Uuid,
        primary_ip4_id: Option<Uuid>,
        primary_ip6_id: Option<Uuid>,
    ) -> Result<ConfigContext, AppError> {
        let (primary_ip4, primary_ip6) = self
            .resolve_target_endpoint_ips(owner, target_id, primary_ip4_id, primary_ip6_id)
            .await?;

        let data = json!({
            "primary_ip4": primary_ip4,
            "primary_ip6": primary_ip6,
        });

        Ok(ConfigContext {
            id: target_id,
            name: format!("target-runtime-context-{target_id}"),
            description: None,
            weight: i32::MIN,
            data,
            is_active: true,
            created_at: None,
            updated_at: None,
        })
    }

    fn build_target_variable_namespace(
        row: &PgRow,
        primary_ip4: Option<String>,
        primary_ip6: Option<String>,
        has_device_type: bool,
    ) -> Result<serde_json::Value, AppError> {
        let mut target = Map::new();
        insert_string(&mut target, "name", read_opt_string(row, "name")?);
        insert_string(&mut target, "primary_ip4", primary_ip4);
        insert_string(&mut target, "primary_ip6", primary_ip6);

        insert_relation(
            &mut target,
            "site",
            &[
                ("name", read_opt_string(row, "site_name")?),
                ("slug", read_opt_string(row, "site_slug")?),
            ],
        );
        insert_relation(
            &mut target,
            "region",
            &[
                ("name", read_opt_string(row, "region_name")?),
                ("slug", read_opt_string(row, "region_slug")?),
            ],
        );
        insert_relation(
            &mut target,
            "tenant",
            &[
                ("name", read_opt_string(row, "tenant_name")?),
                ("slug", read_opt_string(row, "tenant_slug")?),
            ],
        );
        insert_relation(
            &mut target,
            "role",
            &[
                ("name", read_opt_string(row, "role_name")?),
                ("slug", read_opt_string(row, "role_slug")?),
            ],
        );
        insert_relation(
            &mut target,
            "platform",
            &[
                ("name", read_opt_string(row, "platform_name")?),
                ("slug", read_opt_string(row, "platform_slug")?),
            ],
        );
        if has_device_type {
            insert_relation(
                &mut target,
                "device_type",
                &[
                    ("model", read_opt_string(row, "device_type_model")?),
                    ("slug", read_opt_string(row, "device_type_slug")?),
                ],
            );
        }
        insert_relation(
            &mut target,
            "cluster",
            &[("name", read_opt_string(row, "cluster_name")?)],
        );

        let custom_field_data: serde_json::Value = row
            .try_get("custom_field_data")
            .map_err(|e| AppError::Database(format!("Failed to read custom_field_data: {e}")))?;
        target.insert(
            "custom_fields".to_string(),
            serde_json::Value::Object(filter_custom_fields(&custom_field_data)),
        );

        Ok(serde_json::Value::Object(target))
    }

    async fn load_site_tenant(
        &self,
        site_id: Uuid,
        hierarchy: &mut HierarchyScopes,
    ) -> Result<bool, AppError> {
        let row = sqlx::query("SELECT tenant_id FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load site scope: {}", e)))?;
        let Some(row) = row else {
            return Ok(false);
        };
        let tenant_id: Option<Uuid> = row
            .try_get("tenant_id")
            .map_err(|e| AppError::Database(format!("Failed to read site tenant_id: {}", e)))?;
        hierarchy.push_tenant(tenant_id);
        Ok(true)
    }

    async fn load_cluster_hierarchy(
        &self,
        cluster_id: Uuid,
        hierarchy: &mut HierarchyScopes,
    ) -> Result<bool, AppError> {
        let row =
            sqlx::query("SELECT _site_id, tenant_id FROM virtualization_cluster WHERE id = $1")
                .bind(cluster_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to load cluster scope: {}", e)))?;
        let Some(row) = row else {
            return Ok(false);
        };
        let site_id: Option<Uuid> = row
            .try_get("_site_id")
            .map_err(|e| AppError::Database(format!("Failed to read cluster site_id: {}", e)))?;
        let tenant_id: Option<Uuid> = row
            .try_get("tenant_id")
            .map_err(|e| AppError::Database(format!("Failed to read cluster tenant_id: {}", e)))?;
        hierarchy.push("site", site_id);
        hierarchy.push_tenant(tenant_id);
        if let Some(site_id) = site_id {
            self.load_site_tenant(site_id, hierarchy).await?;
        }
        Ok(true)
    }

    async fn load_device_hierarchy(
        &self,
        device_id: Uuid,
        include_runtime: bool,
        hierarchy: &mut HierarchyScopes,
    ) -> Result<bool, AppError> {
        let row = sqlx::query(
            "SELECT site_id, tenant_id, role_id, primary_ip4_id, primary_ip6_id FROM dcim_device WHERE id = $1",
        )
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load device scope: {}", e)))?;
        let Some(row) = row else {
            return Ok(false);
        };
        hierarchy.push("device", Some(device_id));
        let site_id: Option<Uuid> = row
            .try_get("site_id")
            .map_err(|e| AppError::Database(format!("Failed to read device site_id: {}", e)))?;
        let tenant_id: Option<Uuid> = row
            .try_get("tenant_id")
            .map_err(|e| AppError::Database(format!("Failed to read device tenant_id: {}", e)))?;
        let role_id: Option<Uuid> = row
            .try_get("role_id")
            .map_err(|e| AppError::Database(format!("Failed to read device role_id: {}", e)))?;
        if include_runtime {
            hierarchy.runtime_target_context = Some(
                self.build_target_runtime_context(
                    TargetIpOwner::Device,
                    device_id,
                    row.try_get("primary_ip4_id").ok(),
                    row.try_get("primary_ip6_id").ok(),
                )
                .await?,
            );
        }
        hierarchy.push("site", site_id);
        hierarchy.push_tenant(tenant_id);
        hierarchy.push("role", role_id);
        if let Some(site_id) = site_id {
            self.load_site_tenant(site_id, hierarchy).await?;
        }
        Ok(true)
    }

    async fn load_vm_hierarchy(
        &self,
        vm_id: Uuid,
        include_runtime: bool,
        hierarchy: &mut HierarchyScopes,
    ) -> Result<bool, AppError> {
        let row = sqlx::query(
            "SELECT cluster_id, tenant_id, role_id, site_id, primary_ip4_id, primary_ip6_id FROM virtualization_virtualmachine WHERE id = $1",
        )
        .bind(vm_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vm scope: {}", e)))?;
        let Some(row) = row else {
            return Ok(false);
        };
        hierarchy.push("vm", Some(vm_id));
        let cluster_id: Option<Uuid> = row
            .try_get("cluster_id")
            .map_err(|e| AppError::Database(format!("Failed to read vm cluster_id: {}", e)))?;
        let tenant_id: Option<Uuid> = row
            .try_get("tenant_id")
            .map_err(|e| AppError::Database(format!("Failed to read vm tenant_id: {}", e)))?;
        let role_id: Option<Uuid> = row
            .try_get("role_id")
            .map_err(|e| AppError::Database(format!("Failed to read vm role_id: {}", e)))?;
        let site_id: Option<Uuid> = row
            .try_get("site_id")
            .map_err(|e| AppError::Database(format!("Failed to read vm site_id: {}", e)))?;
        if include_runtime {
            hierarchy.runtime_target_context = Some(
                self.build_target_runtime_context(
                    TargetIpOwner::VirtualMachine,
                    vm_id,
                    row.try_get("primary_ip4_id").ok(),
                    row.try_get("primary_ip6_id").ok(),
                )
                .await?,
            );
        }
        hierarchy.push("cluster", cluster_id);
        hierarchy.push_tenant(tenant_id);
        hierarchy.push("role", role_id);
        hierarchy.push("site", site_id);
        if let Some(cluster_id) = cluster_id {
            self.load_cluster_hierarchy(cluster_id, hierarchy).await?;
        }
        if let Some(site_id) = site_id {
            self.load_site_tenant(site_id, hierarchy).await?;
        }
        Ok(true)
    }
}

fn read_opt_string(row: &PgRow, column: &str) -> Result<Option<String>, AppError> {
    let value: Option<String> = row
        .try_get(column)
        .map_err(|e| AppError::Database(format!("Failed to read {column}: {e}")))?;
    Ok(value.filter(|raw| !raw.trim().is_empty()))
}

fn insert_string(map: &mut Map<String, serde_json::Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        map.insert(key.to_string(), serde_json::Value::String(value));
    }
}

fn insert_relation(
    map: &mut Map<String, serde_json::Value>,
    key: &str,
    fields: &[(&str, Option<String>)],
) {
    if fields.iter().all(|(_, value)| value.is_none()) {
        return;
    }
    let mut object = Map::new();
    for (field, value) in fields {
        if let Some(value) = value {
            object.insert(
                (*field).to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
    }
    map.insert(key.to_string(), serde_json::Value::Object(object));
}

fn filter_custom_fields(value: &serde_json::Value) -> Map<String, serde_json::Value> {
    let mut out = Map::new();
    let Some(object) = value.as_object() else {
        return out;
    };
    for (key, field_value) in object {
        if !is_safe_custom_field_key(key) {
            continue;
        }
        let scalar = match field_value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        let capped: String = scalar.chars().take(256).collect();
        out.insert(key.clone(), serde_json::Value::String(capped));
    }
    out
}

/// Delegates to the canonical custom-field key validator in
/// `custom_fields::domain::validation` (single source of truth).
fn is_safe_custom_field_key(key: &str) -> bool {
    crate::contexts::custom_fields::domain::is_valid_custom_field_key(key)
}

impl ConfigContextRepository for SqlxConfigContextRepository {
    async fn get_by_id(&self, context_id: Uuid) -> Result<Option<ConfigContext>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, description, weight, data, is_active, created_at, updated_at
            FROM config_context
            WHERE id = $1
            "#,
        )
        .bind(context_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load config context: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_context_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_contexts_page(
        &self,
        name: Option<&str>,
        is_active: &[String],
        sort: Option<&SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<ConfigContext>, AppError> {
        let mut items_builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, name, description, weight, data, is_active, created_at, updated_at
            FROM config_context
            WHERE 1 = 1
            "#,
        );

        if let Some(name) = name {
            items_builder.push(" AND name ILIKE ");
            items_builder.push_bind(format!("%{name}%"));
        }
        push_string_in(
            &mut items_builder,
            &mut WhereState::after_predicate(),
            "CASE WHEN is_active THEN 'true' ELSE 'false' END",
            is_active,
        );

        items_builder.push(" ORDER BY ");
        match sort.and_then(|spec| {
            list_sort_column(CONFIG_CONTEXT_COLUMNS, &spec.key).map(|column| (column, spec))
        }) {
            Some((column, spec)) => {
                items_builder.push(column);
                items_builder.push(spec.sql_direction());
                items_builder.push(", weight, id");
            }
            None => {
                items_builder.push("weight, id");
            }
        }
        items_builder.push(" LIMIT ");
        items_builder.push_bind(i64::from(limit));
        items_builder.push(" OFFSET ");
        items_builder.push_bind(i64::from(offset));

        let rows = items_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list config contexts: {}", e)))?;

        let items: Vec<ConfigContext> = rows
            .iter()
            .map(Self::parse_context_row)
            .collect::<Result<_, _>>()?;

        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::bigint FROM config_context WHERE 1 = 1",
        );
        if let Some(name) = name {
            count_builder.push(" AND name ILIKE ");
            count_builder.push_bind(format!("%{name}%"));
        }
        push_string_in(
            &mut count_builder,
            &mut WhereState::after_predicate(),
            "CASE WHEN is_active THEN 'true' ELSE 'false' END",
            is_active,
        );

        let total: i64 = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count config contexts: {}", e)))?;
        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Config context total count overflow".to_string()))?;

        Ok(PageResult {
            items,
            total,
            limit,
            offset,
        })
    }

    async fn list_all_contexts(
        &self,
        name: Option<&str>,
        is_active: &[String],
    ) -> Result<Vec<ConfigContext>, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, name, description, weight, data, is_active, created_at, updated_at
            FROM config_context
            WHERE 1 = 1
            "#,
        );
        if let Some(name) = name {
            builder.push(" AND name ILIKE ");
            builder.push_bind(format!("%{name}%"));
        }
        push_string_in(
            &mut builder,
            &mut WhereState::after_predicate(),
            "CASE WHEN is_active THEN 'true' ELSE 'false' END",
            is_active,
        );
        builder.push(" ORDER BY weight, id");

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list config contexts: {}", e))
            })?;
        rows.iter()
            .map(Self::parse_context_row)
            .collect::<Result<Vec<ConfigContext>, AppError>>()
    }

    async fn create_context(&self, context: ConfigContext) -> Result<ConfigContext, AppError> {
        context.validate()?;
        let row = sqlx::query(
            r#"
            INSERT INTO config_context (id, name, weight, data, is_active, description)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, description, weight, data, is_active, created_at, updated_at
            "#,
        )
        .bind(context.id)
        .bind(&context.name)
        .bind(context.weight)
        .bind(&context.data)
        .bind(context.is_active)
        .bind(&context.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create config context: {}", e)))?;

        Self::parse_context_row(&row)
    }

    async fn update_context(&self, context: ConfigContext) -> Result<ConfigContext, AppError> {
        context.validate()?;
        let row = sqlx::query(
            r#"
            UPDATE config_context
            SET name = $2,
                weight = $3,
                data = $4,
                is_active = $5,
                description = $6,
                updated_at = now()
            WHERE id = $1
            RETURNING id, name, description, weight, data, is_active, created_at, updated_at
            "#,
        )
        .bind(context.id)
        .bind(&context.name)
        .bind(context.weight)
        .bind(&context.data)
        .bind(context.is_active)
        .bind(&context.description)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update config context: {}", e)))?;

        match row {
            Some(row) => Self::parse_context_row(&row),
            None => Err(AppError::NotFound(format!(
                "Config context {} not found",
                context.id
            ))),
        }
    }

    async fn delete_context(&self, context_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM config_context WHERE id = $1")
            .bind(context_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete config context: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    async fn list_for_scope(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<ConfigContext>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT c.id, c.name, c.description, c.weight, c.data, c.is_active, c.created_at, c.updated_at
            FROM config_context c
            JOIN config_context_binding b ON b.context_id = c.id
            WHERE b.scope_type = $1
              AND b.scope_id = $2
              AND c.is_active = true
            ORDER BY c.weight, c.id
            "#,
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list config contexts: {}", e)))?;

        rows.iter()
            .map(Self::parse_context_row)
            .collect::<Result<Vec<ConfigContext>, AppError>>()
    }

    async fn list_for_scope_hierarchy(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<ConfigContext>, AppError> {
        let mut hierarchy = HierarchyScopes::default();
        match scope_type {
            "connection_target" => {
                hierarchy.push(scope_type, Some(scope_id));
                if !self
                    .load_device_hierarchy(scope_id, true, &mut hierarchy)
                    .await?
                    && !self
                        .load_vm_hierarchy(scope_id, true, &mut hierarchy)
                        .await?
                {
                    return Ok(Vec::new());
                }
            }
            "device" => {
                if !self
                    .load_device_hierarchy(scope_id, false, &mut hierarchy)
                    .await?
                {
                    return Ok(Vec::new());
                }
            }
            "vm" => {
                if !self
                    .load_vm_hierarchy(scope_id, false, &mut hierarchy)
                    .await?
                {
                    return Ok(Vec::new());
                }
            }
            "cluster" => {
                hierarchy.push(scope_type, Some(scope_id));
                if !self
                    .load_cluster_hierarchy(scope_id, &mut hierarchy)
                    .await?
                {
                    return Ok(Vec::new());
                }
            }
            "site" => {
                hierarchy.push(scope_type, Some(scope_id));
                if !self.load_site_tenant(scope_id, &mut hierarchy).await? {
                    return Ok(Vec::new());
                }
            }
            "tenant" => hierarchy.push_tenant(Some(scope_id)),
            _ => hierarchy.push(scope_type, Some(scope_id)),
        }

        for tenant_id in std::mem::take(&mut hierarchy.tenant_ids) {
            if let Some(group_id) = self.fetch_tenant_group_id(tenant_id).await? {
                hierarchy.push("tenant_group", Some(group_id));
            }
        }
        let HierarchyScopes {
            scopes,
            runtime_target_context,
            ..
        } = hierarchy;
        if scopes.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT c.id, c.name, c.description, c.weight, c.data, c.is_active, c.created_at, c.updated_at, \
             MAX(CASE b.scope_type \
                WHEN 'tenant_group' THEN 0 \
                WHEN 'tenant' THEN 1 \
                WHEN 'site' THEN 2 \
                WHEN 'cluster' THEN 3 \
                WHEN 'role' THEN 4 \
                WHEN 'device' THEN 5 \
                WHEN 'vm' THEN 6 \
                WHEN 'connection_target' THEN 7 \
                ELSE 0 \
             END) AS scope_precedence \
             FROM config_context c \
             JOIN config_context_binding b ON b.context_id = c.id \
             WHERE c.is_active = true AND (",
        );
        let mut is_first_scope = true;
        for (scope_type, scope_id) in &scopes {
            if !is_first_scope {
                builder.push(" OR ");
            }
            is_first_scope = false;

            builder.push("(b.scope_type = ");
            builder.push_bind(scope_type);
            builder.push(" AND b.scope_id = ");
            builder.push_bind(scope_id);
            builder.push(")");
        }
        builder.push(
            ") GROUP BY c.id, c.name, c.description, c.weight, c.data, c.is_active, c.created_at, c.updated_at \
             ORDER BY c.weight, scope_precedence, c.id",
        );

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list config contexts: {}", e))
            })?;

        let mut contexts = rows
            .iter()
            .map(Self::parse_context_row)
            .collect::<Result<Vec<ConfigContext>, AppError>>()?;
        if let Some(runtime_context) = runtime_target_context {
            contexts.insert(0, runtime_context);
        }
        Ok(contexts)
    }

    async fn list_bindings(&self, context_id: Uuid) -> Result<Vec<ConfigContextBinding>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, context_id, scope_type, scope_id
            FROM config_context_binding
            WHERE context_id = $1
            ORDER BY scope_type, scope_id
            "#,
        )
        .bind(context_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list context bindings: {}", e)))?;

        rows.iter()
            .map(Self::parse_binding_row)
            .collect::<Result<Vec<ConfigContextBinding>, AppError>>()
    }

    async fn list_bindings_page(
        &self,
        context_id: Uuid,
        filters: &ConfigContextBindingListFilters,
        sort: Option<&SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<ConfigContextBinding>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, context_id, scope_type, scope_id
            FROM config_context_binding
            WHERE context_id =
            "#,
        );
        builder.push_bind(context_id);
        if !filters.scope_type.is_empty() {
            let mut where_state = WhereState::after_predicate();
            push_string_in(
                &mut builder,
                &mut where_state,
                "scope_type",
                &filters.scope_type,
            );
        }
        push_order_by(
            &mut builder,
            CONFIG_CONTEXT_BINDING_COLUMNS,
            sort,
            "scope_type",
        );
        builder.push(" LIMIT ");
        builder.push_bind(i64::from(limit));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(offset));

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list context bindings page: {}", e))
        })?;

        let mut count_builder = QueryBuilder::new(
            r#"
            SELECT COUNT(*)::bigint
            FROM config_context_binding
            WHERE context_id =
            "#,
        );
        count_builder.push_bind(context_id);
        if !filters.scope_type.is_empty() {
            let mut where_state = WhereState::after_predicate();
            push_string_in(
                &mut count_builder,
                &mut where_state,
                "scope_type",
                &filters.scope_type,
            );
        }

        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count context bindings: {}", e)))?;

        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Context bindings total count overflow".to_string()))?;

        let items = rows
            .iter()
            .map(Self::parse_binding_row)
            .collect::<Result<Vec<ConfigContextBinding>, AppError>>()?;

        Ok(PageResult {
            items,
            total,
            limit,
            offset,
        })
    }

    async fn create_binding(
        &self,
        binding: ConfigContextBinding,
    ) -> Result<ConfigContextBinding, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO config_context_binding (id, context_id, scope_type, scope_id)
            VALUES ($1, $2, $3, $4)
            RETURNING id, context_id, scope_type, scope_id
            "#,
        )
        .bind(binding.id)
        .bind(binding.context_id)
        .bind(&binding.scope_type)
        .bind(binding.scope_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create config binding: {}", e)))?;

        Self::parse_binding_row(&row)
    }

    async fn delete_binding(&self, binding_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM config_context_binding WHERE id = $1")
            .bind(binding_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete config binding: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    async fn load_target_variable_namespace(
        &self,
        target_id: Uuid,
    ) -> Result<Option<serde_json::Value>, AppError> {
        let device_row = sqlx::query(
            r#"
            SELECT
                d.name                    AS name,
                d.primary_ip4_id          AS primary_ip4_id,
                d.primary_ip6_id          AS primary_ip6_id,
                d.custom_field_data       AS custom_field_data,
                s.name                    AS site_name,
                s.slug                    AS site_slug,
                r.name                    AS region_name,
                r.slug                    AS region_slug,
                t.name                    AS tenant_name,
                t.slug                    AS tenant_slug,
                dr.name                   AS role_name,
                dr.slug                   AS role_slug,
                pl.name                   AS platform_name,
                pl.slug                   AS platform_slug,
                dt.model                  AS device_type_model,
                dt.slug                   AS device_type_slug,
                cl.name                   AS cluster_name
            FROM dcim_device d
            LEFT JOIN dcim_site s        ON s.id = d.site_id
            LEFT JOIN dcim_region r      ON r.id = s.region_id
            LEFT JOIN tenancy_tenant t   ON t.id = d.tenant_id
            LEFT JOIN dcim_devicerole dr ON dr.id = d.role_id
            LEFT JOIN dcim_platform pl   ON pl.id = d.platform_id
            LEFT JOIN dcim_devicetype dt ON dt.id = d.device_type_id
            LEFT JOIN virtualization_cluster cl ON cl.id = d.cluster_id
            WHERE d.id = $1
            "#,
        )
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load device variable namespace: {e}"))
        })?;

        if let Some(row) = device_row {
            let primary_ip4_id: Option<Uuid> = row.try_get("primary_ip4_id").ok();
            let primary_ip6_id: Option<Uuid> = row.try_get("primary_ip6_id").ok();
            let (primary_ip4, primary_ip6) = self
                .resolve_target_endpoint_ips(
                    TargetIpOwner::Device,
                    target_id,
                    primary_ip4_id,
                    primary_ip6_id,
                )
                .await?;
            return Ok(Some(Self::build_target_variable_namespace(
                &row,
                primary_ip4,
                primary_ip6,
                true,
            )?));
        }

        let vm_row = sqlx::query(
            r#"
            SELECT
                v.name                    AS name,
                v.primary_ip4_id          AS primary_ip4_id,
                v.primary_ip6_id          AS primary_ip6_id,
                v.custom_field_data       AS custom_field_data,
                s.name                    AS site_name,
                s.slug                    AS site_slug,
                r.name                    AS region_name,
                r.slug                    AS region_slug,
                t.name                    AS tenant_name,
                t.slug                    AS tenant_slug,
                dr.name                   AS role_name,
                dr.slug                   AS role_slug,
                pl.name                   AS platform_name,
                pl.slug                   AS platform_slug,
                cl.name                   AS cluster_name
            FROM virtualization_virtualmachine v
            LEFT JOIN dcim_site s        ON s.id = v.site_id
            LEFT JOIN dcim_region r      ON r.id = s.region_id
            LEFT JOIN tenancy_tenant t   ON t.id = v.tenant_id
            LEFT JOIN dcim_devicerole dr ON dr.id = v.role_id
            LEFT JOIN dcim_platform pl   ON pl.id = v.platform_id
            LEFT JOIN virtualization_cluster cl ON cl.id = v.cluster_id
            WHERE v.id = $1
            "#,
        )
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vm variable namespace: {e}")))?;

        let Some(row) = vm_row else {
            return Ok(None);
        };
        let primary_ip4_id: Option<Uuid> = row.try_get("primary_ip4_id").ok();
        let primary_ip6_id: Option<Uuid> = row.try_get("primary_ip6_id").ok();
        let (primary_ip4, primary_ip6) = self
            .resolve_target_endpoint_ips(
                TargetIpOwner::VirtualMachine,
                target_id,
                primary_ip4_id,
                primary_ip6_id,
            )
            .await?;
        Ok(Some(Self::build_target_variable_namespace(
            &row,
            primary_ip4,
            primary_ip6,
            false,
        )?))
    }
}
