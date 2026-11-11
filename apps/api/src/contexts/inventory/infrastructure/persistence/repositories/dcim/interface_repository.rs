use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::{InterfaceNetworking, InterfaceNetworkingPatch};

use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const INTERFACE_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "wireless_link_id",
        "iface.wireless_link_id",
        "wireless_link_id",
        "iface.wireless_link_id",
        "wireless_links",
        "inventory.resources.fields.wireless_link_id",
    ),
    ListColumn::fk_filter(
        "wireless_lans",
        "ARRAY(SELECT wirelesslan_id FROM dcim_interface_wirelesslans WHERE interface_id = iface.id ORDER BY wirelesslan_id)",
        "wireless_lans",
        "wirelesslan_id",
        "wireless_lans",
        "inventory.resources.fields.wireless_lans",
    ),
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.interfaces.columns.device",
    ),
    ListColumn::sortable("name", "iface.name", "inventory.interfaces.columns.name"),
    ListColumn::sortable("label", "label", "inventory.interfaces.columns.label"),
    ListColumn::sortable("mtu", "mtu", "inventory.resources.fields.mtu"),
    ListColumn::enum_filter("mode", "mode", "mode", "inventory.resources.fields.mode"),
    ListColumn::sortable("speed", "speed", "inventory.resources.fields.speed"),
    ListColumn::enum_filter(
        "duplex",
        "duplex",
        "duplex",
        "inventory.resources.fields.duplex",
    ),
    ListColumn::enum_filter(
        "poe_mode",
        "poe_mode",
        "poe_mode",
        "inventory.resources.fields.poeMode",
    ),
    ListColumn::enum_filter(
        "poe_type",
        "poe_type",
        "poe_type",
        "inventory.resources.fields.poeType",
    ),
    ListColumn::enum_filter(
        "rf_role",
        "rf_role",
        "rf_role",
        "inventory.resources.fields.rfRole",
    ),
    ListColumn::sortable(
        "rf_channel",
        "rf_channel",
        "inventory.resources.fields.rfChannel",
    ),
    ListColumn::sortable(
        "rf_channel_frequency",
        "rf_channel_frequency",
        "inventory.resources.fields.rfChannelFrequency",
    ),
    ListColumn::sortable(
        "rf_channel_width",
        "rf_channel_width",
        "inventory.resources.fields.rfChannelWidth",
    ),
    ListColumn::sortable("tx_power", "tx_power", "inventory.resources.fields.txPower"),
    ListColumn::sortable("wwn", "wwn", "inventory.resources.fields.wwn"),
    ListColumn::sortable(
        "channel_id",
        "channel_id",
        "inventory.resources.fields.channelId",
    ),
    ListColumn::sortable(
        "channels",
        "channels",
        "inventory.resources.fields.channels",
    ),
    ListColumn::sortable(
        "cable_connector",
        "cable_connector",
        "inventory.resources.fields.cableConnector",
    ),
    ListColumn::enum_filter(
        "cable_end",
        "cable_end",
        "cable_end",
        "inventory.resources.fields.cableEnd",
    ),
    ListColumn::sortable(
        "cable_positions",
        "cable_positions",
        "inventory.resources.fields.cablePositions",
    ),
    ListColumn::sortable(
        "description",
        "iface.description",
        "inventory.interfaces.columns.description",
    ),
    ListColumn::enum_filter("type", "type", "type", "inventory.interfaces.columns.type"),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "inventory.interfaces.columns.enabled",
    ),
    ListColumn::enum_filter(
        "mgmt_only",
        "mgmt_only",
        "mgmt_only::text",
        "inventory.interfaces.columns.mgmtOnly",
    ),
];

/// Persisted interface and its canonical networking attributes.
#[derive(Debug, Clone)]
pub struct InterfaceRecord {
    pub networking: InterfaceNetworking,
    pub id: Uuid,
    pub device_id: Option<Uuid>,
    pub device_name: Option<String>,
    pub name: String,
    pub label: String,
    pub description: String,
    pub type_: String,
    pub enabled: bool,
    pub mgmt_only: bool,
    pub wireless_link_id: Option<Uuid>,
    pub wireless_lans: Vec<Uuid>,
}

