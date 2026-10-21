use async_trait::async_trait;
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::repository::{POLICY_COLUMNS, PolicyFilter, PolicyRepository};
use crate::contexts::access_control::domain::{Action, Policy, PolicyEffect, ResourceType};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::list_query::push_order_by;
use crate::contexts::shared::pagination::PageResult;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxPolicyRepository {
    pool: PgPool,
}

impl SqlxPolicyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Emit the shared `WHERE` clause for policy list queries. Reused verbatim
    /// by the page query and its count query so `total` stays in lockstep with
    /// `items`. `effect`/`enabled` are multi-value `IN` filters (match any).
    fn push_filter(builder: &mut QueryBuilder<'_, Postgres>, filter: &PolicyFilter) {
        let mut where_state = WhereState::new();
        if let Some(name) = filter.name.as_deref() {
            where_state.and(builder);
            builder.push("name ILIKE ");
            builder.push_bind(format!("%{name}%"));
        }
        if !filter.effect.is_empty() {
            where_state.and(builder);
            builder.push("effect IN (");
            let mut separated = builder.separated(", ");
            for effect in &filter.effect {
                separated.push_bind(effect_to_string(*effect));
            }
            builder.push(")");
        }
        if !filter.enabled.is_empty() {
            where_state.and(builder);
            builder.push("enabled IN (");
            let mut separated = builder.separated(", ");
            for enabled in &filter.enabled {
                separated.push_bind(*enabled);
            }
            builder.push(")");
        }
    }

    fn parse_string_list(value: Value, field_name: &str) -> Result<Vec<String>, AppError> {
        match value {
            Value::Array(items) => items
                .into_iter()
                .map(|item| {
                    item.as_str().map(|value| value.to_string()).ok_or_else(|| {
                        AppError::Database(format!("Invalid {} entry: expected string", field_name))
                    })
                })
                .collect(),
            Value::Object(_) | Value::Null => Ok(Vec::new()),
            other => Err(AppError::Database(format!(
                "Invalid {} payload: {}",
                field_name, other
            ))),
        }
    }

    /// Deserialize a stored jsonb array of Action/ResourceType enum values,
    /// SKIPPING any element that no longer maps to a live variant instead of
    /// failing the whole row.
    ///
    /// A vocabulary variant can be removed (e.g. the pruned Device /
    /// VirtualMachine / PlaybookJobToken resource types). A persisted
    /// customer-authored policy may still reference the removed value. Strict
    /// `from_value::<Vec<T>>` would hard-error on it, and since a single bad row
    /// fails the whole `list_for_subjects` query, that would fail EVERY
    /// authorization check for the affected subject closed (500 lockout). A
    /// dropped element simply never matches — the intended "stale → no match"
    /// degradation — which is safe because an all-or-nothing empty list still
    /// default-denies. The only structural error left is a non-array payload.
    fn parse_enum_list<T: serde::de::DeserializeOwned>(
        value: Value,
        field_name: &str,
    ) -> Result<Vec<T>, AppError> {
        match value {
            Value::Array(items) => Ok(items
                .into_iter()
                .filter_map(|item| serde_json::from_value::<T>(item).ok())
                .collect()),
            Value::Null => Ok(Vec::new()),
            other => Err(AppError::Database(format!(
                "Invalid policy {field_name} payload: {other}"
            ))),
        }
    }

    fn parse_policy_row(row: &PgRow) -> Result<Policy, AppError> {
        let effect: String = row
            .try_get("effect")
            .map_err(|e| AppError::Database(format!("Failed to read policy effect: {}", e)))?;
        let effect = match effect.as_str() {
            "allow" => PolicyEffect::Allow,
            "deny" => PolicyEffect::Deny,
            other => {
                return Err(AppError::Database(format!(
                    "Unknown policy effect value: {}",
                    other
                )));
            }
        };

        let actions: Value = row
            .try_get("actions")
            .map_err(|e| AppError::Database(format!("Failed to read policy actions: {}", e)))?;
        let actions = Self::parse_enum_list::<Action>(actions, "actions")?;

        let resource_types: Value = row.try_get("resource_types").map_err(|e| {
            AppError::Database(format!("Failed to read policy resource types: {}", e))
        })?;
        let resource_types =
            Self::parse_enum_list::<ResourceType>(resource_types, "resource types")?;

        let constraints: Option<Value> = row
            .try_get("constraints")
            .map_err(|e| AppError::Database(format!("Failed to read policy constraints: {}", e)))?;

        let subjects: Value = row
            .try_get("subjects")
            .map_err(|e| AppError::Database(format!("Failed to read policy subjects: {}", e)))?;
        let subjects = Self::parse_string_list(subjects, "subjects")?;

        let resources: Value = row
            .try_get("resources")
            .map_err(|e| AppError::Database(format!("Failed to read policy resources: {}", e)))?;
        let resources = Self::parse_string_list(resources, "resources")?;

        let enabled: Option<bool> = row
            .try_get("enabled")
            .map_err(|e| AppError::Database(format!("Failed to read policy enabled: {}", e)))?;
        let enabled = enabled.unwrap_or(true);

        Ok(Policy {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read policy id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read policy name: {}", e)))?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read policy description: {}", e))
            })?,
            effect,
            actions,
            resource_types,
            constraints,
            subjects,
            resources,
            enabled,
        })
    }
}

