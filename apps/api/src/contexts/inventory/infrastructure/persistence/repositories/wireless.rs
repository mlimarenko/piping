//! Native Wireless persistence over the existing baseline tables.

use std::sync::Arc;

use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, postgres::PgRow};
use uuid::Uuid;

use crate::contexts::inventory::domain::wireless::{
    WirelessLan, WirelessLanGroup, WirelessLanGroupWrite, WirelessLanWrite, WirelessLink,
    WirelessLinkWrite, is_wireless_interface,
};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::SqlxSecretRepository;
use crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch;
use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const WIRELESS_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "parent_id",
        "t.parent_id",
        "parent_id",
        "t.parent_id",
        "wireless_lan_groups",
        "inventory.resources.fields.parent_id",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "t.owner_id",
        "owner_id",
        "t.owner_id",
        "owners",
        "inventory.resources.fields.owner_id",
    ),
    ListColumn::sortable("name", "t.name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "t.slug", "inventory.resources.fields.slug"),
    ListColumn::sortable(
        "description",
        "t.description",
        "inventory.resources.fields.description",
    ),
];
pub const WIRELESS_LAN_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "group_id",
        "t.group_id",
        "group_id",
        "t.group_id",
        "wireless_lan_groups",
        "inventory.resources.fields.group_id",
    ),
    ListColumn::fk_filter(
        "vlan_id",
        "t.vlan_id",
        "vlan_id",
        "t.vlan_id",
        "vlans",
        "inventory.resources.fields.vlan_id",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "t.tenant_id",
        "tenant_id",
        "t.tenant_id",
        "tenants",
        "inventory.resources.fields.tenant_id",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "t.owner_id",
        "owner_id",
        "t.owner_id",
        "owners",
        "inventory.resources.fields.owner_id",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "(SELECT app_label || '.' || model FROM inventory_content_type WHERE id = t.scope_type_id)",
        "(SELECT app_label || '.' || model FROM inventory_content_type WHERE id = t.scope_type_id)",
        "inventory.resources.fields.scope_type",
    ),
    ListColumn::fk_filter(
        "scope_id",
        "t.scope_id",
        "scope_id",
        "t.scope_id",
        "scopes",
        "inventory.resources.fields.scope_id",
    ),
    ListColumn::enum_filter(
        "auth_type",
        "t.auth_type",
        "t.auth_type",
        "inventory.resources.fields.auth_type",
    ),
    ListColumn::enum_filter(
        "auth_cipher",
        "t.auth_cipher",
        "t.auth_cipher",
        "inventory.resources.fields.auth_cipher",
    ),
    ListColumn::sortable("ssid", "t.ssid", "inventory.resources.fields.ssid"),
    ListColumn::enum_filter(
        "status",
        "t.status",
        "t.status",
        "inventory.resources.fields.status",
    ),
    ListColumn::sortable(
        "description",
        "t.description",
        "inventory.resources.fields.description",
    ),
];
pub const WIRELESS_LINK_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "interface_a_id",
        "t.interface_a_id",
        "interface_a_id",
        "t.interface_a_id",
        "interfaces",
        "inventory.resources.fields.interface_a_id",
    ),
    ListColumn::fk_filter(
        "interface_b_id",
        "t.interface_b_id",
        "interface_b_id",
        "t.interface_b_id",
        "interfaces",
        "inventory.resources.fields.interface_b_id",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "t.tenant_id",
        "tenant_id",
        "t.tenant_id",
        "tenants",
        "inventory.resources.fields.tenant_id",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "t.owner_id",
        "owner_id",
        "t.owner_id",
        "owners",
        "inventory.resources.fields.owner_id",
    ),
    ListColumn::enum_filter(
        "auth_type",
        "t.auth_type",
        "t.auth_type",
        "inventory.resources.fields.auth_type",
    ),
    ListColumn::enum_filter(
        "auth_cipher",
        "t.auth_cipher",
        "t.auth_cipher",
        "inventory.resources.fields.auth_cipher",
    ),
    ListColumn::sortable(
        "distance",
        "t.distance",
        "inventory.resources.fields.distance",
    ),
    ListColumn::enum_filter(
        "distance_unit",
        "t.distance_unit",
        "t.distance_unit",
        "inventory.resources.fields.distance_unit",
    ),
    ListColumn::sortable("ssid", "t.ssid", "inventory.resources.fields.ssid"),
    ListColumn::enum_filter(
        "status",
        "t.status",
        "t.status",
        "inventory.resources.fields.status",
    ),
    ListColumn::sortable(
        "description",
        "t.description",
        "inventory.resources.fields.description",
    ),
];

