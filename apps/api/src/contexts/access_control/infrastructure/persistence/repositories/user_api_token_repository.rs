use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use super::repository::UserApiTokenRepository;
use crate::contexts::access_control::application::ports::UserApiTokenListFilter;
use crate::contexts::access_control::domain::{
    Action, ResourceType, UserApiToken, UserApiTokenScope, UserApiTokenStatus,
    is_valid_permission_pair,
};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::audit::infrastructure::persistence::insert_audit_event;
use crate::contexts::shared::list_query::{ListColumn, push_order_by};
use crate::errors::AppError;

const SCOPE_WIRE_SQL: &str = "CASE WHEN LOWER(s.resource_type) = 'cctv' THEN 'cctv:' || LOWER(s.action) ELSE LOWER(s.action) || ':' || LOWER(s.resource_type) END";
const SCOPE_AGGREGATE_SQL: &str = "(SELECT COALESCE(string_agg(CASE WHEN LOWER(s.resource_type) = 'cctv' THEN 'cctv:' || LOWER(s.action) ELSE LOWER(s.action) || ':' || LOWER(s.resource_type) END, ',' ORDER BY s.action, s.resource_type), '') FROM auth_user_api_token_scope s WHERE s.token_id = t.id)";

pub const USER_API_TOKEN_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "t.name", "userSettings.tokens.columns.name"),
    ListColumn::enum_filter_with_param(
        "scopes",
        SCOPE_AGGREGATE_SQL,
        "scope",
        SCOPE_WIRE_SQL,
        "userSettings.tokens.columns.scopes",
    ),
    ListColumn::enum_filter(
        "status",
        "t.status",
        "t.status",
        "userSettings.tokens.columns.status",
    ),
    ListColumn::sortable(
        "created_at",
        "t.created_at",
        "userSettings.tokens.columns.created_at",
    ),
    ListColumn::sortable(
        "expires_at",
        "t.expires_at",
        "userSettings.tokens.columns.expires_at",
    ),
];

#[derive(Debug, Clone)]
pub struct SqlxUserApiTokenRepository {
    pool: PgPool,
}