/// Input for creating an interface.
#[derive(Debug, Clone)]
pub struct NewInterface {
    pub networking: InterfaceNetworking,
    pub id: Uuid,
    pub device_id: Uuid,
    pub name: String,
    pub type_: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
    pub wireless_link_id: Option<Uuid>,
    pub wireless_lans: Vec<Uuid>,
}

/// Fields that may be updated on an existing `dcim_interface` row.
#[derive(Debug, Clone)]
pub struct UpdateInterface {
    pub networking: InterfaceNetworkingPatch,
    pub name: Option<String>,
    pub type_: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
    pub wireless_link_id: Option<Option<Uuid>>,
    pub wireless_lans: Option<Vec<Uuid>>,
}

/// Multi-value filters for listing `dcim_interface` rows.
#[derive(Debug, Clone, Default)]
pub struct InterfaceListFilter {
    pub wireless_link_id: Vec<Uuid>,
    pub wireless_lans: Vec<Uuid>,
    /// Keys are checked against INTERFACE_COLUMNS before SQL composition.
    pub networking: Vec<(&'static str, Vec<String>)>,
    pub search: Option<String>,
    pub device_id: Vec<Uuid>,
    pub type_: Vec<String>,
    pub enabled: Vec<String>,
    pub mgmt_only: Vec<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkInterfaceResult {
    pub created: Vec<InterfaceRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait InterfaceRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InterfaceRecord>, AppError>;
    async fn list(
        &self,
        filter: InterfaceListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<InterfaceRecord>, i64), AppError>;
    async fn list_by_device(
        &self,
        device_id: Uuid,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<InterfaceRecord>, AppError>;
    async fn create(&self, new_interface: &NewInterface) -> Result<InterfaceRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInterface,
    ) -> Result<Option<InterfaceRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewInterface]) -> Result<BulkInterfaceResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxInterfaceRepository {
    pool: PgPool,
}

impl SqlxInterfaceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn write_wireless_associations(
        connection: &mut sqlx::PgConnection,
        id: Uuid,
        interface_type: &str,
        wireless_link_id: Option<Uuid>,
        wireless_lans: &[Uuid],
        replace_wireless_lans: bool,
    ) -> Result<(), AppError> {
        if (wireless_link_id.is_some() || !wireless_lans.is_empty())
            && !crate::contexts::inventory::domain::wireless::is_wireless_interface(interface_type)
        {
            return Err(AppError::Validation(
                "Wireless associations require a wireless interface type".into(),
            ));
        }
        let actual_links: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM wireless_wirelesslink WHERE interface_a_id = $1 OR interface_b_id = $1 ORDER BY id")
            .bind(id).fetch_all(&mut *connection).await?;
        if wireless_link_id.is_some_and(|link| !actual_links.contains(&link))
            || (wireless_link_id.is_none() && !actual_links.is_empty())
        {
            return Err(AppError::Validation("wireless_link_id must match the link endpoint; edit WirelessLink to change or remove the association".into()));
        }
        let unique: std::collections::HashSet<_> = wireless_lans.iter().collect();
        if unique.len() != wireless_lans.len() {
            return Err(AppError::Validation(
                "wireless_lans contains duplicate LAN IDs".into(),
            ));
        }
        sqlx::query("UPDATE dcim_interface SET wireless_link_id = $2 WHERE id = $1")
            .bind(id)
            .bind(wireless_link_id)
            .execute(&mut *connection)
            .await?;
        // Retained memberships are validation inputs, not replacement requests.
        if replace_wireless_lans {
            sqlx::query("DELETE FROM dcim_interface_wirelesslans WHERE interface_id = $1")
                .bind(id)
                .execute(&mut *connection)
                .await?;
            for lan in wireless_lans {
                sqlx::query("INSERT INTO dcim_interface_wirelesslans (id, interface_id, wirelesslan_id) VALUES ($1, $2, $3)")
                    .bind(Uuid::now_v7()).bind(id).bind(lan).execute(&mut *connection).await?;
            }
        }
        Ok(())
    }