#[derive(Debug, Clone, Copy)]
pub enum WirelessKind {
    Group,
    Lan,
    Link,
}

#[derive(Debug, Clone, Default)]
pub struct WirelessListFilter {
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
    pub ids: Vec<(&'static str, Vec<Uuid>)>,
    pub strings: Vec<(&'static str, Vec<String>)>,
}

impl WirelessListFilter {
    fn push(
        &self,
        builder: &mut QueryBuilder<Postgres>,
        state: &mut WhereState,
        kind: WirelessKind,
    ) -> Result<(), AppError> {
        for (param, values) in &self.ids {
            let column = filter_column(kind.columns(), "Wireless", param)?;
            if column.kind != FilterKind::Uuid {
                return Err(AppError::Validation(format!(
                    "Invalid UUID filter: {param}"
                )));
            }
            push_uuid_in(builder, state, column.sql, values);
        }
        for (param, values) in &self.strings {
            let column = filter_column(kind.columns(), "Wireless", param)?;
            push_string_in(builder, state, column.sql, values);
        }
        Ok(())
    }
}

impl WirelessKind {
    pub fn table(self) -> &'static str {
        match self {
            Self::Group => "wireless_wirelesslangroup",
            Self::Lan => "wireless_wirelesslan",
            Self::Link => "wireless_wirelesslink",
        }
    }
    pub fn columns(self) -> &'static [ListColumn] {
        match self {
            Self::Group => WIRELESS_GROUP_COLUMNS,
            Self::Lan => WIRELESS_LAN_COLUMNS,
            Self::Link => WIRELESS_LINK_COLUMNS,
        }
    }
    fn projection(self) -> String {
        let specific = match self {
            Self::Group => "t.name, t.slug, t.parent_id",
            Self::Lan => {
                "t.ssid, t.group_id, t.status, t.vlan_id, t.tenant_id, (SELECT app_label || '.' || model FROM inventory_content_type WHERE id = t.scope_type_id) AS scope_type, t.scope_id, t.auth_type, t.auth_cipher, t.auth_psk"
            }
            Self::Link => {
                "t.interface_a_id, t.interface_b_id, t.ssid, t.status, t.tenant_id, t.auth_type, t.auth_cipher, t.auth_psk, t.distance::float8 AS distance, t.distance_unit"
            }
        };
        format!(
            "t.id, {specific}, t.description, t.comments, t.owner_id, t.tags, t.custom_field_data, t.created, t.last_updated, t.external_id, t.piping_synced_at"
        )
    }
    fn label(self) -> &'static str {
        if matches!(self, Self::Group) {
            "t.name"
        } else {
            "t.ssid"
        }
    }
}

