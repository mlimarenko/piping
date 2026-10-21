use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::repository::{NamedListFilter, ROLE_COLUMNS, RoleRepository};
use crate::contexts::access_control::domain::Role;
use crate::contexts::shared::list_query::push_order_by;
use crate::contexts::shared::pagination::PageResult;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxRoleRepository {
    pool: PgPool,
}

impl SqlxRoleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_role_row(row: &PgRow) -> Result<Role, AppError> {
        Ok(Role {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read role id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read role name: {}", e)))?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read role description: {}", e))
            })?,
        })
    }
}

#[async_trait]
impl RoleRepository for SqlxRoleRepository {
    async fn list_roles_for_user(&self, user_id: Uuid) -> Result<Vec<Role>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT r.id, r.name, r.description
            FROM rbac_role r
            JOIN rbac_role_assignment ra ON ra.role_id = r.id
            WHERE ra.user_id = $1
               OR EXISTS (
                    SELECT 1
                    FROM identity_group_membership gm
                    WHERE gm.user_id = $1
                      AND gm.group_id = ra.group_id
               )
            ORDER BY r.name
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list roles for user: {}", e)))?;

        rows.iter()
            .map(Self::parse_role_row)
            .collect::<Result<Vec<Role>, AppError>>()
    }

    async fn get_by_id(&self, role_id: Uuid) -> Result<Option<Role>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, description
            FROM rbac_role
            WHERE id = $1
            "#,
        )
        .bind(role_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load role: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_role_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_roles(&self) -> Result<Vec<Role>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, description
            FROM rbac_role
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list roles: {}", e)))?;

        rows.iter()
            .map(Self::parse_role_row)
            .collect::<Result<Vec<Role>, AppError>>()
    }

    async fn list_roles_page(
        &self,
        filter: NamedListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Role>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));

        let push_where = |builder: &mut QueryBuilder<'_, Postgres>| {
            if let Some(search) = filter.search.as_deref() {
                builder.push(" WHERE name ILIKE ");
                builder.push_bind(search.to_string());
            }
        };

        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT id, name, description FROM rbac_role");
        push_where(&mut builder);
        push_order_by(&mut builder, ROLE_COLUMNS, filter.sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(offset as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list roles page: {}", e)))?;

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*)::bigint FROM rbac_role");
        push_where(&mut count_builder);
        let total: i64 = count_builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count roles: {}", e)))?;

        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Roles total count overflow".to_string()))?;

        Ok(PageResult {
            items: rows
                .iter()
                .map(Self::parse_role_row)
                .collect::<Result<Vec<Role>, AppError>>()?,
            total,
            limit,
            offset,
        })
    }

    async fn create_role(&self, role: Role) -> Result<Role, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO rbac_role (id, name, description)
            VALUES ($1, $2, $3)
            RETURNING id, name, description
            "#,
        )
        .bind(role.id)
        .bind(&role.name)
        .bind(&role.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create role: {}", e)))?;

        Self::parse_role_row(&row)
    }

    async fn update_role(&self, role: Role) -> Result<Role, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE rbac_role
            SET name = $2,
                description = $3
            WHERE id = $1
            RETURNING id, name, description
            "#,
        )
        .bind(role.id)
        .bind(&role.name)
        .bind(&role.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update role: {}", e)))?;

        Self::parse_role_row(&row)
    }

    async fn delete_role(&self, role_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM rbac_role WHERE id = $1")
            .bind(role_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete role: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }
}

/// Expand raw OIDC group claims into both the flat (`group`) and full-path
/// (`/group`) forms used to match `identity_group.external_id`.
///
/// This is the single normalization used by BOTH the role-resolution query and
/// the canonical subject resolver, so mapped-vs-fallback decisions stay in
/// lockstep with role resolution (codex CRIT #2 — OIDC normalization parity).
pub(crate) fn expand_oidc_group_ids(oidc_group_ids: &[String]) -> Vec<String> {
    let mut expanded = Vec::new();
    for group_id in oidc_group_ids {
        let trimmed = group_id.trim();
        if trimmed.is_empty() {
            continue;
        }
        expanded.push(trimmed.to_string());
        if let Some(stripped) = trimmed.strip_prefix('/') {
            if !stripped.is_empty() {
                expanded.push(stripped.to_string());
            }
        } else {
            expanded.push(format!("/{trimmed}"));
        }
    }
    expanded.sort();
    expanded.dedup();
    expanded
}

#[cfg(test)]
mod tests {
    use super::expand_oidc_group_ids;

    #[test]
    fn expand_oidc_group_ids_accepts_flat_and_full_path_group_claims() {
        let groups = vec![
            "piping".to_string(),
            "/piping-editor".to_string(),
            "  /piping-admin  ".to_string(),
            "".to_string(),
        ];

        let expanded = expand_oidc_group_ids(&groups);

        assert_eq!(
            expanded,
            vec![
                "/piping".to_string(),
                "/piping-admin".to_string(),
                "/piping-editor".to_string(),
                "piping".to_string(),
                "piping-admin".to_string(),
                "piping-editor".to_string(),
            ]
        );
    }
}
