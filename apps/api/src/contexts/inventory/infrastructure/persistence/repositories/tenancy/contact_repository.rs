//! Contact repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

const FIRST_GROUP_SQL: &str = r#"
(
    SELECT cg.contactgroup_id
    FROM tenancy_contact_groups cg
    WHERE cg.contact_id = c.id
    ORDER BY cg.id
    LIMIT 1
)
"#;

pub const CONTACT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "c.name", "inventory.resources.fields.name"),
    ListColumn::fk_filter(
        "group",
        FIRST_GROUP_SQL,
        "group_id",
        FIRST_GROUP_SQL,
        "contactGroups",
        "inventory.resources.fields.group",
    ),
    ListColumn::sortable("title", "c.title", "inventory.resources.fields.title"),
    ListColumn::sortable("phone", "c.phone", "inventory.resources.fields.phone"),
    ListColumn::sortable("email", "c.email", "inventory.resources.fields.email"),
    ListColumn::sortable("address", "c.address", "inventory.resources.fields.address"),
    ListColumn::sortable("link", "c.link", "inventory.resources.fields.link"),
    ListColumn::sortable(
        "description",
        "c.description",
        "inventory.resources.fields.description",
    ),
];

/// Minimal representation of a `tenancy_contact` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ContactRecord {
    pub id: Uuid,
    pub name: String,
    pub group_id: Option<Uuid>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
}

/// Input for creating a new `tenancy_contact` row.
#[derive(Debug, Clone)]
pub struct NewContact {
    pub id: Uuid,
    pub name: String,
    pub group_id: Option<Uuid>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `tenancy_contact` row.
#[derive(Debug, Clone)]
pub struct UpdateContact {
    pub name: Option<String>,
    pub group_id: Option<Uuid>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ContactRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactRecord>, AppError>;
    async fn list(
        &self,
        group_ids: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ContactRecord>, i64), AppError>;
    async fn create(&self, new_contact: &NewContact) -> Result<ContactRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContact,
    ) -> Result<Option<ContactRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewContact],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ContactRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxContactRepository {
    pool: PgPool,
}

impl SqlxContactRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ContactRecord, AppError> {
        Ok(ContactRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read contact id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read contact name: {}", e)))?,
            group_id: row.try_get::<Option<Uuid>, _>("group_id").map_err(|e| {
                AppError::Database(format!("Failed to read contact group_id: {}", e))
            })?,
            title: row
                .try_get::<Option<String>, _>("title")
                .map_err(|e| AppError::Database(format!("Failed to read contact title: {}", e)))?,
            phone: row
                .try_get::<Option<String>, _>("phone")
                .map_err(|e| AppError::Database(format!("Failed to read contact phone: {}", e)))?,
            email: row
                .try_get::<Option<String>, _>("email")
                .map_err(|e| AppError::Database(format!("Failed to read contact email: {}", e)))?,
            address: row.try_get::<Option<String>, _>("address").map_err(|e| {
                AppError::Database(format!("Failed to read contact address: {}", e))
            })?,
            link: row
                .try_get::<Option<String>, _>("link")
                .map_err(|e| AppError::Database(format!("Failed to read contact link: {}", e)))?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read contact description: {}", e))
                })?,
        })
    }
}

