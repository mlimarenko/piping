//! ContactAssignment repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::content_type;
use crate::contexts::inventory::infrastructure::persistence::repositories::content_type_lookup::lookup_content_type_id;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const CONTACT_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "contentType",
        "object_type_id",
        "content_type",
        "object_type_id",
        "contentTypes",
        "inventory.resources.fields.contentType",
    ),
    ListColumn::sortable("objectId", "object_id", "inventory.resources.fields.object"),
    ListColumn::fk_filter(
        "contact",
        "contact_id",
        "contact_id",
        "contact_id",
        "contacts",
        "inventory.resources.fields.contact",
    ),
    ListColumn::fk_filter(
        "role",
        "role_id",
        "role_id",
        "role_id",
        "contactRoles",
        "inventory.resources.fields.role",
    ),
    ListColumn::enum_filter(
        "priority",
        "priority",
        "priority",
        "inventory.resources.fields.priority",
    ),
];

/// Minimal representation of a `tenancy_contactassignment` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ContactAssignmentRecord {
    pub id: Uuid,
    pub content_type: Uuid,
    pub object_id: Uuid,
    pub contact_id: Uuid,
    pub role_id: Uuid,
    pub priority: Option<String>,
}

/// Input for creating a new `tenancy_contactassignment` row.
#[derive(Debug, Clone)]
pub struct NewContactAssignment {
    pub id: Uuid,
    pub content_type: Uuid,
    pub object_id: Uuid,
    pub contact_id: Uuid,
    pub role_id: Uuid,
    pub priority: Option<String>,
}

/// Fields that may be updated on an existing `tenancy_contactassignment` row.
#[derive(Debug, Clone)]
pub struct UpdateContactAssignment {
    pub content_type: Option<Uuid>,
    pub object_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub priority: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkContactAssignmentResult {
    pub created: Vec<ContactAssignmentRecord>,
    pub errors: Vec<String>,
}

/// Filters, search and sort of one `list` call.
#[derive(Debug, Default)]
pub struct ContactAssignmentFilter<'a> {
    pub content_types: &'a [Uuid],
    pub object_ids: &'a [Uuid],
    pub role_ids: &'a [Uuid],
    pub contact_ids: &'a [Uuid],
    pub priorities: &'a [String],
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ContactAssignmentRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactAssignmentRecord>, AppError>;
    async fn list(
        &self,
        filter: ContactAssignmentFilter<'_>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ContactAssignmentRecord>, i64), AppError>;
    async fn create(
        &self,
        new_assignment: &NewContactAssignment,
    ) -> Result<ContactAssignmentRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContactAssignment,
    ) -> Result<Option<ContactAssignmentRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewContactAssignment],
    ) -> Result<BulkContactAssignmentResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxContactAssignmentRepository {
    pool: PgPool,
}

impl SqlxContactAssignmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Resolve a NetBox-style content type (`app_label`, `model`) to its seeded
    /// `inventory_content_type` UUID. Pure SELECT — zero-legacy: the content-type
    /// vocabulary is seeded from the registry by migration, so write-time never
    /// mints a row. An unknown/unseeded type is a client error (400), never an
    /// INSERT.
    pub async fn resolve_content_type(
        &self,
        app_label: &str,
        model: &str,
    ) -> Result<Uuid, AppError> {
        lookup_content_type_id(&self.pool, app_label, model)
            .await?
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "Unknown content type {:?}: not a seeded inventory content type",
                    content_type::token(app_label, model)
                ))
            })
    }

    fn parse_row(row: &PgRow) -> Result<ContactAssignmentRecord, AppError> {
        Ok(ContactAssignmentRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read contact_assignment id: {}", e))
            })?,
            content_type: row.try_get("object_type_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read contact_assignment object_type_id: {}",
                    e
                ))
            })?,
            object_id: row.try_get("object_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read contact_assignment object_id: {}",
                    e
                ))
            })?,
            contact_id: row.try_get("contact_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read contact_assignment contact_id: {}",
                    e
                ))
            })?,
            role_id: row.try_get("role_id").map_err(|e| {
                AppError::Database(format!("Failed to read contactassignment role_id: {}", e))
            })?,
            priority: row.try_get::<Option<String>, _>("priority").map_err(|e| {
                AppError::Database(format!("Failed to read contactassignment priority: {}", e))
            })?,
        })
    }
}

