use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in,
};
use crate::errors::AppError;

pub const WEBHOOK_SUBSCRIPTION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "webhooks.fields.name"),
    ListColumn::sortable("target_url", "target_url", "webhooks.fields.targetUrl"),
    ListColumn::enum_filter(
        "secret_set",
        "secret IS NOT NULL",
        "CASE WHEN secret IS NOT NULL THEN 'true' ELSE 'false' END",
        "webhooks.fields.secret",
    ),
    ListColumn::enum_filter(
        "event_types",
        "array_to_string(event_types, ',')",
        "unnest(event_types)",
        "webhooks.fields.eventTypes",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "CASE WHEN enabled THEN 'true' ELSE 'false' END",
        "webhooks.fields.enabled",
    ),
    ListColumn::sortable(
        "last_fired_at",
        "last_fired_at",
        "webhooks.fields.lastFired",
    ),
    ListColumn::enum_filter(
        "last_status",
        "last_status",
        "COALESCE(last_status::text, '')",
        "webhooks.fields.lastStatus",
    ),
    ListColumn::sortable("created_at", "created_at", "webhooks.fields.createdAt"),
];

/// List parameters for webhook subscriptions: multi-value column filters and
/// an allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct WebhookSubscriptionListFilter {
    /// Multi-value `enabled` IN filter (matches any).
    pub enabled: Vec<String>,
    /// Multi-value `secret IS NOT NULL` IN filter (matches any).
    pub secret_set: Vec<String>,
    /// Event type overlap filter; matches any subscription with at least one selected event type.
    pub event_types: Vec<String>,
    /// Multi-value `last_status` IN filter (matches any non-null status).
    pub last_status: Vec<String>,
    /// Case-insensitive search across visible webhook identity fields.
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone)]
pub struct WebhookSubscription {
    pub id: Uuid,
    pub name: String,
    pub target_url: String,
    pub secret: Option<String>,
    pub event_types: Vec<String>,
    pub enabled: bool,
    /// Optional NetBox-style condition filter; `None` = deliver on every matching event_type.
    pub conditions: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub last_status: Option<i32>,
    pub last_error: Option<String>,
}

pub struct CreateWebhookSubscription {
    pub name: String,
    pub target_url: String,
    pub secret: Option<String>,
    pub event_types: Vec<String>,
    pub enabled: bool,
    pub conditions: Option<serde_json::Value>,
}

pub struct UpdateWebhookSubscription {
    pub name: Option<String>,
    pub target_url: Option<String>,
    pub secret: Option<Option<String>>,
    pub event_types: Option<Vec<String>>,
    pub enabled: Option<bool>,
    /// `None` = leave conditions unchanged; `Some(None)` = clear; `Some(Some(v))` = set.
    pub conditions: Option<Option<serde_json::Value>>,
}

pub struct UpdateWebhookFired {
    pub last_status: i32,
    pub last_error: Option<String>,
    pub last_fired_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SqlxWebhookSubscriptionRepository {
    pool: PgPool,
}

impl SqlxWebhookSubscriptionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(
        &self,
        filter: &WebhookSubscriptionListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<WebhookSubscription>, u64), AppError> {
        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM webhook_subscriptions");
        Self::push_filter(&mut count_builder, filter);
        let total: i64 = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await?;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, name, target_url, secret, event_types, enabled, conditions,
                   created_at, updated_at, last_fired_at, last_status, last_error
            FROM webhook_subscriptions
            "#,
        );
        Self::push_filter(&mut builder, filter);
        push_order_by(
            &mut builder,
            WEBHOOK_SUBSCRIPTION_COLUMNS,
            filter.sort.as_ref(),
            "created_at DESC, id",
        );
        if filter.sort.is_some() {
            builder.push(", id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(i64::from(limit));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(offset));

        let rows = builder.build().fetch_all(&self.pool).await?;
        let items = rows.iter().map(map_row).collect::<Result<Vec<_>, _>>()?;
        Ok((items, total as u64))
    }

    /// Emit the shared `WHERE` clause for webhook-subscription list queries,
    /// reused verbatim by the page query and its count query so `total` stays in
    /// lockstep with `items`. Column filters are multi-value match-any filters.
    fn push_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &WebhookSubscriptionListFilter,
    ) {
        let mut where_state = WhereState::new();

        if !filter.enabled.is_empty() {
            push_string_in(
                builder,
                &mut where_state,
                "CASE WHEN enabled THEN 'true' ELSE 'false' END",
                &filter.enabled,
            );
        }
        if !filter.secret_set.is_empty() {
            push_string_in(
                builder,
                &mut where_state,
                "CASE WHEN secret IS NOT NULL THEN 'true' ELSE 'false' END",
                &filter.secret_set,
            );
        }
        if !filter.event_types.is_empty() {
            where_state.and(builder);
            builder.push("event_types && ");
            builder.push_bind(filter.event_types.clone());
        }
        if !filter.last_status.is_empty() {
            push_string_in(
                builder,
                &mut where_state,
                "COALESCE(last_status::text, '')",
                &filter.last_status,
            );
        }
        if let Some(search) = filter.search.as_ref() {
            let pattern = format!("%{search}%");
            where_state.and(builder);
            builder.push("(name ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR target_url ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR array_to_string(event_types, ',') ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR COALESCE(last_error, '') ILIKE ");
            builder.push_bind(pattern);
            builder.push(")");
        }
    }