impl ContactRepository for SqlxContactRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT c.id, c.name,
                   (
                       SELECT cg.contactgroup_id
                       FROM tenancy_contact_groups cg
                       WHERE cg.contact_id = c.id
                       ORDER BY cg.id
                       LIMIT 1
                   ) AS group_id,
                   c.title, c.phone, c.email, c.address, c.link, c.description
            FROM tenancy_contact c
            WHERE c.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load contact: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        group_ids: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ContactRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let search_pattern = search.as_deref().map(|s| format!("%{}%", s));

        let mut builder = QueryBuilder::new(
            r#"
            SELECT c.id, c.name,
                   (
                       SELECT cg.contactgroup_id
                       FROM tenancy_contact_groups cg
                       WHERE cg.contact_id = c.id
                       ORDER BY cg.id
                       LIMIT 1
                   ) AS group_id,
                   c.title, c.phone, c.email, c.address, c.link, c.description,
                   COUNT(*) OVER() AS total_count
            FROM tenancy_contact c
            "#,
        );
        let mut where_state = WhereState::new();
        if !group_ids.is_empty() {
            where_state.and(&mut builder);
            builder.push(
                r#"EXISTS (
                    SELECT 1
                    FROM tenancy_contact_groups cg
                    WHERE cg.contact_id = c.id
                      AND "#,
            );
            push_uuid_in(
                &mut builder,
                &mut WhereState::after_where(),
                "cg.contactgroup_id",
                group_ids,
            );
            builder.push(")");
        }
        if let Some(search_pattern) = search_pattern.as_deref() {
            where_state.and(&mut builder);
            builder.push("(c.name ILIKE ");
            builder.push_bind(search_pattern);
            builder.push(" OR c.email ILIKE ");
            builder.push_bind(search_pattern);
            builder.push(")");
        }
        push_order_by(&mut builder, CONTACT_COLUMNS, sort.as_ref(), "c.name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list contacts: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read contact total_count: {}", e)))?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_contact: &NewContact) -> Result<ContactRecord, AppError> {
        let mut tx =
            self.pool.begin().await.map_err(|e| {
                AppError::Database(format!("Failed to begin contact create: {}", e))
            })?;

        sqlx::query(
            r#"
            INSERT INTO tenancy_contact (
                id, name, title, phone, email, address, link, description,
                custom_field_data, comments, tags
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{}'::jsonb, '', '{}')
            "#,
        )
        .bind(new_contact.id)
        .bind(&new_contact.name)
        .bind(new_contact.title.as_deref().unwrap_or(""))
        .bind(new_contact.phone.as_deref().unwrap_or(""))
        .bind(new_contact.email.as_deref().unwrap_or(""))
        .bind(new_contact.address.as_deref().unwrap_or(""))
        .bind(new_contact.link.as_deref().unwrap_or(""))
        .bind(new_contact.description.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create contact: {}", e)))?;

        if let Some(group_id) = new_contact.group_id {
            sqlx::query(
                r#"
                INSERT INTO tenancy_contact_groups (contact_id, contactgroup_id)
                VALUES ($1, $2)
                "#,
            )
            .bind(new_contact.id)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("Failed to assign contact group: {}", e)))?;
        }

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit contact create: {}", e)))?;

        Ok(ContactRecord {
            id: new_contact.id,
            name: new_contact.name.clone(),
            group_id: new_contact.group_id,
            title: new_contact.title.clone(),
            phone: new_contact.phone.clone(),
            email: new_contact.email.clone(),
            address: new_contact.address.clone(),
            link: new_contact.link.clone(),
            description: new_contact.description.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContact,
    ) -> Result<Option<ContactRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let group_id = if changes.group_id.is_some() {
            changes.group_id
        } else {
            existing.group_id
        };
        let title = if changes.title.is_some() {
            changes.title.clone()
        } else {
            existing.title
        };
        let phone = if changes.phone.is_some() {
            changes.phone.clone()
        } else {
            existing.phone
        };
        let email = if changes.email.is_some() {
            changes.email.clone()
        } else {
            existing.email
        };
        let address = if changes.address.is_some() {
            changes.address.clone()
        } else {
            existing.address
        };
        let link = if changes.link.is_some() {
            changes.link.clone()
        } else {
            existing.link
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        let mut tx =
            self.pool.begin().await.map_err(|e| {
                AppError::Database(format!("Failed to begin contact update: {}", e))
            })?;

        sqlx::query(
            r#"
            UPDATE tenancy_contact
            SET name = $2, title = $3, phone = $4, email = $5,
                address = $6, link = $7, description = $8
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(title.as_deref().unwrap_or(""))
        .bind(phone.as_deref().unwrap_or(""))
        .bind(email.as_deref().unwrap_or(""))
        .bind(address.as_deref().unwrap_or(""))
        .bind(link.as_deref().unwrap_or(""))
        .bind(description.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update contact: {}", e)))?;

        sqlx::query("DELETE FROM tenancy_contact_groups WHERE contact_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("Failed to clear contact groups: {}", e)))?;

        if let Some(group_id) = group_id {
            sqlx::query(
                r#"
                INSERT INTO tenancy_contact_groups (contact_id, contactgroup_id)
                VALUES ($1, $2)
                "#,
            )
            .bind(id)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("Failed to assign contact group: {}", e)))?;
        }

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit contact update: {}", e)))?;

        Ok(Some(ContactRecord {
            id,
            name,
            group_id,
            title,
            phone,
            email,
            address,
            link,
            description,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewContact],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ContactRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk contact insert",
            |connection, item| {
                Box::pin(async move {
                    // A contact and its group link are ONE item: both statements
                    // run inside the helper's single savepoint so the pair stays
                    // atomic when the link is rejected.
                    let result = async {
                        sqlx::query(
                            r#"
                INSERT INTO tenancy_contact (
                    id, name, title, phone, email, address, link, description,
                    custom_field_data, comments, tags
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{}'::jsonb, '', '{}')
                "#,
                        )
                        .bind(item.id)
                        .bind(&item.name)
                        .bind(item.title.as_deref().unwrap_or(""))
                        .bind(item.phone.as_deref().unwrap_or(""))
                        .bind(item.email.as_deref().unwrap_or(""))
                        .bind(item.address.as_deref().unwrap_or(""))
                        .bind(item.link.as_deref().unwrap_or(""))
                        .bind(item.description.as_deref().unwrap_or(""))
                        .execute(&mut *connection)
                        .await?;

                        if let Some(group_id) = item.group_id {
                            sqlx::query(
                                r#"
                            INSERT INTO tenancy_contact_groups (contact_id, contactgroup_id)
                            VALUES ($1, $2)
                            "#,
                            )
                            .bind(item.id)
                            .bind(group_id)
                            .execute(&mut *connection)
                            .await?;
                        }
                        Ok::<(), sqlx::Error>(())
                    }
                    .await;

                    result
                        .map_err(|error| {
                            format!("Failed to insert contact '{}': {error}", item.name)
                        })
                        .map(|()| ContactRecord {
                            id: item.id,
                            name: item.name.clone(),
                            group_id: item.group_id,
                            title: item.title.clone(),
                            phone: item.phone.clone(),
                            email: item.email.clone(),
                            address: item.address.clone(),
                            link: item.link.clone(),
                            description: item.description.clone(),
                        })
                })
            },
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "tenancy_contact", id)
            .await
            .map(|_| ())
    }
}