    fn parse_row(row: &PgRow) -> Result<InterfaceRecord, AppError> {
        Ok(InterfaceRecord {
            networking: serde_json::from_value(row.try_get("networking").map_err(|e| {
                AppError::Database(format!("Failed to read interface networking: {e}"))
            })?)
            .map_err(|e| {
                AppError::Database(format!("Failed to decode interface networking: {e}"))
            })?,
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read interface id: {}", e)))?,
            device_id: row.try_get("device_id").ok(),
            device_name: row.try_get("device_name").ok(),
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read interface name: {}", e)))?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read interface label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read interface description: {}", e))
            })?,
            type_: row
                .try_get("type")
                .map_err(|e| AppError::Database(format!("Failed to read interface type: {}", e)))?,
            enabled: row.try_get("enabled").map_err(|e| {
                AppError::Database(format!("Failed to read interface enabled: {}", e))
            })?,
            mgmt_only: row.try_get("mgmt_only").map_err(|e| {
                AppError::Database(format!("Failed to read interface mgmt_only: {}", e))
            })?,
            wireless_link_id: crate::contexts::shared::row::col(row, "wireless_link_id")?,
            wireless_lans: crate::contexts::shared::row::col(row, "wireless_lans")?,
        })
    }

    fn filter_sql(param: &str, kind: FilterKind) -> Option<&'static str> {
        INTERFACE_COLUMNS.iter().find_map(|column| {
            let filter = column.filter?;
            (filter.param == param && filter.kind == kind).then_some(filter.sql)
        })
    }

    fn push_list_filters(builder: &mut QueryBuilder<Postgres>, filter: &InterfaceListFilter) {
        let has_filter = filter
            .networking
            .iter()
            .any(|(_, values)| !values.is_empty())
            || filter.search.is_some()
            || !filter.wireless_link_id.is_empty()
            || !filter.wireless_lans.is_empty()
            || !filter.device_id.is_empty()
            || !filter.type_.is_empty()
            || !filter.enabled.is_empty()
            || !filter.mgmt_only.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        if let Some(sql) = Self::filter_sql("wireless_link_id", FilterKind::Uuid) {
            push_uuid_in(builder, &mut where_state, sql, &filter.wireless_link_id);
        }
        if !filter.wireless_lans.is_empty() {
            where_state.and(builder);
            builder.push("EXISTS (SELECT 1 FROM dcim_interface_wirelesslans WHERE interface_id = iface.id AND ");
            push_uuid_in(
                builder,
                &mut WhereState::after_where(),
                "wirelesslan_id",
                &filter.wireless_lans,
            );
            builder.push(")");
        }
        for (param, values) in &filter.networking {
            if let Some(sql) = Self::filter_sql(param, FilterKind::String) {
                push_string_in(builder, &mut where_state, sql, values);
            }
        }
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "iface.name",
                "iface.label",
                "iface.description",
                "iface.type",
            ],
            filter.search.as_deref(),
        );

        if let Some(sql) = Self::filter_sql("device_id", FilterKind::Uuid) {
            push_uuid_in(builder, &mut where_state, sql, &filter.device_id);
        }
        if let Some(sql) = Self::filter_sql("type", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.type_);
        }
        if let Some(sql) = Self::filter_sql("enabled", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.enabled);
        }
        if let Some(sql) = Self::filter_sql("mgmt_only", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.mgmt_only);
        }
    }

    async fn count_filtered(&self, filter: &InterfaceListFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM dcim_interface iface");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count interfaces: {}", e)))
    }
}

