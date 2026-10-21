use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::connection_config::domain::{
    ConnConfProtocol, ConnConfRecord, NewConnConfRecord, UpdateConnConfRecord,
};
use crate::contexts::shared::delete_guard::{
    BlockerTableMetadata, delete_record_guarded_in_transaction,
};
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const CONNCONF_RECORD_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("id", "id", "connectionConfig.records.columns.id"),
    ListColumn::sortable("name", "name", "connectionConfig.records.columns.name"),
    ListColumn::sortable(
        "description",
        "description",
        "connectionConfig.records.columns.description",
    ),
    ListColumn::enum_filter(
        "protocol",
        "protocol",
        "protocol",
        "connectionConfig.records.columns.protocol",
    ),
    ListColumn::fk_filter(
        "base_record_id",
        "base_record_id",
        "base_record_id",
        "base_record_id",
        "connection_config_records",
        "connectionConfig.records.columns.baseRecord",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "connectionConfig.records.columns.enabled",
    ),
    ListColumn::fk_filter(
        "owner_user_id",
        "owner_user_id",
        "owner_user_id",
        "owner_user_id",
        "users",
        "connectionConfig.records.columns.owner",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "connectionConfig.records.columns.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "connectionConfig.records.columns.updatedAt",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct ConnConfRecordFilter {
    pub protocol: Vec<ConnConfProtocol>,
    pub base_record_id: Vec<Uuid>,
    pub owner_user_id: Vec<Uuid>,
    pub enabled: Vec<String>,
    pub sort: Option<SortSpec>,
}

/// CRUD persistence for `connconf_record`.
#[async_trait]
pub trait ConnConfRecordRepository: Send + Sync {
    async fn list(&self, filter: ConnConfRecordFilter) -> Result<Vec<ConnConfRecord>, AppError>;
    async fn list_all(&self) -> Result<Vec<ConnConfRecord>, AppError>;
    async fn list_by_protocol(
        &self,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<ConnConfRecord>, AppError>;
    async fn list_by_protocol_with_bases(
        &self,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<ConnConfRecord>, AppError>;
    async fn find(&self, id: Uuid) -> Result<Option<ConnConfRecord>, AppError>;
    async fn has_direct_children(&self, id: Uuid) -> Result<bool, AppError>;
    async fn create(&self, input: &NewConnConfRecord) -> Result<ConnConfRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        input: &UpdateConnConfRecord,
    ) -> Result<Option<ConnConfRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<bool, AppError>;
}

#[derive(Clone)]
pub struct SqlxConnConfRecordRepository {
    pool: PgPool,
}

impl SqlxConnConfRecordRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// SELECT order is identical to the try_get keys in `map_row` — no aliasing.
const SELECT_COLUMNS: &str = "id, name, description, protocol, base_record_id, enabled, \
    owner_user_id, params, custom_field_data, created_at, updated_at";
const SELECT_COLUMNS_BASE: &str = "base.id, base.name, base.description, base.protocol, \
    base.base_record_id, base.enabled, base.owner_user_id, base.params, \
    base.custom_field_data, base.created_at, base.updated_at";

fn connconf_blocker_metadata(table: &str) -> BlockerTableMetadata {
    match table {
        "connconf_record" => BlockerTableMetadata {
            parent_join: None,
            entity_key: Some("Connection Config Records"),
        },
        _ => BlockerTableMetadata::default(),
    }
}

fn push_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    filter: &ConnConfRecordFilter,
    state: &mut WhereState,
) {
    if !filter.protocol.is_empty() {
        state.and(builder);
        builder.push("protocol IN (");
        let mut separated = builder.separated(", ");
        for protocol in &filter.protocol {
            separated.push_bind(protocol.as_str());
        }
        builder.push(")");
    }
    push_uuid_in(builder, state, "base_record_id", &filter.base_record_id);
    push_uuid_in(builder, state, "owner_user_id", &filter.owner_user_id);
    push_string_in(builder, state, "enabled::text", &filter.enabled);
}