macro_rules! parse_wireless {
    ($fn_name:ident, $model:ident, $($field:ident),* $(,)?) => {
        fn $fn_name(row: &PgRow) -> Result<$model, AppError> {
            Ok($model {
                id: col(row, "id")?,
                $($field: col(row, stringify!($field))?,)*
                description: col(row, "description")?, comments: col(row, "comments")?,
                owner_id: col(row, "owner_id")?, tags: col(row, "tags")?,
                custom_fields: col(row, "custom_field_data")?, created: col(row, "created")?,
                last_updated: col(row, "last_updated")?, external_id: col(row, "external_id")?,
                piping_synced_at: col(row, "piping_synced_at")?,
            })
        }
    };
}
parse_wireless!(parse_group, WirelessLanGroup, name, slug, parent_id);
parse_wireless!(
    parse_lan,
    WirelessLan,
    ssid,
    group_id,
    status,
    vlan_id,
    tenant_id,
    scope_type,
    scope_id,
    auth_type,
    auth_cipher,
    auth_psk
);
parse_wireless!(
    parse_link,
    WirelessLink,
    interface_a_id,
    interface_b_id,
    ssid,
    status,
    tenant_id,
    auth_type,
    auth_cipher,
    auth_psk,
    distance,
    distance_unit
);

#[derive(Clone)]
pub struct SqlxWirelessRepository {
    pool: PgPool,
    secret_intents: Arc<SecretFieldIntentService<SqlxSecretRepository>>,
}

impl SqlxWirelessRepository {
    pub fn new(pool: PgPool) -> Self {
        let secret_intents = Arc::new(SecretFieldIntentService::new(Arc::new(
            SqlxSecretRepository::new(pool.clone()),
        )));
        Self {
            pool,
            secret_intents,
        }
    }

