use futures_util::future::BoxFuture;
use sqlx::{PgPool, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::{content_type::by_token, extras::tag::Tag};
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub trait TagAuthorizer {
    fn authorize<'a>(
        &'a mut self,
        connection: &'a mut sqlx::PgConnection,
        tag: Tag,
    ) -> BoxFuture<'a, Result<(), AppError>>;
}

/// Native membership writers authorize supplied references on their own
/// connection before acquiring entity locks or mutating rows.
pub async fn begin_native_write(
    pool: &PgPool,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, AppError> {
    let mut transaction = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    crate::contexts::inventory::application::tag_authorization::authorize_native_memberships(
        &mut transaction,
    )
    .await?;
    Ok(transaction)
}

/// Acquire before any definition or membership row in a transaction that writes
/// both. Statement triggers use the same key; locking up front avoids upgrades.
/// Mutations require READ COMMITTED; read-only assessment may use REPEATABLE READ.
pub async fn lock_tag_catalog_for_definition(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0))")
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

/// Membership-only writers share the definition catalogue lock.
pub async fn lock_tag_catalog_for_usage(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended('inventory-tag-catalog',0))")
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

#[derive(Debug, Default)]
pub struct TagFilter {
    pub ids: Option<Vec<Uuid>>,
    pub names: Vec<String>,
    pub slugs: Vec<String>,
    pub colors: Vec<String>,
    pub object_type: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTag {
    pub name: String,
    pub slug: String,
    pub color: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub weight: i32,
    /// Registered taggable content-type tokens. Omission or [] is unrestricted.
    #[serde(default)]
    pub object_types: Vec<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTag {
    #[serde(default, deserialize_with = "deserialize_non_null")]
    #[schema(nullable = false)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null")]
    #[schema(nullable = false)]
    pub slug: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null")]
    #[schema(nullable = false)]
    pub color: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null")]
    #[schema(nullable = false)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null")]
    #[schema(nullable = false)]
    pub weight: Option<i32>,
    /// Omission preserves scope; [] clears restrictions. Null is not accepted.
    #[serde(default, deserialize_with = "deserialize_non_null")]
    #[schema(nullable = false)]
    pub object_types: Option<Vec<String>>,
}

fn deserialize_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn validate_attributes(
    name: Option<&str>,
    slug: Option<&str>,
    color: Option<&str>,
    weight: Option<i32>,
) -> Result<(), AppError> {
    if name.is_some_and(|v| v.trim().is_empty() || v.chars().count() > 100)
        || slug.is_some_and(|v| {
            v.is_empty()
                || v.len() > 100
                || !v
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
        || color.is_some_and(|v| v.len() != 6 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
        || weight.is_some_and(|v| v < 0)
    {
        return Err(AppError::Validation(
            "Invalid Tag name, slug, six-digit color or nonnegative weight".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_scope_token(token: &str) -> Result<(), AppError> {
    if by_token(token).is_none() {
        return Err(AppError::Validation(
            "Unknown or non-taggable object type".into(),
        ));
    }
    Ok(())
}

/// Raw full-product replay must honor the same canonical registry as CRUD,
/// even when an archive also supplies content-type catalogue rows.
pub(crate) async fn validate_scope_registry_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    let tokens: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT c.app_label || '.' || c.model FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(classify_sqlx_error_owned)?;
    for token in tokens {
        validate_scope_token(&token)?;
    }
    Ok(())
}

async fn replace_scope(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    tokens: &[String],
) -> Result<(), AppError> {
    let mut seen = std::collections::HashSet::new();
    if tokens.iter().any(|token| !seen.insert(token)) {
        return Err(AppError::Validation("Duplicate tag object type".into()));
    }
    for token in tokens {
        validate_scope_token(token)?;
    }
    let valid: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inventory_content_type c WHERE c.app_label || '.' || c.model = ANY($1) AND c.app_label || '_' || c.model IN (SELECT table_name FROM inventory_tag_subjects())",
    ).bind(tokens).fetch_one(&mut **transaction).await.map_err(classify_sqlx_error_owned)?;
    if valid != tokens.len() as i64 {
        return Err(AppError::Validation(
            "Unknown or non-taggable object type".into(),
        ));
    }
    sqlx::query("DELETE FROM inventory_tag_object_type WHERE tag_id=$1")
        .bind(id)
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    sqlx::query("INSERT INTO inventory_tag_object_type (tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE app_label || '.' || model = ANY($2)")
        .bind(id).bind(tokens).execute(&mut **transaction).await.map_err(classify_sqlx_error_owned)?;
    Ok(())
}

const TAG_SELECT: &str = "SELECT t.id, t.name, t.slug, t.color, t.description, t.weight, ARRAY(SELECT c.app_label || '.' || c.model FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id WHERE s.tag_id=t.id ORDER BY c.app_label, c.model) AS object_types FROM inventory_tag t";

pub(crate) async fn load_membership_tags(
    connection: &mut sqlx::PgConnection,
    ids: &[Uuid],
) -> Result<Vec<Tag>, AppError> {
    let tags = sqlx::query_as::<_, Tag>(&format!("{TAG_SELECT} WHERE t.id=ANY($1) ORDER BY t.id"))
        .bind(ids)
        .fetch_all(connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(tags)
}

pub const TAG_COLUMNS: &[ListColumn] = &[
    ListColumn::enum_filter("name", "name", "name", "inventory.resources.fields.name"),
    ListColumn::enum_filter("slug", "slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::enum_filter(
        "color",
        "color",
        "color",
        "inventory.resources.fields.color",
    ),
    ListColumn::sortable("weight", "weight", "inventory.resources.fields.tagWeight"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

/// Native Tag catalogue; references always use immutable UUID identities.
#[derive(Clone)]
pub struct SqlxTagRepository {
    pool: PgPool,
}

impl SqlxTagRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Tag>, AppError> {
        // Runtime `query_as` (not the compile-time macro) keeps this read off
        // the SQLx query cache: no new offline metadata entry is required.
        sqlx::query_as::<_, Tag>(&format!("{TAG_SELECT} WHERE t.id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to load tag".into()))
    }

    pub async fn create(&self, item: &NewTag) -> Result<Tag, AppError> {
        if item.description.chars().count() > 200 {
            return Err(AppError::Validation(
                "Tag description is limited to 200 characters".into(),
            ));
        }
        validate_attributes(
            Some(&item.name),
            Some(&item.slug),
            Some(&item.color),
            Some(item.weight),
        )?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        lock_tag_catalog_for_definition(&mut tx).await?;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO inventory_tag (id,name,slug,color,description,weight) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(id).bind(&item.name).bind(&item.slug).bind(&item.color).bind(&item.description).bind(item.weight)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        replace_scope(&mut tx, id, &item.object_types).await?;
        let tag = sqlx::query_as::<_, Tag>(&format!("{TAG_SELECT} WHERE t.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(tag)
    }

    pub async fn update<A: TagAuthorizer>(
        &self,
        id: Uuid,
        changes: &UpdateTag,
        authorize: &mut A,
    ) -> Result<Option<Tag>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        lock_tag_catalog_for_definition(&mut tx).await?;
        let existing = sqlx::query_as::<_, Tag>(&format!("{TAG_SELECT} WHERE t.id=$1"))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(existing) = existing else {
            return Ok(None);
        };
        authorize.authorize(&mut tx, existing).await?;
        if changes
            .description
            .as_ref()
            .is_some_and(|v| v.chars().count() > 200)
        {
            return Err(AppError::Validation(
                "Tag description is limited to 200 characters".into(),
            ));
        }
        validate_attributes(
            changes.name.as_deref(),
            changes.slug.as_deref(),
            changes.color.as_deref(),
            changes.weight,
        )?;
        let updated = sqlx::query("UPDATE inventory_tag SET name=COALESCE($2,name),slug=COALESCE($3,slug),color=COALESCE($4,color),description=COALESCE($5,description),weight=COALESCE($6,weight) WHERE id=$1")
            .bind(id).bind(&changes.name).bind(&changes.slug).bind(&changes.color).bind(&changes.description).bind(changes.weight)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        if let Some(tokens) = &changes.object_types {
            replace_scope(&mut tx, id, tokens).await?;
        }
        let tag = sqlx::query_as::<_, Tag>(&format!("{TAG_SELECT} WHERE t.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        authorize.authorize(&mut tx, tag.clone()).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(tag))
    }

    pub async fn delete<A: TagAuthorizer>(
        &self,
        id: Uuid,
        authorize: &mut A,
    ) -> Result<u64, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        lock_tag_catalog_for_definition(&mut tx).await?;
        let existing = sqlx::query_as::<_, Tag>(&format!("{TAG_SELECT} WHERE t.id=$1"))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(existing) = existing else {
            return Ok(0);
        };
        authorize.authorize(&mut tx, existing).await?;
        let deleted = super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "inventory_tag",
            id,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(deleted)
    }

    pub async fn list_candidates(
        &self,
        filter: &TagFilter,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<Vec<Tag>, AppError> {
        if let Some(token) = &filter.object_type {
            validate_scope_token(token)?;
            let valid: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM inventory_content_type c WHERE c.app_label || '.' || c.model=$1 AND c.app_label || '_' || c.model IN (SELECT table_name FROM inventory_tag_subjects()))")
                .bind(token).fetch_one(&self.pool).await.map_err(classify_sqlx_error_owned)?;
            if !valid {
                return Err(AppError::Validation(
                    "Unknown or non-taggable object type".into(),
                ));
            }
        }
        let mut query = QueryBuilder::new(TAG_SELECT);
        let mut where_state = WhereState::new();
        push_filters(&mut query, &mut where_state, filter);
        push_search_ilike(
            &mut query,
            &mut where_state,
            &["name", "slug", "description"],
            search.as_deref(),
        );
        push_order_by(&mut query, TAG_COLUMNS, sort.as_ref(), "name");
        query
            .build_query_as::<Tag>()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to list tags".into()))
    }
}

fn push_filters<'a>(
    query: &mut QueryBuilder<'a, sqlx::Postgres>,
    state: &mut WhereState,
    filter: &'a TagFilter,
) {
    use crate::contexts::shared::list_query::{push_id_filter, push_string_in};
    push_id_filter(query, state, "id", filter.ids.as_deref());
    push_string_in(query, state, "name", &filter.names);
    push_string_in(query, state, "slug", &filter.slugs);
    push_string_in(query, state, "color", &filter.colors);
    if let Some(token) = &filter.object_type {
        state.and(query);
        query.push("(NOT EXISTS (SELECT 1 FROM inventory_tag_object_type s WHERE s.tag_id=t.id) OR EXISTS (SELECT 1 FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id WHERE s.tag_id=t.id AND c.app_label || '.' || c.model=");
        query.push_bind(token);
        query.push("))");
    }
}