fn map_row(row: sqlx::postgres::PgRow) -> Result<ConnConfRecord, AppError> {
    let protocol_raw: String = row
        .try_get("protocol")
        .map_err(|e| AppError::Database(format!("connconf_record.protocol: {e}")))?;
    let protocol = ConnConfProtocol::parse(&protocol_raw).ok_or_else(|| {
        AppError::Database(format!(
            "connconf_record.protocol has unknown value '{protocol_raw}'"
        ))
    })?;
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("connconf_record.id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| AppError::Database(format!("connconf_record.name: {e}")))?;
    let description: String = row
        .try_get("description")
        .map_err(|e| AppError::Database(format!("connconf_record.description: {e}")))?;
    let base_record_id: Option<Uuid> = row
        .try_get("base_record_id")
        .map_err(|e| AppError::Database(format!("connconf_record.base_record_id: {e}")))?;
    let enabled: bool = row
        .try_get("enabled")
        .map_err(|e| AppError::Database(format!("connconf_record.enabled: {e}")))?;
    let owner_user_id: Option<Uuid> = row
        .try_get("owner_user_id")
        .map_err(|e| AppError::Database(format!("connconf_record.owner_user_id: {e}")))?;
    let params: Value = row
        .try_get("params")
        .map_err(|e| AppError::Database(format!("connconf_record.params: {e}")))?;
    let custom_field_data: Value = row
        .try_get("custom_field_data")
        .map_err(|e| AppError::Database(format!("connconf_record.custom_field_data: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("connconf_record.created_at: {e}")))?;
    let updated_at: DateTime<Utc> = row
        .try_get("updated_at")
        .map_err(|e| AppError::Database(format!("connconf_record.updated_at: {e}")))?;
    Ok(ConnConfRecord {
        id,
        name,
        description,
        protocol,
        base_record_id,
        enabled,
        owner_user_id,
        params,
        custom_field_data,
        created_at,
        updated_at,
    })
}

#[async_trait]
impl ConnConfRecordRepository for SqlxConnConfRecordRepository {
    async fn list(&self, filter: ConnConfRecordFilter) -> Result<Vec<ConnConfRecord>, AppError> {
        let mut builder = QueryBuilder::new("SELECT ");
        builder.push(SELECT_COLUMNS);
        builder.push(" FROM connconf_record");
        let mut where_state = WhereState::new();
        push_filters(&mut builder, &filter, &mut where_state);
        push_order_by(
            &mut builder,
            CONNCONF_RECORD_COLUMNS,
            filter.sort.as_ref(),
            "protocol, name",
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        rows.into_iter().map(map_row).collect()
    }

    async fn list_all(&self) -> Result<Vec<ConnConfRecord>, AppError> {
        self.list(ConnConfRecordFilter::default()).await
    }

    async fn list_by_protocol(
        &self,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<ConnConfRecord>, AppError> {
        self.list(ConnConfRecordFilter {
            protocol: vec![protocol],
            ..ConnConfRecordFilter::default()
        })
        .await
    }

    async fn list_by_protocol_with_bases(
        &self,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<ConnConfRecord>, AppError> {
        let query = format!(
            "WITH RECURSIVE record_tree AS ( \
                SELECT {SELECT_COLUMNS} FROM connconf_record WHERE protocol = $1 \
                UNION \
                SELECT {SELECT_COLUMNS_BASE} \
                FROM connconf_record base \
                JOIN record_tree child ON child.base_record_id = base.id \
             ) \
             SELECT {SELECT_COLUMNS} FROM record_tree ORDER BY protocol, name"
        );
        let rows = sqlx::query(&query)
            .bind(protocol.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        rows.into_iter().map(map_row).collect()
    }

    async fn find(&self, id: Uuid) -> Result<Option<ConnConfRecord>, AppError> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM connconf_record WHERE id = $1");
        let row = sqlx::query(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.map(map_row).transpose()
    }

    async fn has_direct_children(&self, id: Uuid) -> Result<bool, AppError> {
        let has_children: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM connconf_record WHERE base_record_id = $1)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Ok(has_children)
    }

    async fn create(&self, input: &NewConnConfRecord) -> Result<ConnConfRecord, AppError> {
        let query = format!(
            "INSERT INTO connconf_record \
             (name, description, protocol, base_record_id, enabled, owner_user_id, params) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(&input.name)
            .bind(input.description.clone().unwrap_or_default())
            .bind(input.protocol.as_str())
            .bind(input.base_record_id)
            .bind(input.enabled.unwrap_or(true))
            .bind(input.owner_user_id)
            .bind(
                input
                    .params
                    .clone()
                    .unwrap_or(Value::Object(serde_json::Map::new())),
            )
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        map_row(row)
    }

    async fn update(
        &self,
        id: Uuid,
        input: &UpdateConnConfRecord,
    ) -> Result<Option<ConnConfRecord>, AppError> {
        // COALESCE applies only provided fields. base_record_id uses
        // `Option<Option<Uuid>>` so callers can explicitly clear it.
        let query = format!(
            "UPDATE connconf_record SET \
               name = COALESCE($2, name), \
               description = COALESCE($3, description), \
               protocol = COALESCE($4, protocol), \
               base_record_id = CASE WHEN $5::boolean THEN $6 ELSE base_record_id END, \
               enabled = COALESCE($7, enabled), \
               params = COALESCE($8, params), \
               updated_at = now() \
             WHERE id = $1 \
             RETURNING {SELECT_COLUMNS}"
        );
        let (base_set, base_value) = match input.base_record_id {
            Some(inner) => (true, inner),
            None => (false, None),
        };
        let row = sqlx::query(&query)
            .bind(id)
            .bind(input.name.as_deref())
            .bind(input.description.as_deref())
            .bind(input.protocol.map(|protocol| protocol.as_str()))
            .bind(base_set)
            .bind(base_value)
            .bind(input.enabled)
            .bind(input.params.clone())
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.map(map_row).transpose()
    }

    async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        let mut transaction = self.pool.begin().await?;
        let Some(record) = sqlx::query!(
            "SELECT owner_user_id FROM connconf_record WHERE id = $1",
            id
        )
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(false);
        };
        if let Some(user_id) = record.owner_user_id {
            // Parent deletion cascades the record; its later row lock decides
            // idempotent absence rather than a parent RowNotFound error.
            sqlx::query!(
                "SELECT id FROM identity_user WHERE id = $1 FOR UPDATE",
                user_id
            )
            .fetch_optional(&mut *transaction)
            .await?;
        }
        let Some(locked) = sqlx::query!(
            "SELECT owner_user_id FROM connconf_record WHERE id = $1 FOR UPDATE",
            id
        )
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(false);
        };
        if locked.owner_user_id != record.owner_user_id {
            return Err(AppError::Conflict(
                "Connection record ownership changed during deletion".into(),
            ));
        }
        let (owner_scope_type, owner_scope_id) = locked
            .owner_user_id
            .map_or(("connection_config", id), |owner| ("user", owner));
        crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::detach_consumer(
            &mut transaction, "connection_config:record", id, owner_scope_type, owner_scope_id,
        ).await?;
        let deleted = delete_record_guarded_in_transaction(
            &mut transaction,
            "connconf_record",
            id,
            connconf_blocker_metadata,
        )
        .await?;
        transaction.commit().await?;
        Ok(deleted > 0)
    }
}