impl InterfaceRepository for SqlxInterfaceRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InterfaceRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT to_jsonb(iface) || jsonb_build_object('speed', iface.speed::text) AS networking, iface.id,
                   iface.device_id,
                   device.name AS device_name,
                   iface.name,
                   iface.label,
                   iface.description,
                   iface.type,
                   iface.enabled,
                   iface.mgmt_only,
                   iface.wireless_link_id,
                   ARRAY(SELECT wirelesslan_id FROM dcim_interface_wirelesslans WHERE interface_id = iface.id ORDER BY wirelesslan_id) AS wireless_lans
            FROM dcim_interface iface
            LEFT JOIN dcim_device device ON device.id = iface.device_id
            WHERE iface.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load interface: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: InterfaceListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<InterfaceRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT to_jsonb(iface) || jsonb_build_object('speed', iface.speed::text) AS networking, iface.id,
                   iface.device_id,
                   device.name AS device_name,
                   iface.name,
                   iface.label,
                   iface.description,
                   iface.type,
                   iface.enabled,
                   iface.mgmt_only,
                   iface.wireless_link_id,
                   ARRAY(SELECT wirelesslan_id FROM dcim_interface_wirelesslans WHERE interface_id = iface.id ORDER BY wirelesslan_id) AS wireless_lans,
                   COUNT(*) OVER() AS total_count
            FROM dcim_interface iface
            LEFT JOIN dcim_device device ON device.id = iface.device_id
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(&mut builder, INTERFACE_COLUMNS, sort.as_ref(), "iface.name");
        builder.push(", iface.id");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list interfaces: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| {
                AppError::Database(format!("Failed to read interface total_count: {}", e))
            })?;
        let total = match total {
            Some(total) => total,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn list_by_device(
        &self,
        device_id: Uuid,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<InterfaceRecord>, AppError> {
        self.list(
            InterfaceListFilter {
                device_id: vec![device_id],
                ..InterfaceListFilter::default()
            },
            limit,
            offset,
            None,
        )
        .await
        .map(|(items, _total)| items)
    }

    async fn create(&self, new_interface: &NewInterface) -> Result<InterfaceRecord, AppError> {
        new_interface.networking.validate()?;
        let label = new_interface.label.clone().unwrap_or_default();
        let description = new_interface.description.clone().unwrap_or_default();
        let enabled = new_interface.enabled.unwrap_or(true);
        let mgmt_only = new_interface.mgmt_only.unwrap_or(false);
        let mut tx = self.pool.begin().await?;

        // Match source graph observation before any component/FK row locks.
        // bulk_create delegates here, so each item's transaction has this intent.
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;

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
                device_id,
                mtu, mode, speed, duplex, poe_mode, poe_type, rf_role, rf_channel,
                rf_channel_frequency, rf_channel_width, tx_power, wwn, channel_id,
                channels, cable_connector, cable_end, cable_positions, vlan_translation_policy_id
            )
            SELECT $1, '{}'::jsonb, $2, $3, $4, false, $5, $2, $6, $7, $8,
                n.mtu, n.mode, n.speed, n.duplex, n.poe_mode, n.poe_type, n.rf_role, n.rf_channel,
                n.rf_channel_frequency, n.rf_channel_width, n.tx_power, n.wwn, n.channel_id,
                n.channels, n.cable_connector, n.cable_end, n.cable_positions, n.vlan_translation_policy_id
            FROM jsonb_populate_record(NULL::dcim_interface, $9) n
            "#,
        )
        .bind(new_interface.id)
        .bind(&new_interface.name)
        .bind(&label)
        .bind(&description)
        .bind(enabled)
        .bind(&new_interface.type_)
        .bind(mgmt_only)
        .bind(new_interface.device_id)
        .bind(sqlx::types::Json(&new_interface.networking))
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create interface: {}", e)))?;

        Self::write_wireless_associations(
            &mut tx,
            new_interface.id,
            &new_interface.type_,
            new_interface.wireless_link_id,
            &new_interface.wireless_lans,
            true,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        self.get_by_id(new_interface.id)
            .await?
            .ok_or_else(|| AppError::Database("Created interface disappeared".to_string()))
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInterface,
    ) -> Result<Option<InterfaceRecord>, AppError> {
        let mut tx = self.pool.begin().await?;
        // Even an endpoint on an omitted side can move onto a selected side.
        // Serialize before reading/locking it, not after deciding from a snapshot
        // whether its current cable association appears relevant.
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let row = sqlx::query(
            r#"
            SELECT iface.*,
                   to_jsonb(iface) || jsonb_build_object('speed', iface.speed::text) AS networking,
                   NULL::text AS device_name,
                   ARRAY[]::uuid[] AS wireless_lans
            FROM dcim_interface iface
            WHERE iface.id = $1
            FOR UPDATE OF iface
            "#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut existing = Self::parse_row(&row)?;
        // A locking statement keeps its old M2M snapshot after a lock wait.
        // Read memberships separately once the Interface row lock is held.
        existing.wireless_lans = sqlx::query_scalar(
            "SELECT wirelesslan_id FROM dcim_interface_wirelesslans WHERE interface_id = $1 ORDER BY wirelesslan_id",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let wireless_link_id = changes
            .wireless_link_id
            .unwrap_or(existing.wireless_link_id);
        let wireless_lans = changes
            .wireless_lans
            .as_deref()
            .unwrap_or(&existing.wireless_lans);

        let name = changes.name.clone().unwrap_or(existing.name);
        let type_ = changes.type_.clone().unwrap_or(existing.type_);
        let label = changes.label.clone().unwrap_or(existing.label);
        let description = changes.description.clone().unwrap_or(existing.description);
        let enabled = changes.enabled.unwrap_or(existing.enabled);
        let mgmt_only = changes.mgmt_only.unwrap_or(existing.mgmt_only);
        let networking = changes.networking.apply(&existing.networking);
        networking.validate()?;

        sqlx::query(
            r#"
            UPDATE dcim_interface
            SET name = $2, _name = $2, type = $3, label = $4,
                description = $5, enabled = $6, mgmt_only = $7,
                (mtu, mode, speed, duplex, poe_mode, poe_type, rf_role, rf_channel,
                 rf_channel_frequency, rf_channel_width, tx_power, wwn, channel_id,
                 channels, cable_connector, cable_end, cable_positions, vlan_translation_policy_id) =
                (SELECT n.mtu, n.mode, n.speed, n.duplex, n.poe_mode, n.poe_type, n.rf_role, n.rf_channel,
                 n.rf_channel_frequency, n.rf_channel_width, n.tx_power, n.wwn, n.channel_id,
                 n.channels, n.cable_connector, n.cable_end, n.cable_positions, n.vlan_translation_policy_id
                 FROM jsonb_populate_record(NULL::dcim_interface, $8) n)
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&type_)
        .bind(&label)
        .bind(&description)
        .bind(enabled)
        .bind(mgmt_only)
        .bind(sqlx::types::Json(&networking))
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update interface: {}", e)))?;

        Self::write_wireless_associations(
            &mut tx,
            id,
            &type_,
            wireless_link_id,
            wireless_lans,
            changes.wireless_lans.is_some(),
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        self.get_by_id(id).await
    }

    async fn bulk_create(&self, items: &[NewInterface]) -> Result<BulkInterfaceResult, AppError> {
        for item in items {
            item.networking.validate()?;
        }
        let mut created = Vec::new();
        let mut errors = Vec::new();

        for item in items {
            // One transaction per item includes its M2M rows. Failed associations
            // cannot leave a partially created interface behind.
            match self.create(item).await {
                Ok(record) => created.push(record),
                Err(e) => errors.push(format!("Failed to insert interface '{}': {}", item.name, e)),
            }
        }

        Ok(BulkInterfaceResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "dcim_interface",
            id,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