#[async_trait]
impl PolicyRepository for SqlxPolicyRepository {
    async fn list_policies(&self, filter: PolicyFilter) -> Result<Vec<Policy>, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                   enabled
            FROM rbac_policy
            "#,
        );
        Self::push_filter(&mut builder, &filter);
        builder.push(" ORDER BY name");

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list policies: {}", e)))?;

        rows.iter()
            .map(Self::parse_policy_row)
            .collect::<Result<Vec<Policy>, AppError>>()
    }

    async fn list_policies_page(
        &self,
        filter: PolicyFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Policy>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));

        // Page query (rows)
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                   enabled
            FROM rbac_policy
            "#,
        );
        Self::push_filter(&mut builder, &filter);
        push_order_by(&mut builder, POLICY_COLUMNS, filter.sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(offset as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list policies page: {}", e)))?;

        // Count query (same predicates, no ORDER/LIMIT)
        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*)::bigint FROM rbac_policy");
        Self::push_filter(&mut count_builder, &filter);
        let total: i64 = count_builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count policies: {}", e)))?;

        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Policies total count overflow".to_string()))?;

        Ok(PageResult {
            items: rows
                .iter()
                .map(Self::parse_policy_row)
                .collect::<Result<Vec<Policy>, AppError>>()?,
            total,
            limit,
            offset,
        })
    }

    async fn get_policy(&self, policy_id: Uuid) -> Result<Option<Policy>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                   enabled
            FROM rbac_policy
            WHERE id = $1
            "#,
        )
        .bind(policy_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to get policy: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_policy_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn create_policy(&self, policy: Policy) -> Result<Policy, AppError> {
        let actions = serde_json::to_value(&policy.actions)
            .map_err(|e| AppError::Database(format!("Failed to encode policy actions: {}", e)))?;
        let resource_types = serde_json::to_value(&policy.resource_types).map_err(|e| {
            AppError::Database(format!("Failed to encode policy resource types: {}", e))
        })?;
        let subjects = serde_json::to_value(&policy.subjects)
            .map_err(|e| AppError::Database(format!("Failed to encode policy subjects: {}", e)))?;
        let resources = serde_json::to_value(&policy.resources)
            .map_err(|e| AppError::Database(format!("Failed to encode policy resources: {}", e)))?;

        let row = sqlx::query(
            r#"
            INSERT INTO rbac_policy (
                id, name, effect, actions, resource_types, constraints, subjects, resources,
                enabled, description
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                      enabled
            "#,
        )
        .bind(policy.id)
        .bind(policy.name)
        .bind(effect_to_string(policy.effect))
        .bind(actions)
        .bind(resource_types)
        .bind(policy.constraints)
        .bind(subjects)
        .bind(resources)
        .bind(policy.enabled)
        .bind(policy.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create policy: {}", e)))?;

        Self::parse_policy_row(&row)
    }

    async fn update_policy(&self, policy: Policy) -> Result<Policy, AppError> {
        let actions = serde_json::to_value(&policy.actions)
            .map_err(|e| AppError::Database(format!("Failed to encode policy actions: {}", e)))?;
        let resource_types = serde_json::to_value(&policy.resource_types).map_err(|e| {
            AppError::Database(format!("Failed to encode policy resource types: {}", e))
        })?;
        let subjects = serde_json::to_value(&policy.subjects)
            .map_err(|e| AppError::Database(format!("Failed to encode policy subjects: {}", e)))?;
        let resources = serde_json::to_value(&policy.resources)
            .map_err(|e| AppError::Database(format!("Failed to encode policy resources: {}", e)))?;

        let row = sqlx::query(
            r#"
            UPDATE rbac_policy
            SET name = $2,
                effect = $3,
                actions = $4,
                resource_types = $5,
                constraints = $6,
                subjects = $7,
                resources = $8,
                enabled = $9,
                description = $10
            WHERE id = $1
            RETURNING id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                      enabled
            "#,
        )
        .bind(policy.id)
        .bind(policy.name)
        .bind(effect_to_string(policy.effect))
        .bind(actions)
        .bind(resource_types)
        .bind(policy.constraints)
        .bind(subjects)
        .bind(resources)
        .bind(policy.enabled)
        .bind(policy.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update policy: {}", e)))?;

        Self::parse_policy_row(&row)
    }

    async fn delete_policy(&self, policy_id: Uuid) -> Result<(), AppError> {
        sqlx::query("DELETE FROM rbac_policy WHERE id = $1")
            .bind(policy_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete policy: {}", e)))?;
        Ok(())
    }

    async fn set_policy_enabled(&self, policy_id: Uuid, enabled: bool) -> Result<Policy, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE rbac_policy
            SET enabled = $2
            WHERE id = $1
            RETURNING id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                      enabled
            "#,
        )
        .bind(policy_id)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to set policy enabled: {}", e)))?;

        Self::parse_policy_row(&row)
    }

    async fn list_for_subjects(&self, subjects: &[String]) -> Result<Vec<Policy>, AppError> {
        if subjects.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query(
            r#"
            SELECT id, name, description, effect, actions, resource_types, constraints, subjects, resources,
                   enabled
            FROM rbac_policy
            WHERE subjects ?| $1
              AND enabled = true
            "#,
        )
        .bind(subjects)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list policies: {}", e)))?;

        rows.iter()
            .map(Self::parse_policy_row)
            .collect::<Result<Vec<Policy>, AppError>>()
    }
}

fn effect_to_string(effect: PolicyEffect) -> &'static str {
    match effect {
        PolicyEffect::Allow => "allow",
        PolicyEffect::Deny => "deny",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_enum_list_drops_pruned_variants_instead_of_failing_the_row() {
        // A persisted policy authored before Device/VirtualMachine were pruned.
        // The unknown value must be dropped, not error the whole row (which would
        // lock out every authz check for the subject holding this policy).
        let stored = json!(["CONNECTION_TARGET", "VIRTUAL_MACHINE", "INVENTORY"]);
        let parsed =
            SqlxPolicyRepository::parse_enum_list::<ResourceType>(stored, "resource types")
                .expect("stale variant must not fail the row");
        assert_eq!(
            parsed,
            vec![ResourceType::ConnectionTarget, ResourceType::Inventory],
            "known variants survive, the pruned one is silently dropped"
        );
    }

    #[test]
    fn parse_enum_list_accepts_null_as_empty() {
        let parsed = SqlxPolicyRepository::parse_enum_list::<Action>(Value::Null, "actions")
            .expect("null is empty");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_enum_list_rejects_non_array_payload() {
        let err = SqlxPolicyRepository::parse_enum_list::<Action>(json!("READ"), "actions");
        assert!(err.is_err(), "a non-array payload is a structural error");
    }
}