    #[cfg(test)]
    fn filter_sql_for_tests(filter: &WebhookSubscriptionListFilter) -> String {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT 1 FROM webhook_subscriptions");
        Self::push_filter(&mut builder, filter);
        builder.sql().to_string()
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<WebhookSubscription>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, target_url, secret, event_types, enabled, conditions,
                   created_at, updated_at, last_fired_at, last_status, last_error
            FROM webhook_subscriptions
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(map_row(&row)?)),
            None => Ok(None),
        }
    }

    /// Fetch all enabled subscriptions whose `event_types` array contains `event_type`.
    pub async fn list_enabled_for_event_type(
        &self,
        event_type: &str,
    ) -> Result<Vec<WebhookSubscription>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, target_url, secret, event_types, enabled, conditions,
                   created_at, updated_at, last_fired_at, last_status, last_error
            FROM webhook_subscriptions
            WHERE enabled AND $1 = ANY(event_types)
            "#,
        )
        .bind(event_type)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(map_row).collect()
    }

    pub async fn create(
        &self,
        input: &CreateWebhookSubscription,
    ) -> Result<WebhookSubscription, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO webhook_subscriptions (name, target_url, secret, event_types, enabled, conditions)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, target_url, secret, event_types, enabled, conditions,
                      created_at, updated_at, last_fired_at, last_status, last_error
            "#,
        )
        .bind(&input.name)
        .bind(&input.target_url)
        .bind(&input.secret)
        .bind(&input.event_types)
        .bind(input.enabled)
        .bind(&input.conditions)
        .fetch_one(&self.pool)
        .await?;

        map_row(&row)
    }

    pub async fn update(
        &self,
        id: Uuid,
        input: &UpdateWebhookSubscription,
    ) -> Result<Option<WebhookSubscription>, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE webhook_subscriptions SET
                name = COALESCE($2, name),
                target_url = COALESCE($3, target_url),
                secret = CASE WHEN $4 THEN $5 ELSE secret END,
                event_types = COALESCE($6, event_types),
                enabled = COALESCE($7, enabled),
                conditions = CASE WHEN $8 THEN $9 ELSE conditions END,
                updated_at = now()
            WHERE id = $1
            RETURNING id, name, target_url, secret, event_types, enabled, conditions,
                      created_at, updated_at, last_fired_at, last_status, last_error
            "#,
        )
        .bind(id)
        .bind(input.name.as_deref())
        .bind(input.target_url.as_deref())
        .bind(input.secret.is_some())
        .bind(input.secret.as_ref().and_then(|s| s.as_deref()))
        .bind(input.event_types.as_ref())
        .bind(input.enabled)
        .bind(input.conditions.is_some())
        .bind(input.conditions.as_ref().and_then(|c| c.as_ref()))
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(map_row(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM webhook_subscriptions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn clear_secret(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query(
            "UPDATE webhook_subscriptions SET secret = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_fired(
        &self,
        id: Uuid,
        update: &UpdateWebhookFired,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE webhook_subscriptions SET
                last_status = $2,
                last_error = $3,
                last_fired_at = $4,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(update.last_status)
        .bind(&update.last_error)
        .bind(update.last_fired_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<WebhookSubscription, AppError> {
    Ok(WebhookSubscription {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        target_url: row.try_get("target_url")?,
        secret: row.try_get("secret")?,
        event_types: row.try_get("event_types")?,
        enabled: row.try_get("enabled")?,
        conditions: row.try_get("conditions")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        last_fired_at: row.try_get("last_fired_at")?,
        last_status: row.try_get("last_status")?,
        last_error: row.try_get("last_error")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_filter_sql_combines_webhook_table_filters() {
        let sql = SqlxWebhookSubscriptionRepository::filter_sql_for_tests(
            &WebhookSubscriptionListFilter {
                enabled: vec!["true".to_string()],
                secret_set: vec!["false".to_string()],
                event_types: vec!["inventory.device.created".to_string()],
                last_status: vec!["500".to_string()],
                search: None,
                sort: None,
            },
        );

        assert!(sql.contains(" WHERE CASE WHEN enabled THEN 'true' ELSE 'false' END IN ($1)"));
        assert!(
            sql.contains(" AND CASE WHEN secret IS NOT NULL THEN 'true' ELSE 'false' END IN ($2)")
        );
        assert!(sql.contains(" AND event_types && $3"));
        assert!(sql.contains(" AND COALESCE(last_status::text, '') IN ($4)"));
    }
}