impl ContactAssignmentRepository for SqlxContactAssignmentRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactAssignmentRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, object_type_id, object_id, contact_id, role_id, priority
            FROM tenancy_contactassignment
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load contact_assignment: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: ContactAssignmentFilter<'_>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ContactAssignmentRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT a.id, a.object_type_id, a.object_id, a.contact_id, a.role_id, a.priority,
                   COUNT(*) OVER() AS total_count
            FROM tenancy_contactassignment a
            LEFT JOIN inventory_content_type ct ON ct.id = a.object_type_id
            LEFT JOIN tenancy_contact c ON c.id = a.contact_id
            LEFT JOIN tenancy_contactrole r ON r.id = a.role_id
            "#,
        );
        push_contact_assignment_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            CONTACT_ASSIGNMENT_COLUMNS,
            filter.sort.as_ref(),
            "a.id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list contact_assignments: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read contact_assignment total_count: {}",
                    e
                ))
            })?,
            None => count_contact_assignments(&self.pool, &filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(
        &self,
        new_assignment: &NewContactAssignment,
    ) -> Result<ContactAssignmentRecord, AppError> {
        sqlx::query(
            r#"
            INSERT INTO tenancy_contactassignment (
                id, object_type_id, object_id, contact_id, role_id, priority, custom_field_data, tags
            )
            VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, '{}')
            "#,
        )
        .bind(new_assignment.id)
        .bind(new_assignment.content_type)
        .bind(new_assignment.object_id)
        .bind(new_assignment.contact_id)
        .bind(new_assignment.role_id)
        .bind(new_assignment.priority.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create contact_assignment: {}", e)))?;

        Ok(ContactAssignmentRecord {
            id: new_assignment.id,
            content_type: new_assignment.content_type,
            object_id: new_assignment.object_id,
            contact_id: new_assignment.contact_id,
            role_id: new_assignment.role_id,
            priority: new_assignment.priority.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContactAssignment,
    ) -> Result<Option<ContactAssignmentRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let content_type = changes.content_type.unwrap_or(existing.content_type);
        let object_id = changes.object_id.unwrap_or(existing.object_id);
        let contact_id = changes.contact_id.unwrap_or(existing.contact_id);
        let role_id = changes.role_id.unwrap_or(existing.role_id);
        let priority = if changes.priority.is_some() {
            changes.priority.clone()
        } else {
            existing.priority
        };

        sqlx::query(
            r#"
            UPDATE tenancy_contactassignment
            SET object_type_id = $2, object_id = $3, contact_id = $4, role_id = $5, priority = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(content_type)
        .bind(object_id)
        .bind(contact_id)
        .bind(role_id)
        .bind(priority.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update contact_assignment: {}", e)))?;

        Ok(Some(ContactAssignmentRecord {
            id,
            content_type,
            object_id,
            contact_id,
            role_id,
            priority,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewContactAssignment],
    ) -> Result<BulkContactAssignmentResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                r#"
                INSERT INTO tenancy_contactassignment (
                    id, object_type_id, object_id, contact_id, role_id, priority, custom_field_data, tags
                )
                VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, '{}')
                "#,
            )
            .bind(item.id)
            .bind(item.content_type)
            .bind(item.object_id)
            .bind(item.contact_id)
            .bind(item.role_id)
            .bind(item.priority.as_deref()),
            )
            .await?;

            match result {
                Ok(_) => created.push(ContactAssignmentRecord {
                    id: item.id,
                    content_type: item.content_type,
                    object_id: item.object_id,
                    contact_id: item.contact_id,
                    role_id: item.role_id,
                    priority: item.priority.clone(),
                }),
                Err(e) => errors.push(format!("Failed to insert contact_assignment: {}", e)),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!(
                "Failed to commit bulk contact_assignment insert: {}",
                e
            ))
        })?;

        Ok(BulkContactAssignmentResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "tenancy_contactassignment",
            id,
        )
        .await
        .map(|_| ())
    }
}

fn push_contact_assignment_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &ContactAssignmentFilter<'_>,
) {
    let ContactAssignmentFilter {
        content_types,
        object_ids,
        role_ids,
        contact_ids,
        priorities,
        search,
        sort: _,
    } = filter;
    let mut where_state = WhereState::new();
    push_uuid_in(builder, &mut where_state, "a.object_type_id", content_types);
    push_uuid_in(builder, &mut where_state, "a.object_id", object_ids);
    push_uuid_in(builder, &mut where_state, "a.role_id", role_ids);
    push_uuid_in(builder, &mut where_state, "a.contact_id", contact_ids);
    push_string_in(builder, &mut where_state, "a.priority", priorities);
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "a.object_id",
            "a.priority",
            "ct.app_label",
            "ct.model",
            "c.name",
            "c.title",
            "c.phone",
            "c.email",
            "r.name",
            "r.slug",
        ],
        search.as_deref(),
    );
}

async fn count_contact_assignments(
    pool: &PgPool,
    filter: &ContactAssignmentFilter<'_>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new(
        r#"
        SELECT COUNT(*)
        FROM tenancy_contactassignment a
        LEFT JOIN inventory_content_type ct ON ct.id = a.object_type_id
        LEFT JOIN tenancy_contact c ON c.id = a.contact_id
        LEFT JOIN tenancy_contactrole r ON r.id = a.role_id
        "#,
    );
    push_contact_assignment_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count contact_assignments: {e}")))
}
