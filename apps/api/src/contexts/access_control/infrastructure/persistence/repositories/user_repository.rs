use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{Error as SqlxError, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::repository::{USER_COLUMNS, UserListFilter, UserRepository};
use super::role_repository::expand_oidc_group_ids;
use crate::contexts::access_control::domain::{Group, User};
use crate::contexts::shared::list_query::{WhereState, push_order_by, push_string_in};
use crate::contexts::shared::pagination::PageResult;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

#[derive(Debug, Clone)]
pub struct SqlxUserRepository {
    pool: PgPool,
}

impl SqlxUserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_user_row(row: &PgRow) -> Result<User, AppError> {
        Ok(User {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read user id: {}", e)))?,
            username: row
                .try_get("username")
                .map_err(|e| AppError::Database(format!("Failed to read username: {}", e)))?,
            email: row
                .try_get("email")
                .map_err(|e| AppError::Database(format!("Failed to read email: {}", e)))?,
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read status: {}", e)))?,
            auth_source: row
                .try_get("auth_source")
                .map_err(|e| AppError::Database(format!("Failed to read auth_source: {}", e)))?,
            external_id: row
                .try_get("external_id")
                .map_err(|e| AppError::Database(format!("Failed to read external_id: {}", e)))?,
        })
    }

    fn parse_group_row(row: &PgRow) -> Result<Group, AppError> {
        Ok(Group {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read group id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read group name: {}", e)))?,
            external_id: row.try_get("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read group external_id: {}", e))
            })?,
        })
    }
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    async fn get_by_id(&self, user_id: Uuid) -> Result<Option<User>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, username, email, status, auth_source, external_id
            FROM identity_user
            WHERE id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load user: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_user_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_by_username_or_email(&self, identifier: &str) -> Result<Option<User>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, username, email, status, auth_source, external_id
            FROM identity_user
            WHERE username = $1 OR email = $1
            "#,
        )
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load user by identifier: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_user_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_by_external_id(&self, external_id: &str) -> Result<Option<User>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, username, email, status, auth_source, external_id
            FROM identity_user
            WHERE external_id = $1
            "#,
        )
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load user by external ID: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_user_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_users(&self) -> Result<Vec<User>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, username, email, status, auth_source, external_id
            FROM identity_user
            ORDER BY username
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list users: {}", e)))?;

        rows.iter()
            .map(Self::parse_user_row)
            .collect::<Result<Vec<User>, AppError>>()
    }

    async fn list_users_page(
        &self,
        filter: UserListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<User>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));

        // Build the WHERE clause once and reuse it verbatim for both the page
        // query and the count query so `total` stays in lockstep with `items`.
        let push_where = |builder: &mut QueryBuilder<'_, Postgres>| {
            let mut where_state = WhereState::new();
            if let Some(search) = filter.search.as_deref() {
                where_state.and(builder);
                builder.push("(username ILIKE ");
                builder.push_bind(search.to_string());
                builder.push(" OR email ILIKE ");
                builder.push_bind(search.to_string());
                builder.push(")");
            }
            push_string_in(builder, &mut where_state, "status", &filter.status);
            push_string_in(
                builder,
                &mut where_state,
                "auth_source",
                &filter.auth_source,
            );
        };

        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT id, username, email, status, auth_source, external_id FROM identity_user",
        );
        push_where(&mut builder);
        push_order_by(&mut builder, USER_COLUMNS, filter.sort.as_ref(), "username");
        builder.push(" LIMIT ");
        builder.push_bind(limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(offset as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list users page: {}", e)))?;

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*)::bigint FROM identity_user");
        push_where(&mut count_builder);
        let total: i64 = count_builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count users: {}", e)))?;

        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Users total count overflow".to_string()))?;

        Ok(PageResult {
            items: rows
                .iter()
                .map(Self::parse_user_row)
                .collect::<Result<Vec<User>, AppError>>()?,
            total,
            limit,
            offset,
        })
    }

    async fn create_user(&self, user: User) -> Result<User, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO identity_user (id, username, email, status, auth_source, external_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, username, email, status, auth_source, external_id
            "#,
        )
        .bind(user.id)
        .bind(&user.username)
        .bind(&user.email)
        .bind(&user.status)
        .bind(&user.auth_source)
        .bind(&user.external_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_user_write_error)?;

        Self::parse_user_row(&row)
    }

    async fn update_user(&self, user: User) -> Result<User, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE identity_user
            SET username = $2,
                email = $3,
                status = $4,
                auth_source = $5,
                external_id = $6
            WHERE id = $1
            RETURNING id, username, email, status, auth_source, external_id
            "#,
        )
        .bind(user.id)
        .bind(&user.username)
        .bind(&user.email)
        .bind(&user.status)
        .bind(&user.auth_source)
        .bind(&user.external_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_user_write_error)?;

        Self::parse_user_row(&row)
    }

    async fn delete_user(&self, user_id: Uuid) -> Result<bool, AppError> {
        use crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::{detach_consumer, lock_secrets};
        let mut transaction = self.pool.begin().await?;
        if sqlx::query!(
            "SELECT id FROM identity_user WHERE id = $1 FOR UPDATE",
            user_id
        )
        .fetch_optional(&mut *transaction)
        .await?
        .is_none()
        {
            return Ok(false);
        }
        let sets = sqlx::query!("SELECT id FROM connections_user_credential_set WHERE user_id = $1 ORDER BY id FOR UPDATE", user_id)
            .fetch_all(&mut *transaction).await?;
        let records = sqlx::query!(
            "SELECT id FROM connconf_record WHERE owner_user_id = $1 ORDER BY id FOR UPDATE",
            user_id
        )
        .fetch_all(&mut *transaction)
        .await?;
        let set_ids: Vec<Uuid> = sets.iter().map(|row| row.id).collect();
        let record_ids: Vec<Uuid> = records.iter().map(|row| row.id).collect();
        let secrets = sqlx::query!("SELECT secret_id FROM secret_usage_binding WHERE (consumer_type = 'user_auth_password' AND consumer_id = $1) OR (consumer_type = 'user_connection_credential_set' AND consumer_id = ANY($2)) OR (consumer_type = 'connection_config:record' AND consumer_id = ANY($3)) ORDER BY secret_id", user_id, &set_ids, &record_ids)
            .fetch_all(&mut *transaction).await?;
        lock_secrets(
            &mut transaction,
            &secrets.iter().map(|row| row.secret_id).collect::<Vec<_>>(),
        )
        .await?;
        detach_consumer(
            &mut transaction,
            "user_auth_password",
            user_id,
            "user",
            user_id,
        )
        .await?;
        for id in set_ids {
            detach_consumer(
                &mut transaction,
                "user_connection_credential_set",
                id,
                "user",
                user_id,
            )
            .await?;
        }
        for id in record_ids {
            detach_consumer(
                &mut transaction,
                "connection_config:record",
                id,
                "user",
                user_id,
            )
            .await?;
        }
        let result = sqlx::query("DELETE FROM identity_user WHERE id = $1")
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete user: {}", e)))?;

        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_groups_by_external_ids(
        &self,
        external_ids: &[String],
    ) -> Result<Vec<Group>, AppError> {
        if external_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query(
            r#"
            SELECT id, name, external_id
            FROM identity_group
            WHERE external_id = ANY($1)
            ORDER BY name
            "#,
        )
        .bind(external_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list groups by external id: {}", e)))?;

        rows.iter()
            .map(Self::parse_group_row)
            .collect::<Result<Vec<Group>, AppError>>()
    }

    async fn list_groups_for_user(&self, user_id: Uuid) -> Result<Vec<Group>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT g.id, g.name, g.external_id
            FROM identity_group_membership gm
            JOIN identity_group g ON g.id = gm.group_id
            WHERE gm.user_id = $1
            ORDER BY g.name
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list user groups: {}", e)))?;

        rows.iter()
            .map(Self::parse_group_row)
            .collect::<Result<Vec<Group>, AppError>>()
    }

    async fn sync_oidc_group_memberships(
        &self,
        user_id: Uuid,
        oidc_group_ids: &[String],
    ) -> Result<(), AppError> {
        let expanded = expand_oidc_group_ids(oidc_group_ids);
        let mapped_rows = if expanded.is_empty() {
            Vec::new()
        } else {
            sqlx::query(
                r#"
                SELECT id, external_id
                FROM identity_group
                WHERE external_id = ANY($1)
                ORDER BY name
                "#,
            )
            .bind(&expanded)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to map OIDC groups to identity groups: {}",
                    e
                ))
            })?
        };

        let desired_group_ids = mapped_rows
            .iter()
            .map(|row| {
                row.try_get::<Uuid, _>("id").map_err(|e| {
                    AppError::Database(format!("Failed to read mapped group id: {}", e))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut tx =
            self.pool.begin().await.map_err(|e| {
                AppError::Database(format!("Failed to begin OIDC group sync: {}", e))
            })?;

        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(user_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to lock OIDC group sync for user: {}", e))
            })?;

        for row in &mapped_rows {
            let group_id: Uuid = row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read mapped group id: {}", e))
            })?;
            let external_id: Option<String> = row.try_get("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read mapped group external_id: {}", e))
            })?;
            sqlx::query(
                r#"
                INSERT INTO identity_group_membership (
                    id,
                    user_id,
                    group_id,
                    source,
                    external_id,
                    created_at,
                    updated_at
                )
                VALUES (uuidv7(), $1, $2, 'oidc', $3, now(), now())
                ON CONFLICT (user_id, group_id, source) DO UPDATE
                SET external_id = EXCLUDED.external_id,
                    updated_at = now()
                "#,
            )
            .bind(user_id)
            .bind(group_id)
            .bind(external_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to upsert OIDC group membership: {}", e))
            })?;
        }

        if desired_group_ids.is_empty() {
            sqlx::query(
                r#"
                DELETE FROM identity_group_membership
                WHERE user_id = $1
                  AND source = 'oidc'
                "#,
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to delete stale OIDC group memberships: {}",
                    e
                ))
            })?;
        } else {
            sqlx::query(
                r#"
                DELETE FROM identity_group_membership
                WHERE user_id = $1
                  AND source = 'oidc'
                  AND NOT (group_id = ANY($2))
                "#,
            )
            .bind(user_id)
            .bind(&desired_group_ids)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to delete stale OIDC group memberships: {}",
                    e
                ))
            })?;
        }

        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit OIDC group sync: {}", e)))
    }
}

/// Map a write failure on `identity_user` to a semantic [`AppError`].
///
/// The `username` and `email` columns are `UNIQUE`, so a duplicate must surface
/// as 409 Conflict rather than a generic 500. Anything else (including a pool
/// outage) is routed through the shared classifier.
fn map_user_write_error(error: SqlxError) -> AppError {
    if let SqlxError::Database(db_error) = &error
        && db_error.is_unique_violation()
    {
        let constraint = db_error.constraint().unwrap_or_default();
        if constraint.contains("identity_user_username_key") {
            return AppError::Conflict("Username already exists".to_string());
        }
        if constraint.contains("identity_user_email_key") {
            return AppError::Conflict("Email already exists".to_string());
        }
    }
    classify_sqlx_error_owned(error)
}