    async fn load<T>(
        connection: &mut PgConnection,
        kind: WirelessKind,
        id: Uuid,
        lock: bool,
        parse: fn(&PgRow) -> Result<T, AppError>,
    ) -> Result<Option<T>, AppError> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(kind.projection())
            .push(" FROM ")
            .push(kind.table())
            .push(" t WHERE t.id = ")
            .push_bind(id);
        if lock {
            query.push(" FOR UPDATE OF t");
        }
        let row = query.build().fetch_optional(connection).await?;
        row.as_ref().map(parse).transpose()
    }

    async fn list<T>(
        &self,
        kind: WirelessKind,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: WirelessListFilter,
        parse: fn(&PgRow) -> Result<T, AppError>,
    ) -> Result<(Vec<T>, i64), AppError> {
        let from = format!(" FROM {} t", kind.table());
        let mut count = QueryBuilder::<Postgres>::new(format!("SELECT count(*){from}"));
        let mut condition = WhereState::new();
        push_search_ilike(
            &mut count,
            &mut condition,
            &[kind.label()],
            filter.search.as_deref(),
        );
        filter.push(&mut count, &mut condition, kind)?;
        let total: i64 = count.build_query_scalar().fetch_one(&self.pool).await?;
        let mut query =
            QueryBuilder::<Postgres>::new(format!("SELECT {}{from}", kind.projection()));
        let mut condition = WhereState::new();
        push_search_ilike(
            &mut query,
            &mut condition,
            &[kind.label()],
            filter.search.as_deref(),
        );
        filter.push(&mut query, &mut condition, kind)?;
        push_order_by(&mut query, kind.columns(), filter.sort.as_ref(), "t.id");
        if filter.sort.is_some() {
            query.push(", t.id");
        }
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query.build().fetch_all(&self.pool).await?;
        Ok((rows.iter().map(parse).collect::<Result<_, _>>()?, total))
    }

    pub async fn get_group(&self, id: Uuid) -> Result<Option<WirelessLanGroup>, AppError> {
        Self::load(
            &mut *self.pool.acquire().await?,
            WirelessKind::Group,
            id,
            false,
            parse_group,
        )
        .await
    }
    pub async fn get_lan(&self, id: Uuid) -> Result<Option<WirelessLan>, AppError> {
        Self::load(
            &mut *self.pool.acquire().await?,
            WirelessKind::Lan,
            id,
            false,
            parse_lan,
        )
        .await
    }
    pub async fn get_link(&self, id: Uuid) -> Result<Option<WirelessLink>, AppError> {
        Self::load(
            &mut *self.pool.acquire().await?,
            WirelessKind::Link,
            id,
            false,
            parse_link,
        )
        .await
    }
    pub async fn list_groups(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: WirelessListFilter,
    ) -> Result<(Vec<WirelessLanGroup>, i64), AppError> {
        self.list(WirelessKind::Group, limit, offset, filter, parse_group)
            .await
    }
    pub async fn list_lans(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: WirelessListFilter,
    ) -> Result<(Vec<WirelessLan>, i64), AppError> {
        self.list(WirelessKind::Lan, limit, offset, filter, parse_lan)
            .await
    }
    pub async fn list_links(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: WirelessListFilter,
    ) -> Result<(Vec<WirelessLink>, i64), AppError> {
        self.list(WirelessKind::Link, limit, offset, filter, parse_link)
            .await
    }

    pub async fn delete(&self, kind: WirelessKind, id: Uuid) -> Result<(), AppError> {
        if super::delete_guard::delete_inventory_record(&self.pool, kind.table(), id).await? == 0 {
            return Err(AppError::NotFound("Wireless object not found".into()));
        }
        Ok(())
    }

    async fn save_group(
        connection: &mut PgConnection,
        value: &WirelessLanGroup,
        update: bool,
    ) -> Result<(), AppError> {
        value.validate()?;
        // Serialize hierarchy changes so concurrent reparenting cannot create a cycle.
        sqlx::query("SELECT pg_advisory_xact_lock(917701)")
            .execute(&mut *connection)
            .await?;
        let mut parent = value.parent_id;
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = parent {
            if id == value.id || !seen.insert(id) || seen.len() > 200 {
                return Err(AppError::Validation(
                    "Wireless LAN group hierarchy contains a cycle or exceeds 200 levels".into(),
                ));
            }
            let row = sqlx::query("SELECT parent_id FROM wireless_wirelesslangroup WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?
                .ok_or_else(|| AppError::Validation("parent_id does not exist".into()))?;
            parent = col(&row, "parent_id")?;
        }
        let sql = if update {
            "UPDATE wireless_wirelesslangroup SET name = $2, slug = $3, parent_id = $4, description = $5, comments = $6, owner_id = $7, tags = $8, custom_field_data = $9, external_id = $10, piping_synced_at = $11, last_updated = now() WHERE id = $1"
        } else {
            "INSERT INTO wireless_wirelesslangroup (id, name, slug, parent_id, description, comments, owner_id, tags, custom_field_data, external_id, piping_synced_at, created, last_updated, lft, rght, tree_id, level) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now(), now(), 0, 0, 0, 0)"
        };
        sqlx::query(sql)
            .bind(value.id)
            .bind(&value.name)
            .bind(&value.slug)
            .bind(value.parent_id)
            .bind(&value.description)
            .bind(&value.comments)
            .bind(value.owner_id)
            .bind(&value.tags)
            .bind(&value.custom_fields)
            .bind(value.external_id)
            .bind(value.piping_synced_at)
            .execute(connection)
            .await?;
        Ok(())
    }

    async fn save_lan(
        connection: &mut PgConnection,
        value: &WirelessLan,
        update: bool,
    ) -> Result<(), AppError> {
        value.validate()?;
        let scope_type_id =
            if let (Some(token), Some(id)) = (value.scope_type.as_deref(), value.scope_id) {
                let def = crate::contexts::inventory::domain::content_type::CONTENT_TYPES
                    .iter()
                    .find(|def| def.token() == token)
                    .ok_or_else(|| AppError::Validation("Unknown scope_type".into()))?;
                let mut query = QueryBuilder::<Postgres>::new("SELECT id FROM ");
                query
                    .push(def.table)
                    .push(" WHERE id = ")
                    .push_bind(id)
                    .push(" FOR KEY SHARE");
                if query
                    .build()
                    .fetch_optional(&mut *connection)
                    .await?
                    .is_none()
                {
                    return Err(AppError::Validation("scope_id does not exist".into()));
                }
                let row = sqlx::query(
                    "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2",
                )
                .bind(def.app_label)
                .bind(def.model)
                .fetch_one(&mut *connection)
                .await?;
                Some(col::<Uuid>(&row, "id")?)
            } else {
                None
            };
        let sql = if update {
            "UPDATE wireless_wirelesslan SET ssid = $2, group_id = $3, status = $4, vlan_id = $5, tenant_id = $6, scope_type_id = $7, scope_id = $8, auth_type = $9, auth_cipher = $10, auth_psk = $11, description = $12, comments = $13, owner_id = $14, tags = $15, custom_field_data = $16, external_id = $17, piping_synced_at = $18, last_updated = now() WHERE id = $1"
        } else {
            "INSERT INTO wireless_wirelesslan (id, ssid, group_id, status, vlan_id, tenant_id, scope_type_id, scope_id, auth_type, auth_cipher, auth_psk, description, comments, owner_id, tags, custom_field_data, external_id, piping_synced_at, created, last_updated) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, now(), now())"
        };
        sqlx::query(sql)
            .bind(value.id)
            .bind(&value.ssid)
            .bind(value.group_id)
            .bind(&value.status)
            .bind(value.vlan_id)
            .bind(value.tenant_id)
            .bind(scope_type_id)
            .bind(value.scope_id)
            .bind(&value.auth_type)
            .bind(&value.auth_cipher)
            .bind(&value.auth_psk)
            .bind(&value.description)
            .bind(&value.comments)
            .bind(value.owner_id)
            .bind(&value.tags)
            .bind(&value.custom_fields)
            .bind(value.external_id)
            .bind(value.piping_synced_at)
            .execute(connection)
            .await?;
        Ok(())
    }

    async fn save_link(
        connection: &mut PgConnection,
        value: &WirelessLink,
        update: bool,
    ) -> Result<(), AppError> {
        value.validate()?;
        let mut endpoints = [value.interface_a_id, value.interface_b_id];
        endpoints.sort();
        // Lock in UUID order, shared with Interface association/type updates.
        for id in endpoints {
            let row = sqlx::query("SELECT type FROM dcim_interface WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?
                .ok_or_else(|| {
                    AppError::Validation("Wireless link interface does not exist".into())
                })?;
            let kind: String = col(&row, "type")?;
            if !is_wireless_interface(&kind) {
                return Err(AppError::Validation(
                    "Wireless link endpoints must be wireless interfaces".into(),
                ));
            }
        }
        let sql = if update {
            "UPDATE wireless_wirelesslink SET interface_a_id = $2, interface_b_id = $3, ssid = $4, status = $5, tenant_id = $6, auth_type = $7, auth_cipher = $8, auth_psk = $9, distance = $10, distance_unit = $11, description = $12, comments = $13, owner_id = $14, tags = $15, custom_field_data = $16, external_id = $17, piping_synced_at = $18, last_updated = now() WHERE id = $1"
        } else {
            "INSERT INTO wireless_wirelesslink (id, interface_a_id, interface_b_id, ssid, status, tenant_id, auth_type, auth_cipher, auth_psk, distance, distance_unit, description, comments, owner_id, tags, custom_field_data, external_id, piping_synced_at, created, last_updated) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, now(), now())"
        };
        // Postgres accepts float8 assignment to NUMERIC(8,2); the domain range is checked above.
        sqlx::query(sql)
            .bind(value.id)
            .bind(value.interface_a_id)
            .bind(value.interface_b_id)
            .bind(&value.ssid)
            .bind(&value.status)
            .bind(value.tenant_id)
            .bind(&value.auth_type)
            .bind(&value.auth_cipher)
            .bind(&value.auth_psk)
            .bind(value.distance)
            .bind(&value.distance_unit)
            .bind(&value.description)
            .bind(&value.comments)
            .bind(value.owner_id)
            .bind(&value.tags)
            .bind(&value.custom_fields)
            .bind(value.external_id)
            .bind(value.piping_synced_at)
            .execute(&mut *connection)
            .await?;
        // Interface.wireless_link is the canonical reverse association, not an independent choice.
        sqlx::query("UPDATE dcim_interface SET wireless_link_id = NULL WHERE wireless_link_id = $1 AND NOT (id = ANY($2))").bind(value.id).bind(&endpoints[..]).execute(&mut *connection).await?;
        sqlx::query("UPDATE dcim_interface SET wireless_link_id = $1 WHERE id = ANY($2)")
            .bind(value.id)
            .bind(&endpoints[..])
            .execute(connection)
            .await?;
        Ok(())
    }
}

macro_rules! wireless_mutations {
    ($create:ident, $update:ident, $record:ident, $write:ident, $kind:ident, $parse:ident, $save:ident) => {
        impl SqlxWirelessRepository {
            pub async fn $create(
                &self,
                value: $record,
                inputs: Vec<ApplySecretFieldIntentInput>,
            ) -> Result<$record, AppError> {
                let inputs =
                    SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
                let mut tx = super::extras::tag_repository::begin_native_write(&self.pool).await?;
                if matches!(WirelessKind::$kind, WirelessKind::Link) {
                    super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
                }
                // Keep hierarchy, secret-batch and entity locks in the same order as update.
                if matches!(WirelessKind::$kind, WirelessKind::Group) {
                    sqlx::query("SELECT pg_advisory_xact_lock(917701)")
                        .execute(&mut *tx)
                        .await?;
                }
                lock_field_intent_batch(&mut tx, &inputs).await?;
                Self::$save(&mut tx, &value, false).await?;
                self.secret_intents
                    .apply_intents_in_transaction(&mut tx, inputs)
                    .await?;
                let result = Self::load(&mut tx, WirelessKind::$kind, value.id, false, $parse)
                    .await?
                    .ok_or_else(|| {
                        AppError::NotFound("Wireless object not found after insert".into())
                    })?;
                tx.commit().await?;
                Ok(result)
            }
            pub async fn $update(
                &self,
                id: Uuid,
                changes: $write,
                inputs: Vec<ApplySecretFieldIntentInput>,
            ) -> Result<Option<$record>, AppError> {
                let inputs =
                    SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
                let mut tx = super::extras::tag_repository::begin_native_write(&self.pool).await?;
                if matches!(WirelessKind::$kind, WirelessKind::Link) {
                    super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
                }
                // Hierarchy lock must precede the row lock on both create and update paths.
                if matches!(WirelessKind::$kind, WirelessKind::Group) {
                    sqlx::query("SELECT pg_advisory_xact_lock(917701)")
                        .execute(&mut *tx)
                        .await?;
                }
                lock_field_intent_batch(&mut tx, &inputs).await?;
                let Some(mut value) =
                    Self::load(&mut tx, WirelessKind::$kind, id, true, $parse).await?
                else {
                    return Ok(None);
                };
                changes.apply(&mut value)?;
                Self::$save(&mut tx, &value, true).await?;
                self.secret_intents
                    .apply_intents_in_transaction(&mut tx, inputs)
                    .await?;
                let result = Self::load(&mut tx, WirelessKind::$kind, id, false, $parse).await?;
                tx.commit().await?;
                Ok(result)
            }
        }
    };
}
wireless_mutations!(
    create_group,
    update_group,
    WirelessLanGroup,
    WirelessLanGroupWrite,
    Group,
    parse_group,
    save_group
);
wireless_mutations!(
    create_lan,
    update_lan,
    WirelessLan,
    WirelessLanWrite,
    Lan,
    parse_lan,
    save_lan
);
wireless_mutations!(
    create_link,
    update_link,
    WirelessLink,
    WirelessLinkWrite,
    Link,
    parse_link,
    save_link
);
