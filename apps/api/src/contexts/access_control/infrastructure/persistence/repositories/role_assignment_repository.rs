use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::repository::{ROLE_ASSIGNMENT_COLUMNS, RoleAssignmentFilter, RoleAssignmentRepository};
use crate::contexts::access_control::domain::RoleAssignment;
use crate::contexts::shared::list_query::{WhereState, push_order_by, push_uuid_in};
use crate::contexts::shared::pagination::PageResult;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxRoleAssignmentRepository {
    pool: PgPool,
}

impl SqlxRoleAssignmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_assignment_row(row: &PgRow) -> Result<RoleAssignment, AppError> {
        Ok(RoleAssignment {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read role assignment id: {}", e))
            })?,
            user_id: row.try_get("user_id").map_err(|e| {
                AppError::Database(format!("Failed to read role assignment user_id: {}", e))
            })?,
            group_id: row.try_get("group_id").map_err(|e| {
                AppError::Database(format!("Failed to read role assignment group_id: {}", e))
            })?,
            role_id: row.try_get("role_id").map_err(|e| {
                AppError::Database(format!("Failed to read role assignment role_id: {}", e))
            })?,
        })
    }

    /// Emit the shared `FROM … JOIN …` and `WHERE` clause for assignment list
    /// queries. Reused verbatim by the page query and its count query so
    /// `total` stays in lockstep with `items`.
    fn push_from_and_where(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &RoleAssignmentFilter,
    ) {
        builder.push(
            " FROM rbac_role_assignment ra \
             LEFT JOIN rbac_role r ON r.id = ra.role_id \
             LEFT JOIN identity_user u ON u.id = ra.user_id \
             LEFT JOIN identity_group g ON g.id = ra.group_id",
        );
        let mut where_state = WhereState::new();
        push_uuid_in(builder, &mut where_state, "ra.role_id", &filter.role_id);
        push_uuid_in(builder, &mut where_state, "ra.user_id", &filter.user_id);
        push_uuid_in(builder, &mut where_state, "ra.group_id", &filter.group_id);
        let wants_user = filter.subject_type.iter().any(|value| value == "user");
        let wants_group = filter.subject_type.iter().any(|value| value == "group");
        match (wants_user, wants_group) {
            (true, false) => {
                where_state.and(builder);
                builder.push("ra.user_id IS NOT NULL");
            }
            (false, true) => {
                where_state.and(builder);
                builder.push("ra.group_id IS NOT NULL");
            }
            _ => {}
        }
        if let Some(search) = filter.search.as_deref() {
            let pattern = format!("%{search}%");
            where_state.and(builder);
            builder.push("(r.name ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR u.username ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR u.email ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR g.name ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR ra.role_id::text ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR ra.user_id::text ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR ra.group_id::text ILIKE ");
            builder.push_bind(pattern);
            builder.push(")");
        }
    }
}

#[async_trait]
impl RoleAssignmentRepository for SqlxRoleAssignmentRepository {
    async fn list_assignments(&self) -> Result<Vec<RoleAssignment>, AppError> {
        self.list_assignments_filtered(RoleAssignmentFilter::default())
            .await
    }

    async fn list_assignments_filtered(
        &self,
        filter: RoleAssignmentFilter,
    ) -> Result<Vec<RoleAssignment>, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT ra.id, ra.user_id, ra.group_id, ra.role_id");
        Self::push_from_and_where(&mut builder, &filter);
        builder.push(" ORDER BY ra.id");
        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list filtered role assignments: {}", e))
        })?;

        rows.iter()
            .map(Self::parse_assignment_row)
            .collect::<Result<Vec<RoleAssignment>, AppError>>()
    }

    async fn list_assignments_page(
        &self,
        filter: RoleAssignmentFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<RoleAssignment>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));

        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT ra.id, ra.user_id, ra.group_id, ra.role_id");
        Self::push_from_and_where(&mut builder, &filter);
        push_order_by(
            &mut builder,
            ROLE_ASSIGNMENT_COLUMNS,
            filter.sort.as_ref(),
            "ra.id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(offset as i64);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list role assignments page: {}", e))
        })?;

        let mut count_builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*)::bigint");
        Self::push_from_and_where(&mut count_builder, &filter);
        let total: i64 = count_builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count role assignments: {}", e)))?;

        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Role assignments total count overflow".to_string()))?;

        Ok(PageResult {
            items: rows
                .iter()
                .map(Self::parse_assignment_row)
                .collect::<Result<Vec<RoleAssignment>, AppError>>()?,
            total,
            limit,
            offset,
        })
    }

    async fn create_assignment(
        &self,
        assignment: RoleAssignment,
    ) -> Result<RoleAssignment, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO rbac_role_assignment (id, user_id, group_id, role_id)
            VALUES ($1, $2, $3, $4)
            RETURNING id, user_id, group_id, role_id
            "#,
        )
        .bind(assignment.id)
        .bind(assignment.user_id)
        .bind(assignment.group_id)
        .bind(assignment.role_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create role assignment: {}", e)))?;

        Self::parse_assignment_row(&row)
    }

    async fn delete_assignment(&self, assignment_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM rbac_role_assignment WHERE id = $1")
            .bind(assignment_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete role assignment: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }
}