impl SqlxUserApiTokenRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn load_scopes(
        tx: &mut Transaction<'_, Postgres>,
        token_id: Uuid,
    ) -> Result<Vec<UserApiTokenScope>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT action, resource_type, resource_pattern
            FROM auth_user_api_token_scope
            WHERE token_id = $1
            ORDER BY action, resource_type, resource_pattern
            "#,
        )
        .bind(token_id)
        .fetch_all(tx.as_mut())
        .await
        .map_err(|error| AppError::Database(format!("Failed to load token scopes: {error}")))?;

        let mut scopes = Vec::with_capacity(rows.len());
        for row in rows {
            let action_raw: String = row
                .try_get("action")
                .map_err(|error| AppError::Database(format!("Failed to read action: {error}")))?;
            let resource_raw: String = row.try_get("resource_type").map_err(|error| {
                AppError::Database(format!("Failed to read resource_type: {error}"))
            })?;
            let resource_pattern: String = row.try_get("resource_pattern").map_err(|error| {
                AppError::Database(format!("Failed to read resource_pattern: {error}"))
            })?;
            // A stale action/resource_type (e.g. a capability pruned from the
            // catalog) is dropped, never fatal: one unknown scope must not fail
            // the whole token load and lock the holder out of every other scope
            // it still legitimately holds. Dropping a grant is fail-closed (the
            // token loses that capability) — the same rule the policy repository
            // applies in `parse_enum_list`.
            let Ok(action) = serde_json::from_str::<Action>(&format!("\"{action_raw}\"")) else {
                continue;
            };
            let Ok(resource_type) =
                serde_json::from_str::<ResourceType>(&format!("\"{resource_raw}\""))
            else {
                continue;
            };
            if !is_valid_permission_pair(&action, &resource_type) {
                continue;
            }
            scopes.push(UserApiTokenScope {
                action,
                resource_type,
                resource_pattern,
            });
        }
        Ok(scopes)
    }

    fn parse_token_row(row: &sqlx::postgres::PgRow) -> Result<UserApiToken, AppError> {
        let status_raw: String = row
            .try_get("status")
            .map_err(|error| AppError::Database(format!("Failed to read token status: {error}")))?;
        let status = UserApiTokenStatus::parse(status_raw.as_str())
            .ok_or_else(|| AppError::Database("Invalid token status in DB".to_string()))?;
        Ok(UserApiToken {
            id: row
                .try_get("id")
                .map_err(|error| AppError::Database(format!("Failed to read token id: {error}")))?,
            user_id: row.try_get("user_id").map_err(|error| {
                AppError::Database(format!("Failed to read token user_id: {error}"))
            })?,
            name: row.try_get("name").map_err(|error| {
                AppError::Database(format!("Failed to read token name: {error}"))
            })?,
            token_prefix: row.try_get("token_prefix").map_err(|error| {
                AppError::Database(format!("Failed to read token prefix: {error}"))
            })?,
            token_hash: row.try_get("token_hash").map_err(|error| {
                AppError::Database(format!("Failed to read token hash: {error}"))
            })?,
            status,
            expires_at: row.try_get("expires_at").map_err(|error| {
                AppError::Database(format!("Failed to read token expires_at: {error}"))
            })?,
            last_used_at: row.try_get("last_used_at").map_err(|error| {
                AppError::Database(format!("Failed to read token last_used_at: {error}"))
            })?,
            created_at: row.try_get("created_at").map_err(|error| {
                AppError::Database(format!("Failed to read token created_at: {error}"))
            })?,
            updated_at: row.try_get("updated_at").map_err(|error| {
                AppError::Database(format!("Failed to read token updated_at: {error}"))
            })?,
            revoked_at: row.try_get("revoked_at").map_err(|error| {
                AppError::Database(format!("Failed to read token revoked_at: {error}"))
            })?,
            revoked_by_user_id: row.try_get("revoked_by_user_id").map_err(|error| {
                AppError::Database(format!("Failed to read token revoked_by_user_id: {error}"))
            })?,
            scopes: Vec::new(),
        })
    }

    fn push_list_filters(
        builder: &mut sqlx::QueryBuilder<'_, Postgres>,
        user_id: Uuid,
        filter: &UserApiTokenListFilter,
    ) {
        builder.push(" WHERE t.user_id = ");
        builder.push_bind(user_id);

        if let Some(search) = filter
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let pattern = format!("%{search}%");
            builder.push(
                r#"
                AND (
                    t.name ILIKE
                "#,
            );
            builder.push_bind(pattern.clone());
            builder.push(" OR t.token_prefix ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(
                r#"
                    OR EXISTS (
                        SELECT 1
                        FROM auth_user_api_token_scope s
                        WHERE s.token_id = t.id
                          AND CASE
                                WHEN LOWER(s.resource_type) = 'cctv'
                                  THEN 'cctv:' || LOWER(s.action)
                                ELSE LOWER(s.action) || ':' || LOWER(s.resource_type)
                              END ILIKE
                "#,
            );
            builder.push_bind(pattern);
            builder.push("))");
        }

        if !filter.scopes.is_empty() {
            builder.push(
                r#"
                AND EXISTS (
                    SELECT 1
                    FROM auth_user_api_token_scope s
                    WHERE s.token_id = t.id
                      AND CASE
                            WHEN LOWER(s.resource_type) = 'cctv'
                              THEN 'cctv:' || LOWER(s.action)
                            ELSE LOWER(s.action) || ':' || LOWER(s.resource_type)
                          END IN (
                "#,
            );
            {
                let mut separated = builder.separated(", ");
                for scope in &filter.scopes {
                    separated.push_bind(scope.to_ascii_lowercase());
                }
            }
            builder.push("))");
        }

        if !filter.status.is_empty() {
            builder.push(" AND t.status IN (");
            {
                let mut separated = builder.separated(", ");
                for status in &filter.status {
                    separated.push_bind(status.to_ascii_lowercase());
                }
            }
            builder.push(")");
        }
    }
}

#[async_trait]
impl UserApiTokenRepository for SqlxUserApiTokenRepository {
    async fn create_token(
        &self,
        token: UserApiToken,
        audit_event: AuditEvent,
    ) -> Result<UserApiToken, AppError> {
        let mut tx = self.pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to start token create transaction: {error}"))
        })?;

        // Serialize credential issuance with seat eviction. After an eviction
        // commits, an in-flight token request must not mint a new credential.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(421_001_i64)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to lock seat issuance: {error}"))
            })?;
        let has_seat: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM license_seat_assignment WHERE user_id = $1 AND revoked_at IS NULL)",
        )
        .bind(token.user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| AppError::Database(format!("Failed to check seat issuance: {error}")))?;
        if !has_seat {
            return Err(AppError::Forbidden(
                "LICENSE_SEAT_LIMIT_EXCEEDED: no active license seat".to_string(),
            ));
        }

        let row = sqlx::query(
            r#"
            INSERT INTO auth_user_api_token (
                id,
                user_id,
                name,
                token_prefix,
                token_hash,
                status,
                expires_at,
                last_used_at,
                created_at,
                updated_at,
                revoked_at,
                revoked_by_user_id
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id, user_id, name, token_prefix, token_hash, status,
                      expires_at, last_used_at, created_at, updated_at, revoked_at, revoked_by_user_id
            "#,
        )
        .bind(token.id)
        .bind(token.user_id)
        .bind(token.name)
        .bind(token.token_prefix)
        .bind(token.token_hash)
        .bind(token.status.as_str())
        .bind(token.expires_at)
        .bind(token.last_used_at)
        .bind(token.created_at)
        .bind(token.updated_at)
        .bind(token.revoked_at)
        .bind(token.revoked_by_user_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(|error| AppError::Database(format!("Failed to create user API token: {error}")))?;

        for scope in &token.scopes {
            let action_raw = serde_json::to_value(&scope.action)
                .ok()
                .and_then(|value| value.as_str().map(ToString::to_string))
                .ok_or_else(|| {
                    AppError::Internal("Failed to serialize token scope action".to_string())
                })?;
            let resource_raw = serde_json::to_value(&scope.resource_type)
                .ok()
                .and_then(|value| value.as_str().map(ToString::to_string))
                .ok_or_else(|| {
                    AppError::Internal("Failed to serialize token scope resource type".to_string())
                })?;

            sqlx::query(
                r#"
                INSERT INTO auth_user_api_token_scope (
                    id,
                    token_id,
                    action,
                    resource_type,
                    resource_pattern
                )
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(token.id)
            .bind(action_raw)
            .bind(resource_raw)
            .bind(scope.resource_pattern.as_str())
            .execute(tx.as_mut())
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to insert user API token scope: {error}"))
            })?;
        }

        let mut token = Self::parse_token_row(&row)?;
        token.scopes = Self::load_scopes(&mut tx, token.id).await?;
        insert_audit_event(tx.as_mut(), &audit_event).await?;
        tx.commit().await.map_err(|error| {
            AppError::Database(format!("Failed to commit token create: {error}"))
        })?;
        Ok(token)
    }

    async fn list_user_tokens_page(
        &self,
        user_id: Uuid,
        limit: u32,
        offset: u32,
        filter: UserApiTokenListFilter,
    ) -> Result<Vec<UserApiToken>, AppError> {
        let mut tx = self.pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to start list transaction: {error}"))
        })?;

        let mut builder = sqlx::QueryBuilder::new(
            r#"
            SELECT id, user_id, name, token_prefix, token_hash, status,
                   expires_at, last_used_at, created_at, updated_at, revoked_at, revoked_by_user_id
            FROM auth_user_api_token t
            "#,
        );
        Self::push_list_filters(&mut builder, user_id, &filter);
        push_order_by(
            &mut builder,
            USER_API_TOKEN_COLUMNS,
            filter.sort.as_ref(),
            "t.created_at DESC, t.id",
        );
        if filter.sort.is_some() {
            builder.push(", t.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(i64::from(limit));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(offset));

        let rows = builder
            .build()
            .fetch_all(tx.as_mut())
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to list user API tokens: {error}"))
            })?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let mut token = Self::parse_token_row(&row)?;
            token.scopes = Self::load_scopes(&mut tx, token.id).await?;
            items.push(token);
        }
        tx.commit().await.map_err(|error| {
            AppError::Database(format!(
                "Failed to commit user API token list transaction: {error}"
            ))
        })?;
        Ok(items)
    }

    async fn count_user_tokens(
        &self,
        user_id: Uuid,
        filter: UserApiTokenListFilter,
    ) -> Result<u64, AppError> {
        let mut builder = sqlx::QueryBuilder::new(
            r#"
            SELECT COUNT(*)::bigint
            FROM auth_user_api_token t
            "#,
        );
        Self::push_list_filters(&mut builder, user_id, &filter);
        let total: i64 = builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to count user API tokens: {error}"))
            })?;

        Ok(total as u64)
    }

    async fn find_token_with_scopes(
        &self,
        token_id: Uuid,
    ) -> Result<Option<UserApiToken>, AppError> {
        let mut tx = self.pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to start find token transaction: {error}"))
        })?;

        let row = sqlx::query(
            r#"
            SELECT id, user_id, name, token_prefix, token_hash, status,
                   expires_at, last_used_at, created_at, updated_at, revoked_at, revoked_by_user_id
            FROM auth_user_api_token
            WHERE id = $1
            "#,
        )
        .bind(token_id)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(|error| AppError::Database(format!("Failed to load user API token: {error}")))?;

        let token = match row {
            Some(row) => {
                let mut token = Self::parse_token_row(&row)?;
                token.scopes = Self::load_scopes(&mut tx, token.id).await?;
                Some(token)
            }
            None => None,
        };

        tx.commit().await.map_err(|error| {
            AppError::Database(format!("Failed to commit find token transaction: {error}"))
        })?;

        Ok(token)
    }

    async fn revoke_user_token(
        &self,
        user_id: Uuid,
        token_id: Uuid,
        now: DateTime<Utc>,
        audit_event: AuditEvent,
    ) -> Result<Option<UserApiToken>, AppError> {
        let mut tx = self.pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to start revoke transaction: {error}"))
        })?;

        let row = sqlx::query(
            r#"
            UPDATE auth_user_api_token
            SET status = 'revoked',
                revoked_at = $3,
                revoked_by_user_id = $1,
                updated_at = $3
            WHERE id = $2
              AND user_id = $1
            RETURNING id, user_id, name, token_prefix, token_hash, status,
                      expires_at, last_used_at, created_at, updated_at, revoked_at, revoked_by_user_id
            "#,
        )
        .bind(user_id)
        .bind(token_id)
        .bind(now)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(|error| AppError::Database(format!("Failed to revoke user API token: {error}")))?;

        let token = match row {
            Some(row) => {
                let mut token = Self::parse_token_row(&row)?;
                token.scopes = Self::load_scopes(&mut tx, token.id).await?;
                Some(token)
            }
            None => None,
        };

        if token.is_some() {
            insert_audit_event(tx.as_mut(), &audit_event).await?;
        }

        tx.commit().await.map_err(|error| {
            AppError::Database(format!(
                "Failed to commit revoke token transaction: {error}"
            ))
        })?;
        Ok(token)
    }

    async fn touch_last_used(&self, token_id: Uuid, now: DateTime<Utc>) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE auth_user_api_token
            SET last_used_at = $2,
                updated_at = $2
            WHERE id = $1
            "#,
        )
        .bind(token_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to update token last_used_at: {error}"))
        })?;
        Ok(())
    }

    async fn count_active_tokens_for_user(
        &self,
        user_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<u32, AppError> {
        let total = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)::bigint
            FROM auth_user_api_token
            WHERE user_id = $1
              AND status = 'active'
              AND (expires_at IS NULL OR expires_at > $2)
            "#,
        )
        .bind(user_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to count active user API tokens: {error}"))
        })?;

        Ok(total as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::USER_API_TOKEN_COLUMNS;
    use crate::contexts::shared::list_query::{SortSpec, list_sort_column};
    use crate::errors::AppError;

    #[test]
    fn token_sort_allowlist_uses_http_query_keys_not_ui_column_aliases() {
        assert_eq!(
            list_sort_column(USER_API_TOKEN_COLUMNS, "name"),
            Some("t.name")
        );
        assert_eq!(
            list_sort_column(USER_API_TOKEN_COLUMNS, "created_at"),
            Some("t.created_at")
        );
        assert_eq!(
            list_sort_column(USER_API_TOKEN_COLUMNS, "expires_at"),
            Some("t.expires_at")
        );
        assert!(list_sort_column(USER_API_TOKEN_COLUMNS, "createdAt").is_none());
        assert!(list_sort_column(USER_API_TOKEN_COLUMNS, "expiresAt").is_none());
    }

    #[test]
    fn token_sort_parser_rejects_removed_ui_alias_keys() {
        let err = SortSpec::parse_columns(Some("createdAt"), None, USER_API_TOKEN_COLUMNS);
        assert!(matches!(err, Err(AppError::Validation(_))));
    }
}
