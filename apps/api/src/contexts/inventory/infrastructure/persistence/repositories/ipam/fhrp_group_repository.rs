use std::sync::Arc;

use sqlx::{PgPool, Postgres, QueryBuilder, Row, postgres::PgRow};
use uuid::Uuid;

use crate::contexts::inventory::domain::FhrpGroup;
use crate::contexts::secrets::application::consumer_type::{
    FHRP_AUTH_KEY_FIELD, INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::SqlxSecretRepository;
use crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const FHRP_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "auth_type",
        "auth_type",
        "inventory.resources.fields.auth_type",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "owners",
        "inventory.resources.fields.owner_id",
    ),
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "group_id",
        "group_id",
        "inventory.resources.fields.group_id",
    ),
    ListColumn::enum_filter(
        "protocol",
        "protocol",
        "protocol",
        "inventory.resources.fields.protocol",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

macro_rules! fhrp_select { () => { "SELECT id, group_id, name, protocol, auth_type, description, comments, owner_id, tags, custom_field_data, created, last_updated, external_id, piping_synced_at FROM ipam_fhrpgroup" }; }

#[derive(Debug, Clone, Default)]
pub struct FhrpGroupFilter {
    pub owner_id: Vec<Uuid>,
    pub protocol: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "Matches the inventory repository ports; consumers use the concrete SQLx repository"
)]
pub trait FhrpGroupRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<FhrpGroup>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: FhrpGroupFilter,
    ) -> Result<(Vec<FhrpGroup>, i64), AppError>;
    async fn create(&self, item: &FhrpGroup) -> Result<(), AppError>;
    async fn update(&self, item: &FhrpGroup) -> Result<(), AppError>;
    async fn create_with_secret_intents(
        &self,
        item: &FhrpGroup,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<(), AppError>;
    async fn update_with_secret_intents(
        &self,
        item: &FhrpGroup,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxFhrpGroupRepository {
    pool: PgPool,
    secret_intents: Arc<SecretFieldIntentService<SqlxSecretRepository>>,
}

fn parse_row(row: &PgRow) -> Result<FhrpGroup, sqlx::Error> {
    Ok(FhrpGroup {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        name: row.try_get("name")?,
        protocol: row.try_get("protocol")?,
        auth_type: row.try_get("auth_type")?,
        auth_key: None,
        description: row.try_get("description")?,
        comments: row.try_get("comments")?,
        owner_id: row.try_get("owner_id")?,
        tags: row.try_get("tags")?,
        custom_fields: Some(row.try_get("custom_field_data")?),
        created: row.try_get("created")?,
        last_updated: row.try_get("last_updated")?,
        external_id: row.try_get("external_id")?,
        piping_synced_at: row.try_get("piping_synced_at")?,
    })
}

fn push_filters(
    builder: &mut QueryBuilder<Postgres>,
    filter: &FhrpGroupFilter,
) -> Result<(), AppError> {
    let mut state = WhereState::new();
    push_uuid_in(
        builder,
        &mut state,
        filter_column(FHRP_GROUP_COLUMNS, "FHRP group", "owner_id")?.sql,
        &filter.owner_id,
    );
    push_string_in(
        builder,
        &mut state,
        filter_column(FHRP_GROUP_COLUMNS, "FHRP group", "protocol")?.sql,
        &filter.protocol,
    );
    push_search_ilike(
        builder,
        &mut state,
        &["name", "protocol", "group_id", "description", "comments"],
        filter.search.as_deref(),
    );
    Ok(())
}

impl SqlxFhrpGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        let secret_intents = Arc::new(SecretFieldIntentService::new(Arc::new(
            SqlxSecretRepository::new(pool.clone()),
        )));
        Self {
            pool,
            secret_intents,
        }
    }

    fn prepare_intents(
        item: &FhrpGroup,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Vec<ApplySecretFieldIntentInput>, AppError> {
        item.validate()?;
        let native = item
            .auth_key
            .as_ref()
            .map(|key| ApplySecretFieldIntentInput {
                consumer_type: INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into(),
                consumer_id: item.id,
                owner_scope_type: INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into(),
                owner_scope_id: item.id,
                field_key: FHRP_AUTH_KEY_FIELD.into(),
                intent: key.intent,
                value: key.value.clone(),
                secret_id: None,
            });
        native
            .into_iter()
            .chain(inputs)
            .map(SecretFieldIntentService::<SqlxSecretRepository>::validate_input)
            .collect()
    }
}

impl FhrpGroupRepository for SqlxFhrpGroupRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<FhrpGroup>, AppError> {
        sqlx::query(concat!(fhrp_select!(), " WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(parse_row)
            .transpose()
            .map_err(classify_sqlx_error_owned)
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: FhrpGroupFilter,
    ) -> Result<(Vec<FhrpGroup>, i64), AppError> {
        let mut count = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM ipam_fhrpgroup");
        push_filters(&mut count, &filter)?;
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::<Postgres>::new(fhrp_select!());
        push_filters(&mut query, &filter)?;
        push_order_by(
            &mut query,
            FHRP_GROUP_COLUMNS,
            filter.sort.as_ref(),
            "group_id",
        );
        query.push(", id");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items = rows
            .iter()
            .map(parse_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(classify_sqlx_error_owned)?;
        Ok((items, total))
    }

    async fn create(&self, item: &FhrpGroup) -> Result<(), AppError> {
        self.create_with_secret_intents(item, Vec::new()).await
    }

    async fn create_with_secret_intents(
        &self,
        item: &FhrpGroup,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<(), AppError> {
        let inputs = Self::prepare_intents(item, inputs)?;
        let mut transaction =
            super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        lock_field_intent_batch(&mut transaction, &inputs).await?;
        sqlx::query("INSERT INTO ipam_fhrpgroup (id, group_id, name, protocol, auth_type, description, comments, owner_id, tags, custom_field_data, external_id, piping_synced_at, created, last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,NOW(),NOW())")
            .bind(item.id).bind(item.group_id).bind(&item.name).bind(&item.protocol)
            .bind(&item.auth_type).bind(&item.description).bind(&item.comments)
            .bind(item.owner_id).bind(&item.tags).bind(item.custom_fields.clone().unwrap_or_else(|| serde_json::json!({})))
            .bind(item.external_id).bind(item.piping_synced_at)
            .execute(&mut *transaction).await.map_err(classify_sqlx_error_owned)?;
        self.secret_intents
            .apply_intents_in_transaction(&mut transaction, inputs)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn update(&self, item: &FhrpGroup) -> Result<(), AppError> {
        self.update_with_secret_intents(item, Vec::new()).await
    }

    async fn update_with_secret_intents(
        &self,
        item: &FhrpGroup,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<(), AppError> {
        let inputs = Self::prepare_intents(item, inputs)?;
        let mut transaction =
            super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        lock_field_intent_batch(&mut transaction, &inputs).await?;
        let result = sqlx::query("UPDATE ipam_fhrpgroup SET group_id=$2, name=$3, protocol=$4, auth_type=$5, description=$6, comments=$7, owner_id=$8, tags=$9, custom_field_data=custom_field_data || $10::jsonb, external_id=$11, piping_synced_at=$12, last_updated=NOW() WHERE id=$1")
            .bind(item.id).bind(item.group_id).bind(&item.name).bind(&item.protocol)
            .bind(&item.auth_type).bind(&item.description).bind(&item.comments)
            .bind(item.owner_id).bind(&item.tags).bind(item.custom_fields.clone().unwrap_or_else(|| serde_json::json!({})))
            .bind(item.external_id).bind(item.piping_synced_at)
            .execute(&mut *transaction).await.map_err(classify_sqlx_error_owned)?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("FHRP group not found".into()));
        }
        self.secret_intents
            .apply_intents_in_transaction(&mut transaction, inputs)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let deleted =
            super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_fhrpgroup", id)
                .await?;
        if deleted == 0 {
            return Err(AppError::NotFound("FHRP group not found".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::fhrp_group_assignment_repository::{
        FhrpGroupAssignmentFilter, FhrpGroupAssignmentRepository, SqlxFhrpGroupAssignmentRepository,
    };
    use super::*;
    use crate::contexts::inventory::domain::FhrpGroupAssignment;

    /// Uses the real baseline, including its UUID content-type registry and constraints.
    #[sqlx::test(migrations = "./migrations")]
    async fn round_trips_metadata_and_enforces_typed_assignment(pool: PgPool) {
        let groups = SqlxFhrpGroupRepository::new(pool.clone());
        let assignments = SqlxFhrpGroupAssignmentRepository::new(pool.clone());
        let mut group = FhrpGroup {
            id: Uuid::new_v4(),
            group_id: 42,
            name: "gateway".into(),
            protocol: "vrrp3".into(),
            auth_type: Some("md5".into()),
            auth_key: None,
            description: "edge pair".into(),
            comments: "test metadata".into(),
            tags: vec!["edge".into()],
            custom_fields: Some(serde_json::json!({"region":"west"})),
            external_id: Some(123),
            piping_synced_at: Some(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            ..Default::default()
        };
        groups.create(&group).await.unwrap();
        let stored = groups.get_by_id(group.id).await.unwrap().unwrap();
        assert!(stored.created.is_some());
        assert_eq!(stored.created, stored.last_updated);
        group.created = stored.created;
        group.last_updated = stored.last_updated;
        assert_eq!(
            serde_json::to_value(&stored).unwrap(),
            serde_json::to_value(&group).unwrap()
        );
        assert!(matches!(
            groups.create(&group).await,
            Err(AppError::Conflict(_))
        ));
        group.name = "renamed".into();
        group.created = None;
        group.last_updated = None;
        groups.update(&group).await.unwrap();
        let updated = groups.get_by_id(group.id).await.unwrap().unwrap();
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.created, stored.created);
        assert!(updated.last_updated >= stored.last_updated);
        let (page, total) = groups
            .list_paginated(Some(1), Some(999), FhrpGroupFilter::default())
            .await
            .unwrap();
        assert!(page.is_empty());
        assert_eq!(total, 1);

        let interface_id = Uuid::new_v4();
        sqlx::query("INSERT INTO dcim_interface (id, custom_field_data, name, label, description, mark_connected, enabled, _name, type, mgmt_only) VALUES ($1,'{}','eth0','','',false,true,'eth0','virtual',false)")
            .bind(interface_id).execute(&pool).await.unwrap();
        let mut assignment = FhrpGroupAssignment {
            id: Uuid::new_v4(),
            group_id: group.id,
            interface_type: "dcim.interface".into(),
            interface_id,
            priority: 255,
            external_id: Some(456),
            piping_synced_at: group.piping_synced_at,
            ..Default::default()
        };
        assignments.create(&assignment).await.unwrap();
        let stored_assignment = assignments.get_by_id(assignment.id).await.unwrap().unwrap();
        assignment.created = stored_assignment.created;
        assignment.last_updated = stored_assignment.last_updated;
        assert_eq!(
            serde_json::to_value(&assignment).unwrap(),
            serde_json::to_value(&stored_assignment).unwrap()
        );
        let mut duplicate = assignment.clone();
        duplicate.id = Uuid::new_v4();
        assert!(matches!(
            assignments.create(&duplicate).await,
            Err(AppError::Conflict(_))
        ));
        assignment.priority = 0;
        assignments.update(&assignment).await.unwrap();
        assert_eq!(
            assignments
                .get_by_id(assignment.id)
                .await
                .unwrap()
                .unwrap()
                .priority,
            0
        );
        assignment.interface_type = "virtualization.vminterface".into();
        assert!(matches!(
            assignments.update(&assignment).await,
            Err(AppError::Validation(_))
        ));
        assignment.interface_type = "dcim.interface".into();
        assignment.interface_id = Uuid::new_v4();
        assert!(matches!(
            assignments.update(&assignment).await,
            Err(AppError::Validation(_))
        ));
        let (page, total) = assignments
            .list_paginated(Some(1), Some(999), FhrpGroupAssignmentFilter::default())
            .await
            .unwrap();
        assert!(page.is_empty());
        assert_eq!(total, 1);
        assignments.delete(assignment.id).await.unwrap();
        assert!(
            assignments
                .get_by_id(assignment.id)
                .await
                .unwrap()
                .is_none()
        );
        groups.delete(group.id).await.unwrap();
        assert!(groups.get_by_id(group.id).await.unwrap().is_none());
    }
}
